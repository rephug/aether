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
use tokenizers::{Encoding, Tokenizer};

use super::{RerankCandidate, RerankResult, RerankerProvider};
use crate::InferError;

pub const CANDLE_RERANKER_PROVIDER_NAME: &str = "candle";
pub const CANDLE_RERANKER_MODEL_REPO: &str = "Qwen/Qwen3-Reranker-0.6B";
pub const CANDLE_RERANKER_MODEL_NAME: &str = "qwen3-reranker-0.6b";
const MODEL_DIR_NAME: &str = "qwen3-reranker-0.6b";
const CHECKSUMS_FILE: &str = "checksums.txt";
const MAX_TOKENS: usize = 2048;
const BATCH_CHUNK_SIZE: usize = 4;
const REQUIRED_FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];

pub struct CandleRerankerProvider {
    model_dir: PathBuf,
    loaded_model: OnceLock<Arc<LoadedRerankerModel>>,
}

struct LoadedRerankerModel {
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

impl CandleRerankerProvider {
    pub fn new(model_dir: PathBuf) -> Self {
        Self {
            model_dir,
            loaded_model: OnceLock::new(),
        }
    }

    pub fn provider_name(&self) -> &'static str {
        CANDLE_RERANKER_PROVIDER_NAME
    }

    pub fn model_name(&self) -> &'static str {
        CANDLE_RERANKER_MODEL_NAME
    }

    pub fn ensure_model_downloaded(&self) -> Result<PathBuf, InferError> {
        let files = self.ensure_model_files()?;
        Ok(files.model_root)
    }

    fn ensure_loaded(&self) -> Result<&Arc<LoadedRerankerModel>, InferError> {
        if let Some(loaded) = self.loaded_model.get() {
            return Ok(loaded);
        }

        let files = self.ensure_model_files()?;
        let loaded = Arc::new(Self::load_model(&files)?);
        let _ = self.loaded_model.set(loaded);
        self.loaded_model.get().ok_or_else(|| {
            InferError::ModelUnavailable("failed to initialize candle reranker model".to_owned())
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
        let repo = Repo::new(CANDLE_RERANKER_MODEL_REPO.to_owned(), RepoType::Model);
        let api_repo = api.repo(repo);

        tracing::info!(
            model_repo = CANDLE_RERANKER_MODEL_REPO,
            model_dir = %files.model_root.display(),
            "downloading Candle reranker model files"
        );

        for filename in REQUIRED_FILES {
            let source_path = api_repo.get(filename).map_err(|err| {
                InferError::ModelUnavailable(format!(
                    "model not found at {} (failed downloading {} from {}: {}). Run `aetherd --download-models` with network access or switch to another reranker provider.",
                    files.model_root.display(),
                    filename,
                    CANDLE_RERANKER_MODEL_REPO,
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
                    "checksum mismatch for Candle reranker model file"
                );
                return Ok(false);
            }

            seen.insert(filename.to_owned());
        }

        Ok(REQUIRED_FILES
            .iter()
            .all(|name| seen.contains(*name) && files.model_root.join(name).exists()))
    }

    fn load_model(files: &ModelFiles) -> Result<LoadedRerankerModel, InferError> {
        let config_json = fs::read_to_string(&files.config_path)?;
        let parsed_cfg: QwenConfig = serde_json::from_str(&config_json)?;
        if parsed_cfg.hidden_size == 0 {
            return Err(InferError::InvalidResponse(
                "invalid Candle reranker model config: hidden_size=0".to_owned(),
            ));
        }
        let config: qwen2::Config = serde_json::from_str(&config_json)?;

        let tokenizer = Tokenizer::from_file(&files.tokenizer_path)
            .map_err(|err| InferError::Tokenizer(err.to_string()))?;

        let device = Device::Cpu;
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(
                std::slice::from_ref(&files.weights_path),
                DType::F16,
                &device,
            )?
        };
        let model = qwen2::Model::new(&config, vb)?;

        Ok(LoadedRerankerModel {
            model: Mutex::new(model),
            tokenizer: Mutex::new(tokenizer),
            device,
        })
    }

    fn rerank_sync_with_loaded(
        loaded: &LoadedRerankerModel,
        query: &str,
        candidates: &[RerankCandidate],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, InferError> {
        if candidates.is_empty() || top_n == 0 {
            return Ok(Vec::new());
        }

        if query.trim().is_empty() {
            let mut passthrough = candidates
                .iter()
                .enumerate()
                .map(|(original_rank, candidate)| RerankResult {
                    id: candidate.id.clone(),
                    score: 0.0,
                    original_rank,
                })
                .collect::<Vec<_>>();
            passthrough.truncate(top_n.min(candidates.len()));
            return Ok(passthrough);
        }

        let mut scored = Vec::with_capacity(candidates.len());

        for batch in candidates.chunks(BATCH_CHUNK_SIZE) {
            let docs = batch
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>();
            let scores = Self::score_chunk(loaded, query, &docs)?;
            for (candidate, score) in batch.iter().zip(scores.into_iter()) {
                scored.push((candidate.id.clone(), score));
            }
        }

        let mut reranked = scored
            .into_iter()
            .enumerate()
            .map(|(original_rank, (id, score))| RerankResult {
                id,
                score,
                original_rank,
            })
            .collect::<Vec<_>>();

        reranked.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.original_rank.cmp(&right.original_rank))
                .then_with(|| left.id.cmp(&right.id))
        });
        reranked.truncate(top_n.min(candidates.len()));

        Ok(reranked)
    }

    fn score_chunk(
        loaded: &LoadedRerankerModel,
        query: &str,
        documents: &[&str],
    ) -> Result<Vec<f32>, InferError> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let (encodings, pad_id) = {
            let tokenizer = loaded
                .tokenizer
                .lock()
                .map_err(|_| InferError::LockPoisoned("candle reranker tokenizer".to_owned()))?;
            let pad_id = tokenizer
                .get_padding()
                .map(|params| params.pad_id)
                .unwrap_or(0);

            let mut encodings = Vec::with_capacity(documents.len());
            for document in documents {
                let encoding = tokenizer
                    .encode((query, *document), true)
                    .map_err(|err| InferError::Tokenizer(err.to_string()))?;
                encodings.push(encoding);
            }

            (encodings, pad_id)
        };

        let max_len = encodings
            .iter()
            .map(|encoding| encoding.get_ids().len().min(MAX_TOKENS))
            .max()
            .unwrap_or(0);

        if max_len == 0 {
            return Ok(vec![0.0; documents.len()]);
        }

        let mut input_ids = Vec::with_capacity(documents.len() * max_len);
        let mut attention_masks = Vec::with_capacity(documents.len() * max_len);

        for encoding in &encodings {
            append_encoding(
                encoding,
                pad_id,
                max_len,
                &mut input_ids,
                &mut attention_masks,
            );
        }

        let input_ids = Tensor::from_vec(input_ids, (documents.len(), max_len), &loaded.device)?;
        let attention_mask =
            Tensor::from_vec(attention_masks, (documents.len(), max_len), &loaded.device)?;

        let hidden_states = {
            let mut model = loaded
                .model
                .lock()
                .map_err(|_| InferError::LockPoisoned("candle reranker model".to_owned()))?;
            model.clear_kv_cache();
            let output = model.forward(&input_ids, 0, Some(&attention_mask))?;
            model.clear_kv_cache();
            output
        };

        let pooled = mean_pool(&hidden_states, &attention_mask)?.to_dtype(DType::F32)?;
        let rows = pooled.to_vec2::<f32>()?;

        Ok(rows
            .into_iter()
            .map(|row| sigmoid(row_mean(&row)))
            .collect::<Vec<_>>())
    }
}

#[async_trait]
impl RerankerProvider for CandleRerankerProvider {
    async fn rerank(
        &self,
        query: &str,
        candidates: &[RerankCandidate],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, InferError> {
        let loaded = Arc::clone(self.ensure_loaded()?);
        let query = query.to_owned();
        let candidates = candidates.to_vec();

        tokio::task::spawn_blocking(move || {
            Self::rerank_sync_with_loaded(
                loaded.as_ref(),
                query.as_str(),
                candidates.as_slice(),
                top_n,
            )
        })
        .await
        .map_err(|err| {
            InferError::ModelUnavailable(format!("candle reranker task failed: {err}"))
        })?
    }

    fn provider_name(&self) -> &str {
        CANDLE_RERANKER_PROVIDER_NAME
    }
}

fn append_encoding(
    encoding: &Encoding,
    pad_id: u32,
    max_len: usize,
    input_ids: &mut Vec<u32>,
    attention_masks: &mut Vec<u32>,
) {
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

fn mean_pool(last_hidden_state: &Tensor, attention_mask: &Tensor) -> Result<Tensor, InferError> {
    let (batch, seq, hidden) = last_hidden_state.dims3()?;
    let mask = attention_mask.to_dtype(last_hidden_state.dtype())?;
    let expanded_mask = mask.unsqueeze(2)?.broadcast_as((batch, seq, hidden))?;
    let summed = (last_hidden_state * expanded_mask)?.sum(1)?;
    let counts = mask.sum(1)?.unsqueeze(1)?.expand((batch, hidden))?;
    Ok(summed.broadcast_div(&counts)?)
}

fn row_mean(row: &[f32]) -> f32 {
    if row.is_empty() {
        return 0.0;
    }

    let sum = row.iter().copied().fold(0.0f32, |acc, value| acc + value);
    sum / row.len() as f32
}

fn sigmoid(value: f32) -> f32 {
    let clamped = value.clamp(-30.0, 30.0);
    1.0 / (1.0 + (-clamped).exp())
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
        let provider = CandleRerankerProvider::new(PathBuf::from(".aether/models"));
        assert!(provider.loaded_model.get().is_none());
    }
}
