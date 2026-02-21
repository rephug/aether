use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::qwen2;
use hf_hub::api::sync::ApiBuilder;
use hf_hub::{Cache, Repo, RepoType};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokenizers::Tokenizer;

use crate::{EmbeddingProvider, InferError};

pub const CANDLE_PROVIDER_NAME: &str = "candle";
pub const CANDLE_MODEL_REPO: &str = "Qwen/Qwen3-Embedding-0.6B";
pub const CANDLE_MODEL_NAME: &str = "qwen3-embedding-0.6b";
pub const CANDLE_EMBEDDING_DIM: usize = 1024;
const MODEL_DIR_NAME: &str = "qwen3-embedding-0.6b";
const CHECKSUMS_FILE: &str = "checksums.txt";
const MAX_TOKENS: usize = 8192;
const BATCH_CHUNK_SIZE: usize = 8;
const REQUIRED_FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];

#[derive(Clone)]
pub struct CandleEmbeddingProvider {
    model_dir: PathBuf,
    loaded_model: OnceLock<Arc<LoadedModel>>,
}

struct LoadedModel {
    model: Mutex<qwen2::Model>,
    tokenizer: Mutex<Tokenizer>,
    device: Device,
}

#[derive(Debug, Clone)]
struct ModelFiles {
    model_root: PathBuf,
    config_path: PathBuf,
    tokenizer_path: PathBuf,
    weights_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct QwenConfig {
    hidden_size: usize,
}

impl ModelFiles {
    fn from_root(model_root: PathBuf) -> Self {
        Self {
            config_path: model_root.join("config.json"),
            tokenizer_path: model_root.join("tokenizer.json"),
            weights_path: model_root.join("model.safetensors"),
            model_root,
        }
    }

    fn all_present(&self) -> bool {
        self.config_path.exists() && self.tokenizer_path.exists() && self.weights_path.exists()
    }
}

impl CandleEmbeddingProvider {
    pub fn new(model_dir: PathBuf) -> Self {
        Self {
            model_dir,
            loaded_model: OnceLock::new(),
        }
    }

    pub fn provider_name(&self) -> &'static str {
        CANDLE_PROVIDER_NAME
    }

    pub fn model_name(&self) -> &'static str {
        CANDLE_MODEL_NAME
    }

    pub fn ensure_model_downloaded(&self) -> Result<PathBuf, InferError> {
        let files = self.ensure_model_files()?;
        Ok(files.model_root)
    }

    fn ensure_loaded(&self) -> Result<&Arc<LoadedModel>, InferError> {
        if let Some(loaded) = self.loaded_model.get() {
            return Ok(loaded);
        }

        let files = self.ensure_model_files()?;
        let loaded = Arc::new(Self::load_model(&files)?);
        let _ = self.loaded_model.set(loaded);
        self.loaded_model.get().ok_or_else(|| {
            InferError::ModelUnavailable("failed to initialize candle model".to_owned())
        })
    }

    fn model_root(&self) -> PathBuf {
        self.model_dir.join(MODEL_DIR_NAME)
    }

    fn ensure_model_files(&self) -> Result<ModelFiles, InferError> {
        let model_root = self.model_root();
        fs::create_dir_all(&model_root)?;

        let files = ModelFiles::from_root(model_root);
        if files.all_present() && self.verify_checksums(&files)? {
            return Ok(files);
        }

        self.download_model_files(&files)?;
        self.write_checksums(&files)?;

        if !self.verify_checksums(&files)? {
            return Err(InferError::ModelUnavailable(format!(
                "downloaded model files failed checksum verification at {}",
                files.model_root.display()
            )));
        }

        Ok(files)
    }

    fn download_model_files(&self, files: &ModelFiles) -> Result<(), InferError> {
        let cache = Cache::new(files.model_root.join("hf-cache"));
        let api = ApiBuilder::from_cache(cache).build()?;
        let repo = Repo::new(CANDLE_MODEL_REPO.to_owned(), RepoType::Model);
        let api_repo = api.repo(repo);

        tracing::info!(
            model_repo = CANDLE_MODEL_REPO,
            model_dir = %files.model_root.display(),
            "downloading Candle embedding model files"
        );

        for filename in REQUIRED_FILES {
            let source_path = api_repo.get(filename).map_err(|err| {
                InferError::ModelUnavailable(format!(
                    "model not found at {} (failed downloading {} from {}: {}). Run `aetherd --download-models` with network access or switch to another embedding provider.",
                    files.model_root.display(),
                    filename,
                    CANDLE_MODEL_REPO,
                    err
                ))
            })?;

            fs::copy(&source_path, files.model_root.join(filename)).map_err(|err| {
                InferError::ModelUnavailable(format!(
                    "failed to copy downloaded model file {} into {}: {}",
                    filename,
                    files.model_root.display(),
                    err
                ))
            })?;
        }

        Ok(())
    }

    fn write_checksums(&self, files: &ModelFiles) -> Result<(), InferError> {
        let mut output = File::create(files.model_root.join(CHECKSUMS_FILE))?;
        for filename in REQUIRED_FILES {
            let checksum = sha256_file(files.model_root.join(filename))?;
            writeln!(output, "{checksum}  {filename}")?;
        }
        Ok(())
    }

    fn verify_checksums(&self, files: &ModelFiles) -> Result<bool, InferError> {
        let checksum_path = files.model_root.join(CHECKSUMS_FILE);
        if !checksum_path.exists() {
            return Ok(false);
        }

        let file = File::open(checksum_path)?;
        let mut seen = HashSet::new();

        for line in BufReader::new(file).lines() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let mut parts = line.split_whitespace();
            let Some(expected) = parts.next() else {
                return Ok(false);
            };
            let Some(filename) = parts.next() else {
                return Ok(false);
            };

            if !REQUIRED_FILES.contains(&filename) {
                continue;
            }

            let path = files.model_root.join(filename);
            if !path.exists() {
                return Ok(false);
            }

            let actual = sha256_file(&path)?;
            if actual != expected {
                tracing::warn!(
                    file = filename,
                    model_dir = %files.model_root.display(),
                    "checksum mismatch for Candle embedding model file"
                );
                return Ok(false);
            }

            seen.insert(filename.to_owned());
        }

        Ok(REQUIRED_FILES
            .iter()
            .all(|name| seen.contains(*name) && files.model_root.join(name).exists()))
    }

    fn load_model(files: &ModelFiles) -> Result<LoadedModel, InferError> {
        let config_json = fs::read_to_string(&files.config_path)?;
        let parsed_cfg: QwenConfig = serde_json::from_str(&config_json)?;
        if parsed_cfg.hidden_size != CANDLE_EMBEDDING_DIM {
            return Err(InferError::InvalidEmbeddingResponse(format!(
                "unexpected model hidden size {} (expected {})",
                parsed_cfg.hidden_size, CANDLE_EMBEDDING_DIM
            )));
        }
        let config: qwen2::Config = serde_json::from_str(&config_json)?;

        let tokenizer = Tokenizer::from_file(&files.tokenizer_path)
            .map_err(|err| InferError::Tokenizer(err.to_string()))?;

        let device = Device::Cpu;
        tracing::info!("Using CPU with F32 for Candle embeddings");
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(
                std::slice::from_ref(&files.weights_path),
                DType::F32,
                &device,
            )?
        };
        let model = qwen2::Model::new(&config, vb)?;

        Ok(LoadedModel {
            model: Mutex::new(model),
            tokenizer: Mutex::new(tokenizer),
            device,
        })
    }

    fn embed_texts_with_loaded(
        loaded: &LoadedModel,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, InferError> {
        let mut outputs = vec![vec![0.0; CANDLE_EMBEDDING_DIM]; texts.len()];

        let mut active = Vec::new();
        for (index, text) in texts.iter().enumerate() {
            if text.trim().is_empty() {
                continue;
            }
            active.push((index, text.as_str()));
        }

        if active.is_empty() {
            return Ok(outputs);
        }

        for chunk in active.chunks(BATCH_CHUNK_SIZE) {
            let batch_inputs = chunk.iter().map(|(_, text)| *text).collect::<Vec<_>>();
            let batch_outputs = Self::embed_chunk(loaded, &batch_inputs)?;

            for ((index, _), embedding) in chunk.iter().zip(batch_outputs.into_iter()) {
                outputs[*index] = embedding;
            }
        }

        Ok(outputs)
    }

    fn embed_chunk(loaded: &LoadedModel, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let (encodings, pad_id) = {
            let tokenizer = loaded
                .tokenizer
                .lock()
                .map_err(|_| InferError::LockPoisoned("candle tokenizer".to_owned()))?;
            let pad_id = tokenizer
                .get_padding()
                .map(|params| params.pad_id)
                .unwrap_or(0);
            let encodings = tokenizer
                .encode_batch(texts.to_vec(), true)
                .map_err(|err| InferError::Tokenizer(err.to_string()))?;
            (encodings, pad_id)
        };

        let max_len = encodings
            .iter()
            .map(|encoding| encoding.get_ids().len().min(MAX_TOKENS))
            .max()
            .unwrap_or(0);

        if max_len == 0 {
            return Ok(vec![vec![0.0; CANDLE_EMBEDDING_DIM]; texts.len()]);
        }

        let mut input_ids = Vec::with_capacity(texts.len() * max_len);
        let mut attention_masks = Vec::with_capacity(texts.len() * max_len);

        for encoding in &encodings {
            let ids = encoding.get_ids();
            let mask = encoding.get_attention_mask();
            let len = ids.len().min(MAX_TOKENS);

            input_ids.extend_from_slice(&ids[..len]);
            attention_masks.extend_from_slice(&mask[..len]);

            if len < max_len {
                input_ids.extend(std::iter::repeat_n(pad_id, max_len - len));
                attention_masks.extend(std::iter::repeat_n(0u32, max_len - len));
            }
        }

        let input_ids = Tensor::from_vec(input_ids, (texts.len(), max_len), &loaded.device)?;
        let attention_mask =
            Tensor::from_vec(attention_masks, (texts.len(), max_len), &loaded.device)?;

        let hidden_states = {
            let mut model = loaded
                .model
                .lock()
                .map_err(|_| InferError::LockPoisoned("candle model".to_owned()))?;
            model.clear_kv_cache();
            let output = model.forward(&input_ids, 0, Some(&attention_mask))?;
            model.clear_kv_cache();
            output
        };

        let pooled = mean_pool(&hidden_states, &attention_mask)?.to_dtype(DType::F32)?;
        let mut rows = pooled.to_vec2::<f32>()?;

        for row in &mut rows {
            if row.len() != CANDLE_EMBEDDING_DIM {
                return Err(InferError::InvalidEmbeddingResponse(format!(
                    "expected {} dimensions from candle model, got {}",
                    CANDLE_EMBEDDING_DIM,
                    row.len()
                )));
            }
            l2_normalize(row);
        }

        Ok(rows)
    }
}

#[async_trait]
impl EmbeddingProvider for CandleEmbeddingProvider {
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>, InferError> {
        if text.trim().is_empty() {
            return Ok(vec![0.0; CANDLE_EMBEDDING_DIM]);
        }

        let provider = self.clone();
        let input = vec![text.to_owned()];
        let mut output = tokio::task::spawn_blocking(move || {
            let loaded = Arc::clone(provider.ensure_loaded()?);
            Self::embed_texts_with_loaded(loaded.as_ref(), &input)
        })
        .await
        .map_err(|err| {
            InferError::ModelUnavailable(format!("candle embedding task failed: {err}"))
        })??;
        Ok(output
            .pop()
            .unwrap_or_else(|| vec![0.0; CANDLE_EMBEDDING_DIM]))
    }
}

fn mean_pool(last_hidden_state: &Tensor, attention_mask: &Tensor) -> Result<Tensor, InferError> {
    let (batch, seq, hidden) = last_hidden_state.dims3()?;
    let mask = attention_mask.to_dtype(last_hidden_state.dtype())?;
    let expanded_mask = mask.unsqueeze(2)?.broadcast_as((batch, seq, hidden))?;
    let summed = (last_hidden_state * expanded_mask)?.sum(1)?;
    let counts = mask.sum(1)?.unsqueeze(1)?.expand((batch, hidden))?;
    Ok(summed.broadcast_div(&counts)?)
}

fn l2_normalize(embedding: &mut [f32]) {
    let norm_sq = embedding
        .iter()
        .map(|value| value * value)
        .fold(0.0f32, |acc, value| acc + value);

    let norm = norm_sq.sqrt();
    if norm < 1e-8 {
        embedding.fill(0.0);
        return;
    }

    for value in embedding.iter_mut() {
        *value /= norm;
    }
}

fn sha256_file(path: impl AsRef<Path>) -> Result<String, InferError> {
    let bytes = fs::read(path)?;
    let digest = Sha256::digest(bytes);
    Ok(format!("{digest:x}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction_is_lazy() {
        let provider = CandleEmbeddingProvider::new(PathBuf::from(".aether/models"));
        assert!(provider.loaded_model.get().is_none());
    }

    #[test]
    fn embedding_dimension_is_1024() {
        assert_eq!(CANDLE_EMBEDDING_DIM, 1024);
    }

    #[test]
    fn l2_normalize_zero_vector_returns_zeros() {
        let mut embedding = vec![0.0f32, 0.0, 0.0, 0.0];
        l2_normalize(&mut embedding);

        assert!(embedding.iter().all(|value| *value == 0.0));
        assert!(embedding.iter().all(|value| !value.is_nan()));
    }

    #[test]
    fn l2_normalize_non_zero_vector_has_unit_length() {
        let mut embedding = vec![3.0f32, 4.0, 0.0];
        l2_normalize(&mut embedding);

        let norm = embedding
            .iter()
            .map(|value| value * value)
            .fold(0.0f32, |acc, value| acc + value)
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert!((embedding[0] - 0.6).abs() < 1e-6);
        assert!((embedding[1] - 0.8).abs() < 1e-6);
        assert_eq!(embedding[2], 0.0);
    }
}
