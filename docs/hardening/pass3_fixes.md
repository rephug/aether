# AETHER Hardening Pass 3 — Codex Fix Prompt

You are working on the AETHER project at the repository root. This prompt contains
verified bug fixes from three independent code reviews. Each fix has been validated
against the actual source. Apply ALL fixes below, then run the validation gate.

**CRITICAL: Do NOT change any public API signatures, struct field names, or trait
method signatures. All fixes are internal implementation changes only.**

---

## Fix 1: CozoDB storage engine — sled → sqlite

**Decision Register v2 #12 specifies sqlite.** Sled uses exclusive filesystem locks
that prevent concurrent daemon + MCP access. Both the Cargo feature and the runtime
init string must change.

### File: `Cargo.toml` (workspace root)

Change the `cozo` dependency feature from `storage-sled` to `storage-sqlite`:

```toml
# BEFORE:
cozo = { version = "0.7", default-features = false, features = ["storage-sled", "graph-algo"] }

# AFTER:
cozo = { version = "0.7", default-features = false, features = ["storage-sqlite", "graph-algo"] }
```

### File: `crates/aether-store/src/graph_cozo.rs`

At line 26, change the storage engine string from `"sled"` to `"sqlite"`:

```rust
// BEFORE:
let db = DbInstance::new("sled", &graph_path_str, Default::default())

// AFTER:
let db = DbInstance::new("sqlite", &graph_path_str, Default::default())
```

---

## Fix 2: Candle reranker — proper template + yes/no logit scoring

**This is the highest-impact fix.** The current `score_chunk` function in the Candle
reranker has TWO bugs:

1. **Wrong scoring method:** Uses `mean_pool → row_mean → sigmoid` which produces
   ~0.5 for all inputs (effectively random). Qwen3-Reranker scores by comparing
   "yes" vs "no" token logits at the last token position.

2. **Missing chat template:** The model was trained with a specific chat template
   including system message, `<|im_start|>` tokens, instruction tags, and thinking
   block suffix. Without this template, the model receives wrong token patterns and
   produces meaningless hidden states. The current code uses bare sentence-pair
   encoding `tokenizer.encode((query, document), true)` which is incorrect.

**Why we keep Qwen3-Reranker instead of swapping to a BERT cross-encoder:**
Qwen3-Reranker supports custom instructions, is benchmarked on MTEB-Code (code
retrieval), and understands semantic code relationships. A web-passage-trained
MiniLM cross-encoder would be simpler but significantly worse for code search.
The chat template is a fixed format string (~10 lines), not complex logic.

**Key insight:** `candle_transformers::models::qwen2::Model::forward()` already
includes `lm_head` projection and returns **logits** of shape
`(batch_size, seq_len, vocab_size)`, NOT hidden states. The variable currently named
`hidden_states` in the code is actually the full logit tensor. No additional weight
loading is needed.

### File: `crates/aether-infer/src/reranker/candle.rs`

**Step 2a: Add reranker instruction constant and template function**

Add these constants and the template function AFTER the existing constants (after
line 27, after `const REQUIRED_FILES`):

```rust
/// Default instruction for code search reranking. Qwen3-Reranker supports custom
/// instructions that tell the model what "relevant" means for this domain.
const RERANKER_INSTRUCTION: &str =
    "Given a code search query, retrieve relevant code documentation that describes matching symbols, functions, or modules";

/// Format a query-document pair using the Qwen3-Reranker chat template.
/// The model was trained with this exact template format and produces
/// meaningless scores without it.
///
/// Template structure:
///   <|im_start|>system\n{judge prompt}<|im_end|>
///   <|im_start|>user\n<Instruct>: {instruction}\n<Query>: {query}\n<Document>: {doc}<|im_end|>
///   <|im_start|>assistant\n<think>\n\n</think>\n\n
fn format_reranker_input(query: &str, document: &str) -> String {
    format!(
        "<|im_start|>system\nJudge whether the Document meets the requirements based on the Query and the Instruct provided. Note that the answer can only be \"yes\" or \"no\".<|im_end|>\n<|im_start|>user\n<Instruct>: {}\n<Query>: {}\n<Document>: {}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n",
        RERANKER_INSTRUCTION, query, document
    )
}
```

**Step 2b: Add yes/no token IDs to `LoadedRerankerModel`**

Replace the `LoadedRerankerModel` struct definition (around line 35):

```rust
// BEFORE:
struct LoadedRerankerModel {
    model: Mutex<qwen2::Model>,
    tokenizer: Mutex<Tokenizer>,
    device: Device,
}

// AFTER:
struct LoadedRerankerModel {
    model: Mutex<qwen2::Model>,
    tokenizer: Mutex<Tokenizer>,
    device: Device,
    yes_token_id: u32,
    no_token_id: u32,
}
```

**Step 2c: Resolve yes/no token IDs in `load_model`**

In the `load_model` function (around line 225), after the tokenizer is loaded and
before the `Ok(LoadedRerankerModel { ... })` return, resolve the token IDs:

```rust
// Add after tokenizer creation, before model loading:
let yes_token_id = tokenizer
    .token_to_id("yes")
    .ok_or_else(|| InferError::Tokenizer("tokenizer missing 'yes' token".to_owned()))?;
let no_token_id = tokenizer
    .token_to_id("no")
    .ok_or_else(|| InferError::Tokenizer("tokenizer missing 'no' token".to_owned()))?;
```

And add those fields to the returned struct:

```rust
Ok(LoadedRerankerModel {
    model: Mutex::new(model),
    tokenizer: Mutex::new(tokenizer),
    device,
    yes_token_id,
    no_token_id,
})
```

**Step 2d: Rewrite `score_chunk` with template tokenization + yes/no logit scoring**

Replace the entire `score_chunk` function. The two critical changes vs the old code:
1. **Tokenization:** Each query-document pair is formatted through
   `format_reranker_input()` and then tokenized as a single string (NOT as a
   sentence pair). Use `tokenizer.encode(formatted_string, false)` — `false`
   because the template already includes all special tokens.
2. **Scoring:** Extract logits at the last token position, index into yes/no
   token IDs, and compute `softmax([no_logit, yes_logit])[1]`.

```rust
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
            // Format with the full chat template — the model was trained with
            // this exact format and requires it for meaningful scores.
            let formatted = format_reranker_input(query, document);
            let encoding = tokenizer
                .encode(formatted.as_str(), false)
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
    // Track the actual (non-padded) length of each sequence so we can find the
    // last real token position for logit extraction.
    let mut seq_lengths: Vec<usize> = Vec::with_capacity(documents.len());

    for encoding in &encodings {
        let len = encoding.get_ids().len().min(MAX_TOKENS);
        seq_lengths.push(len);
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

    // model.forward() returns LOGITS of shape (batch, seq_len, vocab_size).
    // The lm_head projection is already applied inside qwen2::Model.
    let logits = {
        let mut model = loaded
            .model
            .lock()
            .map_err(|_| InferError::LockPoisoned("candle reranker model".to_owned()))?;
        model.clear_kv_cache();
        let output = model.forward(&input_ids, 0, Some(&attention_mask))?;
        model.clear_kv_cache();
        output
    };

    // For each sequence: extract the logit vector at the last non-padded token,
    // then compute softmax([no_logit, yes_logit])[1] as the relevance score.
    // This is the official Qwen3-Reranker scoring method.
    let logits_f32 = logits.to_dtype(DType::F32)?;
    let mut scores = Vec::with_capacity(documents.len());

    for (batch_idx, &seq_len) in seq_lengths.iter().enumerate() {
        let last_pos = if seq_len > 0 { seq_len - 1 } else { 0 };
        // logits shape: (batch, seq_len, vocab_size)
        let token_logits = logits_f32
            .i((batch_idx, last_pos))?
            .to_vec1::<f32>()?;

        let yes_logit = *token_logits
            .get(loaded.yes_token_id as usize)
            .unwrap_or(&-10.0);
        let no_logit = *token_logits
            .get(loaded.no_token_id as usize)
            .unwrap_or(&-10.0);

        // softmax([no_logit, yes_logit])[1] = P(yes | query, document)
        let max_val = yes_logit.max(no_logit);
        let exp_yes = (yes_logit - max_val).exp();
        let exp_no = (no_logit - max_val).exp();
        let score = exp_yes / (exp_yes + exp_no);

        scores.push(score);
    }

    Ok(scores)
}
```

**Step 2e: Remove dead helper functions**

Delete the `mean_pool`, `row_mean`, and `sigmoid` functions entirely. They are no
longer called by any code path.

```rust
// DELETE these three functions completely:
// fn mean_pool(...)
// fn row_mean(...)
// fn sigmoid(...)
```

If any other code in the file references these functions, the compiler will flag it.
There should be no other callers.

---

## Fix 3: Search rerank window — pass wider limit to retrieval

The `fuse_limit` (rerank window, e.g. 50) is calculated AFTER `lexical_search` and
`semantic_search` are called with `limit` (e.g. 10). This means the reranker only
ever sees ≤20 candidates, defeating the wider window.

### File: `crates/aetherd/src/search.rs`

In the `execute_search` function, the `SearchMode::Hybrid` branch needs to compute
the retrieval limit before calling the search functions.

Find the Hybrid match arm (around line 138). Currently the code is:

```rust
SearchMode::Hybrid => {
    let (semantic_matches, fallback_reason) = semantic_search(
        workspace,
        &store,
        &normalized_query,
        language_hint.as_deref(),
        limit,                              // <-- problem: uses user limit
        store_present,
        &config,
    )?;
```

Change the Hybrid branch to compute `retrieval_limit` first, use it for both search
calls, and keep `limit` for final truncation:

```rust
SearchMode::Hybrid => {
    let search_config = config.search.clone();
    let retrieval_limit = if matches!(search_config.reranker, SearchRerankerKind::None) {
        limit
    } else {
        search_config.rerank_window.max(limit).clamp(1, 200)
    };

    let (semantic_matches, fallback_reason) = semantic_search(
        workspace,
        &store,
        &normalized_query,
        language_hint.as_deref(),
        retrieval_limit,                    // <-- wider window
        store_present,
        &config,
    )?;
    if semantic_matches.is_empty() {
        return Ok(SearchExecution {
            mode_requested: SearchMode::Hybrid,
            mode_used: SearchMode::Lexical,
            fallback_reason,
            matches: lexical_matches,
        });
    }

    let fuse_limit = retrieval_limit;
    let fused = fuse_hybrid_results(&lexical_matches, &semantic_matches, fuse_limit);
    let matches = maybe_rerank_hybrid_results(
        workspace,
        &store,
        &normalized_query,
        limit,                              // <-- final truncation limit stays
        fused,
        search_config.reranker,
        search_config.rerank_window,
    )?;

    Ok(SearchExecution {
        mode_requested: SearchMode::Hybrid,
        mode_used: SearchMode::Hybrid,
        fallback_reason: None,
        matches,
    })
}
```

Also update the `lexical_search` call that happens BEFORE the match statement
(around line 103). Currently it uses `limit`:

```rust
let lexical_matches = lexical_search(&store, &normalized_query, limit)?;
```

This needs to use the wider limit too, but only for Hybrid mode. The cleanest
approach: move the initial lexical_search inside each match arm, OR compute the
retrieval_limit before the match and use it. Here is the approach that preserves the
existing structure — compute retrieval_limit early:

```rust
let limit = limit.clamp(1, 100);
let (normalized_query, language_hint) = extract_language_hint_from_query(query);

let search_config = config.search.clone();
let retrieval_limit = if mode == SearchMode::Hybrid
    && !matches!(search_config.reranker, SearchRerankerKind::None)
{
    search_config.rerank_window.max(limit).clamp(1, 200)
} else {
    limit
};

let lexical_matches = lexical_search(&store, &normalized_query, retrieval_limit)?;
```

Note: `SearchMode` must derive `PartialEq` for the `==` comparison. If it doesn't
already, add `#[derive(PartialEq)]` to the enum. If that causes issues, use
`matches!(mode, SearchMode::Hybrid)` instead.

Remove the duplicate `let search_config = config.search.clone();` inside the Hybrid
arm since it was moved up.

---

## Fix 4: Cohere API key — wrap in Secret

### File: `crates/aether-infer/src/reranker/cohere.rs`

Change `api_key` from raw `String` to `aether_core::Secret`:

```rust
// BEFORE (line 14):
api_key: String,

// AFTER:
api_key: aether_core::Secret,
```

Update the constructor `from_env` (around line 20):

```rust
// BEFORE:
let api_key = std::env::var(api_key_env)
    .ok()
    .map(|value| value.trim().to_owned())
    .filter(|value| !value.is_empty())
    .ok_or_else(|| InferError::MissingCohereApiKey(api_key_env.to_owned()))?;

// AFTER:
let api_key = std::env::var(api_key_env)
    .ok()
    .map(|value| value.trim().to_owned())
    .filter(|value| !value.is_empty())
    .map(aether_core::Secret::new)
    .ok_or_else(|| InferError::MissingCohereApiKey(api_key_env.to_owned()))?;
```

Update the bearer_auth call (line 64):

```rust
// BEFORE:
.bearer_auth(&self.api_key)

// AFTER:
.bearer_auth(self.api_key.expose())
```

---

## Fix 5: Git rename path parsing — proper brace-enclosed rename support

### File: `crates/aether-analysis/src/coupling.rs`

The `normalize_rename_path` function (around line 797) fails on brace-enclosed renames
like `crates/{old => new}/src/lib.rs`. The current logic uses `rsplit_once("=>")` which
gives ` new}/src/lib.rs` on the right side, then tries `trim_start_matches('{')` and
`trim_end_matches('}')` — but the `}` isn't at the string's end and the prefix `crates/`
is lost entirely. Result: `new}/src/lib.rs` (broken path with stray brace, missing prefix).

**Why not use `--no-renames`:** Adding `--no-renames` to the git command would bypass the
parsing bug, but at the cost of coupling analysis quality — git would report renames as
full delete + full add (inflating change magnitude weights), dead paths would appear as
coupling targets, and the semantic signal "this was a rename" is lost for temporal analysis.

**Fix:** Rewrite `normalize_rename_path` to handle both rename formats:
- Brace-enclosed: `prefix{old => new}suffix` → `prefix + new + suffix`
- Simple: `old_path => new_path` → `new_path`

```rust
// BEFORE (around line 797):
fn normalize_rename_path(path: &str) -> String {
    let value = path.trim();
    if let Some((_, right)) = value.rsplit_once("=>") {
        return right
            .trim()
            .trim_start_matches('{')
            .trim_end_matches('}')
            .trim()
            .to_owned();
    }

    value.to_owned()
}

// AFTER:
fn normalize_rename_path(path: &str) -> String {
    let value = path.trim();

    // Handle brace-enclosed renames: prefix{old => new}suffix
    // Example: crates/{old => new}/src/lib.rs → crates/new/src/lib.rs
    if let (Some(brace_start), Some(brace_end)) = (value.find('{'), value.find('}')) {
        if brace_start < brace_end {
            let prefix = &value[..brace_start];
            let inner = &value[brace_start + 1..brace_end];
            let suffix = &value[brace_end + 1..];

            if let Some((_, new_part)) = inner.split_once("=>") {
                return format!("{}{}{}", prefix, new_part.trim(), suffix);
            }
        }
    }

    // Handle simple renames: old_path => new_path
    if let Some((_, right)) = value.rsplit_once("=>") {
        return right.trim().to_owned();
    }

    value.to_owned()
}
```

Also add tests for both rename formats in the test module at the bottom of the file:

```rust
#[test]
fn normalize_rename_path_brace_enclosed() {
    assert_eq!(
        normalize_rename_path("crates/{old => new}/src/lib.rs"),
        "crates/new/src/lib.rs"
    );
}

#[test]
fn normalize_rename_path_brace_enclosed_no_prefix() {
    assert_eq!(
        normalize_rename_path("{old_crate => new_crate}/src/main.rs"),
        "new_crate/src/main.rs"
    );
}

#[test]
fn normalize_rename_path_simple() {
    assert_eq!(
        normalize_rename_path("old/path.rs => new/path.rs"),
        "new/path.rs"
    );
}

#[test]
fn normalize_rename_path_no_rename() {
    assert_eq!(
        normalize_rename_path("src/lib.rs"),
        "src/lib.rs"
    );
}
```

---

## Fix 6: Python import parsing — strip parentheses

### File: `crates/aether-parse/src/languages/python.rs`

In `parse_import_from_statement` (around line 431), `from module import (a, b)` produces
malformed edges `module.(a` and `module.b)` because parentheses are never stripped.

After the line `let names = names_raw.trim();` and before the `if names == "*"` check,
add parentheses stripping:

```rust
let module = resolve_import_module(module_raw.trim(), file_path);
let names = names_raw.trim();

// Strip surrounding parentheses from multi-line imports:
// from module import (a, b) → a, b
let names = names
    .strip_prefix('(')
    .and_then(|s| s.strip_suffix(')'))
    .unwrap_or(names)
    .trim();

if names == "*" {
```

Also add a test for this case in the test module at the bottom of the file:

```rust
#[test]
fn parenthesized_import() {
    let edges = parse_import_from_statement("from module import (a, b)", "src/main.py");
    assert_eq!(edges, vec!["module.a".to_owned(), "module.b".to_owned()]);
}

#[test]
fn parenthesized_import_with_whitespace() {
    let edges = parse_import_from_statement("from module import (\n    a,\n    b,\n)", "src/main.py");
    assert_eq!(edges, vec!["module.a".to_owned(), "module.b".to_owned()]);
}
```

---

## Fix 7: VS Code extension — inference model dropdown

### File: `vscode-extension/package.json`

The `aether.inferenceModel` enum lists embedding models instead of instruct models.
Replace the enum values:

```json
// BEFORE:
"aether.inferenceModel": {
    "type": "string",
    "enum": [
        "qwen3-embeddings-0.6B",
        "qwen3-embeddings-4B",
        "qwen3-embeddings-8B",
        "gemini-2.0-flash"
    ],
    "default": "qwen3-embeddings-0.6B",
    "description": "Model name passed to the selected provider."
},

// AFTER:
"aether.inferenceModel": {
    "type": "string",
    "enum": [
        "qwen2.5-coder:7b-instruct-q4_K_M",
        "qwen2.5-coder:1.5b-instruct-q4_K_M",
        "gemini-2.0-flash"
    ],
    "default": "gemini-2.0-flash",
    "description": "Model name passed to the selected provider for SIR generation."
},
```

---

## Fix 8: CI test matrix — add missing crates

### File: `.github/workflows/ci.yml`

In the `test` job matrix, add the 4 missing crates:

```yaml
# BEFORE:
matrix:
    crate:
        - aether-core
        - aether-config
        - aether-store
        - aether-memory
        - aether-analysis
        - aether-mcp
        - aetherd

# AFTER:
matrix:
    crate:
        - aether-core
        - aether-config
        - aether-store
        - aether-memory
        - aether-analysis
        - aether-mcp
        - aether-infer
        - aether-parse
        - aether-sir
        - aether-lsp
        - aetherd
```

---

## Fix 9: Arrow workspace version cleanup

### File: `Cargo.toml` (workspace root)

The workspace declares `arrow-array = "54"` and `arrow-schema = "54"` but no crate
uses `workspace = true` for these. `aether-store` declares its own `"56.2"`. Clean up
by updating the workspace versions to match what's actually used:

```toml
# BEFORE:
arrow-array = "54"
arrow-schema = "54"

# AFTER:
arrow-array = "56.2"
arrow-schema = "56.2"
```

### File: `crates/aether-store/Cargo.toml`

Switch to using the workspace versions:

```toml
# BEFORE:
arrow-array = "56.2"
arrow-schema = "56.2"

# AFTER:
arrow-array.workspace = true
arrow-schema.workspace = true
```

---

## Fix 10: LanceDB pushdown filter for list_embeddings_for_symbols

### File: `crates/aether-store/src/vector.rs`

In `list_embeddings_for_symbols` (the `LanceVectorStore` impl, around line 686), the
current code does a full table scan and filters in Rust. Add an `only_if` predicate
to push the filter down to LanceDB.

**Important:** Large `IN (...)` clauses can exceed DataFusion's SQL parser limits.
Currently `symbol_set` is bounded by search results (~200 via `rerank_window`), but
the fix must be robust against future callers passing larger sets. Chunk into batches
of 500 IDs and collect results across chunks.

Add a helper function near the top of the impl block (or as a free function):

```rust
/// Maximum IDs per IN clause to stay within DataFusion parser limits.
const PUSHDOWN_CHUNK_SIZE: usize = 500;

/// Build a SQL predicate: `symbol_id IN ('id1', 'id2', ...)`
fn build_in_predicate(ids: &[&str]) -> String {
    let values = ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    format!("symbol_id IN ({})", values)
}
```

Find the query execution block inside the for loop over table names (around line 728):

```rust
// BEFORE:
let batches = table
    .query()
    .select(Select::columns(&[
        "symbol_id",
        "sir_hash",
        "provider",
        "model",
        "embedding",
        "updated_at",
    ]))
    .execute()
    .await
    .map_err(map_lancedb_err)?
    .try_collect::<Vec<_>>()
    .await
    .map_err(map_lancedb_err)?;

// AFTER:
// Chunk symbol IDs to avoid exceeding DataFusion's IN clause parser limits.
let symbol_ids: Vec<&str> = symbol_set.iter().map(|s| s.as_str()).collect();
let mut batches: Vec<RecordBatch> = Vec::new();
let select_cols = Select::columns(&[
    "symbol_id",
    "sir_hash",
    "provider",
    "model",
    "embedding",
    "updated_at",
]);

for chunk in symbol_ids.chunks(PUSHDOWN_CHUNK_SIZE) {
    let predicate = build_in_predicate(chunk);
    let chunk_batches = table
        .query()
        .select(select_cols.clone())
        .only_if(predicate.as_str())
        .execute()
        .await
        .map_err(map_lancedb_err)?
        .try_collect::<Vec<_>>()
        .await
        .map_err(map_lancedb_err)?;
    batches.extend(chunk_batches);
}
```

Note: If `Select` does not implement `Clone`, construct it inside the loop instead.

The inner loop `if !symbol_set.contains(symbol_id.as_str()) { continue; }` can
remain as a safety check but will no longer be the primary filter.

---

## Fix 11: Cache SymbolExtractor in LSP hover

### File: `crates/aether-lsp/src/lib.rs`

`SymbolExtractor::new()` is called on every hover request, recompiling tree-sitter
queries each time. Cache it using `std::sync::OnceLock`.

Add near the top of the file (after imports):

```rust
use std::sync::OnceLock;

static SYMBOL_EXTRACTOR: OnceLock<std::sync::Mutex<aether_parse::SymbolExtractor>> = OnceLock::new();

fn get_extractor() -> Result<std::sync::MutexGuard<'static, aether_parse::SymbolExtractor>, HoverResolveError> {
    let mutex = SYMBOL_EXTRACTOR.get_or_init(|| {
        std::sync::Mutex::new(
            aether_parse::SymbolExtractor::new()
                .expect("failed to initialize SymbolExtractor"),
        )
    });
    mutex
        .lock()
        .map_err(|_| HoverResolveError::Parse("SymbolExtractor lock poisoned".to_owned()))
}
```

Then in `resolve_hover_markdown_for_path` (around line 153), replace:

```rust
// BEFORE:
let mut extractor =
    SymbolExtractor::new().map_err(|err| HoverResolveError::Parse(err.to_string()))?;

// AFTER:
let mut extractor = get_extractor()?;
```

Note: `extractor` is now a `MutexGuard`, which implements `DerefMut`, so subsequent
calls like `extractor.extract_from_source(...)` will work without changes.

---

## Fix 12: SQL LIKE tag filter — exact match

### File: `crates/aether-store/src/lib.rs`

The tag search filter uses `LIKE '%"tag"%'` which causes substring matches (searching
for "arch" matches "architecture").

Find the tag filter SQL (around line 2058). Replace the LIKE pattern with json_each
for exact matching:

```sql
-- BEFORE:
AND LOWER(s.tags) LIKE LOWER(?)

-- AFTER:
AND EXISTS (
    SELECT 1 FROM json_each(s.tags) AS je
    WHERE LOWER(je.value) = LOWER(?)
)
```

And update the parameter binding — change from `format!("%\"{tag}\"%")` pattern to
just passing the tag value directly:

```rust
// BEFORE (the parameter binding for the LIKE):
params.push(format!("%\"{}\"%" , tag));

// AFTER:
params.push(tag.to_owned());
```

Search for the exact binding code near the SQL construction and adjust accordingly.
The `tags` column contains a JSON array like `["struct","public"]`, and `json_each`
will iterate the array elements for exact comparison.

---

## Validation Gate

After applying ALL fixes, run:

```bash
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test -p aether-core
cargo test -p aether-config
cargo test -p aether-store
cargo test -p aether-parse
cargo test -p aether-sir
cargo test -p aether-infer
cargo test -p aether-lsp
cargo test -p aether-analysis
cargo test -p aether-mcp
cargo test -p aetherd
```

All commands must pass with zero errors and zero warnings.

If `cargo clippy` warns about unused imports after deleting `mean_pool`, `row_mean`,
and `sigmoid`, remove those imports.

If there are test failures related to the reranker scoring changes (scores now produce
different values), update test expectations to match the new yes/no logit scoring
behavior. The scores will now range from 0.0 to 1.0 based on yes/no probability
rather than the previous ~0.5 uniform output.

---

## Summary of Changes

| # | File | Change | Lines |
|---|------|--------|-------|
| 1 | Cargo.toml | cozo storage-sled → storage-sqlite | ~1 |
| 1 | graph_cozo.rs | "sled" → "sqlite" | ~1 |
| 2 | reranker/candle.rs | chat template + yes/no logit scoring replaces mean_pool | ~80 |
| 3 | search.rs | retrieval_limit = rerank_window before search | ~15 |
| 4 | reranker/cohere.rs | api_key: String → Secret | ~5 |
| 5 | coupling.rs | proper brace-enclosed rename parsing + 4 tests | ~30 |
| 6 | python.rs | strip parentheses + 2 tests | ~15 |
| 7 | package.json | embedding models → instruct models | ~5 |
| 8 | ci.yml | add 4 missing crates to test matrix | ~4 |
| 9 | Cargo.toml + aether-store | arrow 54→56.2, use workspace = true | ~4 |
| 10 | vector.rs | chunked pushdown predicate for symbol_id filter | ~25 |
| 11 | aether-lsp/lib.rs | OnceLock cache for SymbolExtractor | ~15 |
| 12 | aether-store/lib.rs | LIKE → json_each exact tag match | ~5 |
| **Total** | **12 files** | | **~185 lines** |
