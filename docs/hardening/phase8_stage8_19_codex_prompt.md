# Codex Prompt — Phase 8.19: MCP Refactoring Intelligence

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Three changes in this stage. Read the spec and session context first:
- `docs/roadmap/phase_8_stage_8_19_mcp_refactoring_intelligence.md`
- `docs/hardening/phase8_stage8_19_session_context.md`

Then read these source files:
- `crates/aether-mcp/src/tools/router.rs` (tool registration pattern)
- `crates/aether-mcp/src/tools/sir.rs` (aether_dependencies_logic — the function to modify)
- `crates/aether-mcp/src/tools/impact.rs` (blast_radius — reference for complex query tool)
- `crates/aether-mcp/src/tools/search.rs` (search tool — modify for fallback)
- `crates/aether-mcp/src/tools/mod.rs` (module declarations)
- `crates/aether-mcp/src/state.rs` (SharedState — add validation)
- `crates/aether-mcp/src/lib.rs` (server startup)
- `crates/aether-store/src/graph.rs` (store_get_callers, store_get_dependencies)
- `crates/aether-store/src/symbols.rs` (symbol query methods)
- `crates/aether-store/src/lib.rs` (Store sub-traits — post-refactor)
- `crates/aether-core/src/lib.rs` (SymbolEdge, EdgeKind)
- `crates/aether-config/src/embeddings.rs` (embedding config fields)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add -b feature/phase8-stage8-19-mcp-refactoring-intelligence /home/rephu/aether-phase8-mcp-refactor-intel
cd /home/rephu/aether-phase8-mcp-refactor-intel
```

## SOURCE INSPECTION

Before writing code, verify these assumptions. If any is false, STOP and report:

1. `AetherMcpServer` in `router.rs` uses the `#[tool(...)]` attribute macro
   to register tools. Each tool method takes `Parameters<RequestType>` and
   returns `Result<Json<ResponseType>, McpError>`.

2. `aether_dependencies_logic` in `sir.rs` resolves a symbol_id, then
   queries `store.get_callers()` and `store.get_dependencies()` for that
   symbol's `qualified_name`. It does NOT check the symbol's `kind`.

3. `symbol_edges` table has columns: `source_id`, `target_qualified_name`,
   `edge_kind`, `file_path`. Primary key is `(source_id, target_qualified_name, edge_kind)`.

4. `SharedState` in `state.rs` holds `store: Arc<SqliteStore>`,
   `config: Arc<AetherConfig>`, and `vector_store: Option<Arc<dyn VectorStore>>`.

5. The search tool in `search.rs` already has a `fallback_reason` field in
   its response struct. Check what type it is and how it's currently set.

6. `store.list_symbols_for_file(file_path)` returns `Vec<SymbolRecord>`.
   `store.get_callers(qualified_name)` returns `Vec<SymbolEdge>`.

7. The embedding config has a field for the API key env var name. Find
   the exact field path: likely `config.embeddings.api_key_env` or similar.

## CHANGE 1: New `aether_usage_matrix` tool

Create `crates/aether-mcp/src/tools/usage_matrix.rs`.

### Request struct

```rust
pub struct AetherUsageMatrixRequest {
    /// Symbol name (e.g., "Store", "SqliteStore")
    pub symbol: String,
    /// Optional file path to disambiguate
    pub file: Option<String>,
    /// Optional kind filter (e.g., "trait", "struct")
    pub kind: Option<String>,
}
```

### Response struct

```rust
pub struct AetherUsageMatrixResponse {
    pub schema_version: &'static str,  // "1.0"
    pub target: String,
    pub target_file: String,
    pub method_count: u32,
    pub consumer_count: u32,
    pub matrix: Vec<ConsumerRow>,
    pub method_consumers: Vec<MethodConsumers>,
    pub uncalled_methods: Vec<String>,
    pub suggested_clusters: Vec<MethodCluster>,
}

pub struct ConsumerRow {
    pub consumer_file: String,
    pub methods_used: Vec<String>,
    pub method_count: u32,
}

pub struct MethodConsumers {
    pub method: String,
    pub consumer_files: Vec<String>,
    pub consumer_count: u32,
}

pub struct MethodCluster {
    pub cluster_name: String,
    pub methods: Vec<String>,
    pub shared_consumers: Vec<String>,
    pub reason: String,
}
```

### Logic

```
1. Resolve target symbol:
   - Query symbols table for records matching `symbol` name
   - Filter by `file` and `kind` if provided
   - If ambiguous, return error with candidates

2. Find child methods:
   - Query symbols WHERE qualified_name LIKE '{target_qualified_name}::%'
     AND file_path = target_file_path
     AND kind IN ('function', 'method')
   - Collect as Vec<(method_id, method_qualified_name, method_short_name)>
   - method_short_name = qualified_name after last "::"

3. For each method, find callers:
   - Query symbol_edges WHERE target_qualified_name = method_qualified_name
     AND edge_kind = 'calls'
   - For each edge, resolve source_id to file_path via symbols table
   - Build HashMap<method_short_name, HashSet<caller_file_path>>

4. Build matrix (consumer → methods):
   - Invert the map: HashMap<caller_file_path, Vec<method_short_name>>
   - Sort by method_count descending

5. Build method_consumers (method → consumers):
   - Direct from step 3's map
   - Sort by consumer_count descending

6. Find uncalled_methods:
   - Methods with zero callers in the edges

7. Compute suggested_clusters:
   - For each method, create a bitvector of its consumer files
   - Group methods with identical consumer bitvectors
   - For groups with >1 method, create a MethodCluster
   - cluster_name: derive from common consumer pattern or first method prefix
   - reason: "Always co-consumed by: {list of shared consumer files}"
```

### Registration

Add to `router.rs`:
```rust
#[tool(
    name = "aether_usage_matrix",
    description = "Get a consumer-by-method usage matrix for a trait or struct, showing which files call which methods and suggesting method clusters for trait decomposition"
)]
pub async fn aether_usage_matrix(
    &self,
    Parameters(request): Parameters<AetherUsageMatrixRequest>,
) -> Result<Json<AetherUsageMatrixResponse>, McpError> {
```

Add `pub mod usage_matrix;` to `tools/mod.rs`.

## CHANGE 2: Type-level aggregation in `aether_dependencies`

Modify `aether_dependencies_logic` in `crates/aether-mcp/src/tools/sir.rs`.

After resolving the symbol, check its `kind`. If `kind` is `struct`, `trait`,
`enum`, or `type_alias`:

1. Find child methods (same logic as usage_matrix step 2)
2. For each child method, query callers and dependencies
3. Aggregate: deduplicate callers by qualified_name, count methods_called per caller
4. Aggregate: deduplicate dependencies by qualified_name, count referencing_methods
5. Set `aggregated: true` in response
6. Add `child_method_count` field to response

Add these fields to the response struct:
```rust
pub aggregated: bool,           // false for functions, true for types
pub child_method_count: u32,    // 0 for functions
```

For regular function/method symbols, behavior is UNCHANGED. `aggregated`
is false, `child_method_count` is 0.

## CHANGE 3: Graceful search fallback

### state.rs

Add to `SharedState`:
```rust
pub semantic_search_available: bool,
```

In both `open_readwrite` and `open_readonly`, after loading config:

```rust
let semantic_search_available = if config.embeddings.enabled {
    // Check if the required API key env var is set
    let key_env = &config.embeddings.api_key_env;
    if key_env.is_empty() || std::env::var(key_env).is_ok() {
        true
    } else {
        tracing::warn!(
            "Embedding provider requires {} but it is not set. \
             Semantic search will be unavailable. Register the MCP \
             server with --env {}=<value> to enable it.",
            key_env, key_env
        );
        false
    }
} else {
    false
};
```

### search.rs

In the search logic, before attempting semantic or hybrid search, check
`self.state.semantic_search_available`. If false and the requested mode
is `hybrid` or `semantic`:

- Fall back to lexical mode
- Set `fallback_reason` to `Some("Embedding API key not configured. Register MCP server with --env <KEY>=<value> to enable semantic search.")`
- Do NOT return an error

## TESTS

Add to `crates/aether-mcp/tests/mcp_tools.rs`:

1. **usage_matrix test:** Create a workspace with 3+ files, each containing
   functions that call methods on a common struct. Run `aether_usage_matrix`
   and verify matrix dimensions, consumer counts, and that uncalled_methods
   is correct.

2. **aggregated dependencies test:** Call `aether_dependencies` on a struct
   symbol. Verify `aggregated: true`, `child_method_count > 0`, and that
   callers/dependencies include entries from child methods.

3. **aggregated dependencies regression:** Call `aether_dependencies` on a
   regular function symbol. Verify `aggregated: false`, `child_method_count: 0`,
   and that callers/dependencies match the non-aggregated behavior.

4. **search fallback test:** Unset the embedding API key env var, construct
   SharedState, verify `semantic_search_available: false`. Call search with
   hybrid mode, verify fallback to lexical with `fallback_reason` set.

## VALIDATION

```bash
cargo fmt --all --check
cargo clippy -p aether-mcp -- -D warnings
cargo test -p aether-mcp
```

Then full workspace check:
```bash
cargo clippy --workspace -- -D warnings
cargo test -p aetherd
cargo test -p aether-store
```

## COMMIT

```bash
git add -A
git commit -m "Add aether_usage_matrix tool, type-level dependency aggregation, and semantic search graceful fallback"
```

Do NOT push automatically. Report commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin feature/phase8-stage8-19-mcp-refactoring-intelligence
```
