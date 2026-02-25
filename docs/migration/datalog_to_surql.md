# Datalog to SurrealQL Migration Oracle

This document maps every CozoDB `run_script(...)` query in
`crates/aether-store/src/graph_cozo.rs` to its SurrealDB 3.0 / SurrealQL equivalent used in
`crates/aether-store/src/graph_surreal.rs`.

Use this as:

- a parity checklist during migration
- a review aid for sorting / edge-case behavior
- a test oracle when comparing Cozo vs Surreal helper outputs

## Notes

- Cozo relation names:
  - `symbols`
  - `edges`
  - `co_change_edges`
  - `tested_by`
- Surreal tables / relations:
  - `symbol`
  - `depends_on` (relation table for graph edges)
  - `co_change`
  - `tested_by`
- Cozo graph algorithms (`community_detection_louvain`, `pagerank`, `strongly_connected_components`,
  `connected_components`) do not exist in SurrealDB. Surreal migration strategy is:
  - fetch edge rows via SurrealQL
  - run algorithm in Rust (petgraph + local logic)
  - preserve Cozo fallback sorting semantics

## Schema Creation (`ensure_schema` / `ensure_relation`)

### Cozo: create `symbols`

```cozo
:create symbols {
    symbol_id: String =>
    qualified_name: String,
    name: String,
    kind: String,
    file_path: String,
    language: String,
    signature_fingerprint: String,
    last_seen_at: Int
}
```

### SurrealQL

```sql
DEFINE TABLE IF NOT EXISTS symbol SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS symbol_id ON symbol TYPE string;
DEFINE FIELD IF NOT EXISTS qualified_name ON symbol TYPE string;
DEFINE FIELD IF NOT EXISTS name ON symbol TYPE string;
DEFINE FIELD IF NOT EXISTS kind ON symbol TYPE string;
DEFINE FIELD IF NOT EXISTS file_path ON symbol TYPE string;
DEFINE FIELD IF NOT EXISTS language ON symbol TYPE string;
DEFINE FIELD IF NOT EXISTS signature_fingerprint ON symbol TYPE string;
DEFINE FIELD IF NOT EXISTS last_seen_at ON symbol TYPE int;
DEFINE INDEX IF NOT EXISTS idx_symbol_symbol_id ON symbol FIELDS symbol_id UNIQUE;
```

### Cozo: create `edges`

```cozo
:create edges {
    source_id: String,
    target_id: String,
    edge_kind: String =>
    file_path: String
}
```

### SurrealQL

```sql
DEFINE TABLE IF NOT EXISTS depends_on SCHEMAFULL TYPE RELATION FROM symbol TO symbol;
DEFINE FIELD IF NOT EXISTS edge_kind ON depends_on TYPE string;
DEFINE FIELD IF NOT EXISTS file_path ON depends_on TYPE string;
DEFINE FIELD IF NOT EXISTS weight ON depends_on TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS in ON depends_on TYPE record<symbol> REFERENCE;
DEFINE FIELD IF NOT EXISTS out ON depends_on TYPE record<symbol> REFERENCE;
```

### Cozo: create `co_change_edges`

```cozo
:create co_change_edges {
    file_a: String,
    file_b: String =>
    co_change_count: Int,
    total_commits_a: Int,
    total_commits_b: Int,
    git_coupling: Float,
    static_signal: Float,
    semantic_signal: Float,
    fused_score: Float,
    coupling_type: String,
    last_co_change_commit: String,
    last_co_change_at: Int,
    mined_at: Int
}
```

### SurrealQL

```sql
DEFINE TABLE IF NOT EXISTS co_change SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS file_a ON co_change TYPE string;
DEFINE FIELD IF NOT EXISTS file_b ON co_change TYPE string;
DEFINE FIELD IF NOT EXISTS co_change_count ON co_change TYPE int;
DEFINE FIELD IF NOT EXISTS total_commits_a ON co_change TYPE int;
DEFINE FIELD IF NOT EXISTS total_commits_b ON co_change TYPE int;
DEFINE FIELD IF NOT EXISTS git_coupling ON co_change TYPE float;
DEFINE FIELD IF NOT EXISTS static_signal ON co_change TYPE float;
DEFINE FIELD IF NOT EXISTS semantic_signal ON co_change TYPE float;
DEFINE FIELD IF NOT EXISTS fused_score ON co_change TYPE float;
DEFINE FIELD IF NOT EXISTS coupling_type ON co_change TYPE string;
DEFINE FIELD IF NOT EXISTS last_co_change_commit ON co_change TYPE string;
DEFINE FIELD IF NOT EXISTS last_co_change_at ON co_change TYPE int;
DEFINE FIELD IF NOT EXISTS mined_at ON co_change TYPE int;
DEFINE INDEX IF NOT EXISTS idx_co_change_pair ON co_change FIELDS file_a, file_b UNIQUE;
```

### Cozo: create `tested_by`

```cozo
:create tested_by {
    target_file: String,
    test_file: String =>
    intent_count: Int,
    confidence: Float,
    inference_method: String
}
```

### SurrealQL

```sql
DEFINE TABLE IF NOT EXISTS tested_by SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS target_file ON tested_by TYPE string;
DEFINE FIELD IF NOT EXISTS test_file ON tested_by TYPE string;
DEFINE FIELD IF NOT EXISTS intent_count ON tested_by TYPE int;
DEFINE FIELD IF NOT EXISTS confidence ON tested_by TYPE float;
DEFINE FIELD IF NOT EXISTS inference_method ON tested_by TYPE string;
DEFINE INDEX IF NOT EXISTS idx_tested_by_pair ON tested_by FIELDS target_file, test_file UNIQUE;
```

## Internal Edge Snapshot Query

### `list_dependency_edges_raw()`

### Cozo

```cozo
?[source_id, target_id, edge_kind] :=
    *edges{source_id, target_id, edge_kind, file_path}
```

### SurrealQL

```sql
SELECT VALUE {
  source_id: source.symbol_id,
  target_id: target.symbol_id,
  edge_kind: edge_kind
}
FROM depends_on
LET source = out, target = in
WHERE source != NONE AND target != NONE;
```

Parity notes:

- Surreal side filters to `calls` / `depends_on` in Rust, matching Cozo post-filter behavior.
- Output sorted in Rust by `(source_id, target_id, edge_kind)` for deterministic algorithms.

## Migration Export Queries

### `list_all_symbols_for_migration()`

### Cozo

```cozo
?[symbol_id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at] :=
    *symbols{symbol_id, qualified_name, name, kind, file_path, language, signature_fingerprint, last_seen_at}
:order symbol_id
```

### SurrealQL import target

```sql
UPSERT symbol SET
  symbol_id = $symbol_id,
  qualified_name = $qualified_name,
  name = $name,
  kind = $kind,
  file_path = $file_path,
  language = $language,
  signature_fingerprint = $signature_fingerprint,
  last_seen_at = $last_seen_at,
  updated_at = time::now()
WHERE symbol_id = $symbol_id;
```

Parity notes:

- Cozo export sorts by `symbol_id`; migration code also sorts exported `SymbolRecord`s by `id`.

### `list_all_edges_for_migration()`

### Cozo

```cozo
?[source_id, target_id, edge_kind, file_path] :=
    *edges{source_id, target_id, edge_kind, file_path}
:order source_id, target_id, edge_kind, file_path
```

### SurrealQL import target

```sql
LET $src = (SELECT id FROM symbol WHERE symbol_id = $source_id LIMIT 1);
LET $dst = (SELECT id FROM symbol WHERE symbol_id = $target_id LIMIT 1);
DELETE depends_on
WHERE out = $src[0].id AND in = $dst[0].id AND file_path = $file_path AND edge_kind = $edge_kind;
RELATE $src[0].id->depends_on->$dst[0].id
SET edge_kind = $edge_kind, file_path = $file_path, weight = 1.0;
```

Parity notes:

- Unknown edge kinds are skipped in both implementations.

## Cozo Built-In Graph Algorithm Queries (Replaced by Rust + petgraph)

These four Cozo queries are now mapped to `SELECT` edge snapshots + Rust computation.

### `try_louvain_from_cozo()`

### Cozo

```cozo
dep_edges[source, target] := *edges{source_id: source, target_id: target, edge_kind}, edge_kind = "calls"
dep_edges[source, target] := *edges{source_id: source, target_id: target, edge_kind}, edge_kind = "depends_on"
?[node, community] := community_detection_louvain(*dep_edges[], node, community)
:order node
```

### SurrealQL edge fetch + Rust

```sql
SELECT VALUE {
  source_id: source.symbol_id,
  target_id: target.symbol_id,
  edge_kind: edge_kind
}
FROM depends_on
LET source = out, target = in
WHERE edge_kind INSIDE ["calls", "depends_on"] AND source != NONE AND target != NONE;
```

Rust parity:

- Run Louvain-style modularity optimization in Rust (petgraph-backed).
- Final `list_louvain_communities()` sorts by `symbol_id` ascending to match Cozo wrapper behavior.
- Cozo fallback behavior (on built-in failure) also returned deterministic connected-component-based IDs; Surreal preserves deterministic ordering.

### `try_pagerank_from_cozo()`

### Cozo

```cozo
dep_edges[source, target] := *edges{source_id: source, target_id: target, edge_kind}, edge_kind = "calls"
dep_edges[source, target] := *edges{source_id: source, target_id: target, edge_kind}, edge_kind = "depends_on"
?[node, rank] := pagerank(*dep_edges[], node, rank)
:order node
```

### SurrealQL edge fetch + Rust

Same edge fetch as Louvain, then iterative PageRank in Rust.

Parity notes:

- Cozo wrapper sorted by rank desc, then node id asc.
- Surreal wrapper applies the same ordering after computation.

### `try_scc_from_cozo()`

### Cozo

```cozo
dep_edges[source, target] := *edges{source_id: source, target_id: target, edge_kind}, edge_kind = "calls"
dep_edges[source, target] := *edges{source_id: source, target_id: target, edge_kind}, edge_kind = "depends_on"
?[node, component] := strongly_connected_components(*dep_edges[], node, component)
:order component, node
```

### SurrealQL edge fetch + Rust

Same edge fetch as Louvain, then `petgraph::algo::kosaraju_scc`.

Parity notes:

- Components sorted internally ascending.
- Component list sorted by size descending, then lexical member list.

### `try_connected_components_from_cozo()`

### Cozo

```cozo
dep_edges[source, target] := *edges{source_id: source, target_id: target, edge_kind}, edge_kind = "calls"
dep_edges[source, target] := *edges{source_id: source, target_id: target, edge_kind}, edge_kind = "depends_on"
?[node, component] := connected_components(*dep_edges[], node, component)
:order component, node
```

### SurrealQL edge fetch + Rust

Same edge fetch as Louvain, then undirected component labeling in Rust (petgraph union-find).

Parity notes:

- Components sorted internally ascending.
- Component list sorted by size descending, then lexical member list.

## Public Helper Queries (Non-Trait)

### `has_dependency_between_files(file_a, file_b)`

### Cozo

```cozo
?[source_id] :=
    *edges{source_id, target_id},
    *symbols{symbol_id: source_id, file_path: $file_a},
    *symbols{symbol_id: target_id, file_path: $file_b}
?[source_id] :=
    *edges{source_id, target_id},
    *symbols{symbol_id: source_id, file_path: $file_b},
    *symbols{symbol_id: target_id, file_path: $file_a}
:limit 1
```

### SurrealQL

```sql
SELECT VALUE {
  source_file: source.file_path,
  target_file: target.file_path
}
FROM depends_on
LET source = out, target = in
WHERE source != NONE AND target != NONE;
```

Parity notes:

- Surreal implementation performs the file-pair check in Rust and returns on any matching row.

### `list_upstream_dependency_traversal(target_symbol_id, max_depth)`

### Cozo

```cozo
dep_edges[source, target] := *edges{source_id: source, target_id: target, edge_kind: "calls"}
dep_edges[source, target] := *edges{source_id: source, target_id: target, edge_kind: "depends_on"}

reachable[source, target, depth] := dep_edges[$start, target], source = $start, depth = 1
reachable[source, target, depth] :=
    reachable[_, mid, prev_depth],
    prev_depth < $max_depth,
    dep_edges[mid, target],
    source = mid,
    depth = prev_depth + 1

?[source_id, target_id, depth] := reachable[source_id, target_id, depth]
:order depth, source_id, target_id
```

### SurrealQL + Rust

```sql
SELECT VALUE {
  source_id: source.symbol_id,
  target_id: target.symbol_id,
  edge_kind: edge_kind
}
FROM depends_on
LET source = out, target = in
WHERE edge_kind INSIDE ["calls", "depends_on"] AND source != NONE AND target != NONE;
```

Rust parity:

- BFS-style expansion from `target_symbol_id` up to `max_depth`
- dedupe edges on `(source_id, target_id, depth)`
- `nodes` sorted by `(depth, symbol_id)`
- `edges` sorted by `(depth, source_id, target_id)`

### `upsert_co_change_edges(records)`

### Cozo (per record)

```cozo
?[file_a, file_b, ...] <- [[$file_a, $file_b, ...]]
:put co_change_edges { file_a, file_b => ... }
```

### SurrealQL (per record)

```sql
DELETE co_change WHERE file_a = $file_a AND file_b = $file_b;
CREATE co_change SET
  file_a = $file_a,
  file_b = $file_b,
  co_change_count = $co_change_count,
  total_commits_a = $total_commits_a,
  total_commits_b = $total_commits_b,
  git_coupling = $git_coupling,
  static_signal = $static_signal,
  semantic_signal = $semantic_signal,
  fused_score = $fused_score,
  coupling_type = $coupling_type,
  last_co_change_commit = $last_co_change_commit,
  last_co_change_at = $last_co_change_at,
  mined_at = $mined_at;
```

Parity notes:

- Replaces row by `(file_a, file_b)` pair.

### `get_co_change_edge(file_a, file_b)`

### Cozo

```cozo
?[file_a, file_b, ...] :=
    *co_change_edges{file_a, file_b, ...},
    file_a = $file_a,
    file_b = $file_b
:limit 1
```

### SurrealQL

```sql
SELECT VALUE {
  file_a: file_a,
  file_b: file_b,
  co_change_count: co_change_count,
  total_commits_a: total_commits_a,
  total_commits_b: total_commits_b,
  git_coupling: git_coupling,
  static_signal: static_signal,
  semantic_signal: semantic_signal,
  fused_score: fused_score,
  coupling_type: coupling_type,
  last_co_change_commit: last_co_change_commit,
  last_co_change_at: last_co_change_at,
  mined_at: mined_at
}
FROM co_change
WHERE file_a = $file_a AND file_b = $file_b
LIMIT 1;
```

### `list_co_change_edges_for_file(file_path, min_fused_score)`

### Cozo

```cozo
?[...] := *co_change_edges{...}, file_a = $file_path, fused_score >= $min_fused_score
?[...] := *co_change_edges{...}, file_b = $file_path, fused_score >= $min_fused_score
```

### SurrealQL

```sql
SELECT VALUE { ... }
FROM co_change
WHERE (file_a = $file_path OR file_b = $file_path)
  AND fused_score >= $min_fused_score;
```

Parity notes:

- Results sorted in Rust by `fused_score DESC, file_a ASC, file_b ASC`.

### `list_top_co_change_edges(limit)`

### Cozo

```cozo
?[...] := *co_change_edges{...}
```

### SurrealQL

```sql
SELECT VALUE { ... }
FROM co_change;
```

Parity notes:

- Cozo and Surreal both sort in Rust by `fused_score DESC, file_a ASC, file_b ASC`.
- `limit` clamped to `[1, 200]`.

### `replace_tested_by_for_test_file(test_file, records)`

### Cozo query 1 (delete existing rows for test file)

```cozo
?[target_file, test_file] :=
    *tested_by{target_file, test_file, intent_count, confidence, inference_method},
    test_file = $test_file
:rm tested_by { target_file, test_file }
```

### SurrealQL query 1

```sql
DELETE tested_by WHERE test_file = $test_file;
```

### Cozo query 2 (insert replacement rows, repeated per record)

```cozo
?[target_file, test_file, intent_count, confidence, inference_method] <- [[...]]
:put tested_by { target_file, test_file => intent_count, confidence, inference_method }
```

### SurrealQL query 2

```sql
CREATE tested_by SET
  target_file = $target_file,
  test_file = $test_file,
  intent_count = $intent_count,
  confidence = $confidence,
  inference_method = $inference_method;
```

Parity notes:

- `intent_count` clamped to `>= 0`
- `confidence` clamped to `[0.0, 1.0]`

### `list_tested_by_for_target_file(target_file)`

### Cozo

```cozo
?[target_file, test_file, intent_count, confidence, inference_method] :=
    *tested_by{target_file, test_file, intent_count, confidence, inference_method},
    target_file = $target_file
```

### SurrealQL

```sql
SELECT VALUE {
  target_file: target_file,
  test_file: test_file,
  intent_count: intent_count,
  confidence: confidence,
  inference_method: inference_method
}
FROM tested_by
WHERE target_file = $target_file;
```

Parity notes:

- Results sorted in Rust by `confidence DESC, test_file ASC`.

## `GraphStore` Trait Method Queries

### `upsert_symbol_node(symbol)`

### Cozo

```cozo
?[symbol_id, qualified_name, name, kind, file_path, language, signature_fingerprint, last_seen_at] <- [[...]]
:put symbols {
  symbol_id =>
  qualified_name,
  name,
  kind,
  file_path,
  language,
  signature_fingerprint,
  last_seen_at
}
```

### SurrealQL

```sql
UPSERT symbol SET
  symbol_id = $symbol_id,
  qualified_name = $qualified_name,
  name = $name,
  kind = $kind,
  file_path = $file_path,
  language = $language,
  signature_fingerprint = $signature_fingerprint,
  last_seen_at = $last_seen_at,
  updated_at = time::now()
WHERE symbol_id = $symbol_id;
```

### `upsert_edge(edge)`

### Cozo

```cozo
?[source_id, target_id, edge_kind, file_path] <- [[...]]
:put edges { source_id, target_id, edge_kind => file_path }
```

### SurrealQL

```sql
LET $src = (SELECT id FROM symbol WHERE symbol_id = $source_id LIMIT 1);
LET $dst = (SELECT id FROM symbol WHERE symbol_id = $target_id LIMIT 1);
IF count($src) = 0 OR count($dst) = 0 { RETURN NONE; };
DELETE depends_on
WHERE out = $src[0].id AND in = $dst[0].id AND file_path = $file_path AND edge_kind = $edge_kind;
RELATE $src[0].id->depends_on->$dst[0].id
SET edge_kind = $edge_kind, file_path = $file_path, weight = 1.0;
```

Parity notes:

- Surreal silently skips edge insertion if either symbol is missing (matches practical Cozo behavior when graph sync only inserts resolved nodes).

### `get_callers(qualified_name)`

### Cozo

```cozo
?[symbol_id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at] :=
    *edges{source_id: symbol_id, target_id, edge_kind: "calls"},
    *symbols{symbol_id: target_id, qualified_name: $qname},
    *symbols{symbol_id, qualified_name, file_path, language, kind, signature_fingerprint, last_seen_at}
:order qualified_name, symbol_id
```

### SurrealQL

```sql
SELECT VALUE {
  id: source.symbol_id,
  file_path: source.file_path,
  language: source.language,
  kind: source.kind,
  qualified_name: source.qualified_name,
  signature_fingerprint: source.signature_fingerprint,
  last_seen_at: source.last_seen_at
}
FROM depends_on
LET source = out, target = in
WHERE edge_kind = "calls" AND target.qualified_name = $qualified_name
ORDER BY source.qualified_name ASC, source.symbol_id ASC;
```

### `get_dependencies(symbol_id)`

### Cozo

```cozo
?[symbol_id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at] :=
    *edges{source_id: $source_id, target_id: symbol_id, edge_kind: "calls"},
    *symbols{symbol_id, qualified_name, file_path, language, kind, signature_fingerprint, last_seen_at}
:order qualified_name, symbol_id
```

### SurrealQL

```sql
SELECT VALUE {
  id: target.symbol_id,
  file_path: target.file_path,
  language: target.language,
  kind: target.kind,
  qualified_name: target.qualified_name,
  signature_fingerprint: target.signature_fingerprint,
  last_seen_at: target.last_seen_at
}
FROM depends_on
LET source = out, target = in
WHERE edge_kind = "calls" AND source.symbol_id = $symbol_id
ORDER BY target.qualified_name ASC, target.symbol_id ASC;
```

### `get_call_chain(symbol_id, depth)`

### Cozo

```cozo
reachable[node, depth] := *edges{source_id: $start, target_id: node, edge_kind: "calls"}, depth = 1
reachable[node, depth] :=
    reachable[prev, prev_depth],
    prev_depth < $max_depth,
    *edges{source_id: prev, target_id: node, edge_kind: "calls"},
    depth = prev_depth + 1

?[symbol_id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at, depth] :=
    reachable[symbol_id, depth],
    *symbols{symbol_id, qualified_name, file_path, language, kind, signature_fingerprint, last_seen_at}
:order depth, qualified_name, symbol_id
```

### SurrealQL + Rust

Surreal implementation fetches call edges and symbols separately, then performs BFS layering in Rust:

```sql
SELECT VALUE {
  source_id: source.symbol_id,
  target_id: target.symbol_id,
  edge_kind: edge_kind
}
FROM depends_on
LET source = out, target = in
WHERE edge_kind INSIDE ["calls"] AND source != NONE AND target != NONE;

SELECT VALUE {
  id: symbol_id,
  file_path: file_path,
  language: language,
  kind: kind,
  qualified_name: qualified_name,
  signature_fingerprint: signature_fingerprint,
  last_seen_at: last_seen_at
}
FROM symbol
ORDER BY qualified_name ASC, symbol_id ASC;
```

Parity notes:

- Dedupes repeated nodes by first-seen (minimum) depth.
- Output is `Vec<Vec<SymbolRecord>>` grouped by depth level.
- Each level sorted by `qualified_name`, then `symbol_id`.

### `delete_edges_for_file(file_path)`

### Cozo

```cozo
?[source_id, target_id, edge_kind] := *edges{source_id, target_id, edge_kind, file_path: $file_path}
:rm edges { source_id, target_id, edge_kind }
```

### SurrealQL

```sql
DELETE depends_on WHERE file_path = $file_path;
```

## Cozo-Only Derived Helpers Reimplemented in Rust (No Direct SurrealQL Equivalent)

These methods are public in `graph_cozo.rs` and are reimplemented in `graph_surreal.rs`:

- `list_louvain_communities`
- `list_cross_community_edges`
- `list_pagerank`
- `list_strongly_connected_components`
- `list_connected_components`

Surreal strategy:

1. Query dependency edges (`calls` + `depends_on`) using parameterized SurrealQL.
2. Run CPU-bound computation in `tokio::task::spawn_blocking`.
3. Sort outputs to match Cozo wrapper behavior.

## Behavior / Sorting Parity Summary

- `list_louvain_communities`: sorted by `symbol_id ASC`
- `list_cross_community_edges`: sorted by `(source_id, target_id, edge_kind)`
- `list_pagerank`: sorted by `(score DESC, symbol_id ASC)`
- `list_strongly_connected_components`: component members sorted asc; components sorted by size desc then lexical members
- `list_connected_components`: same sorting as SCC
- `list_co_change_edges_for_file`: sorted by `(fused_score DESC, file_a ASC, file_b ASC)`
- `list_top_co_change_edges`: same sorting + `limit` clamp `[1, 200]`
- `list_tested_by_for_target_file`: sorted by `(confidence DESC, test_file ASC)`
- `list_upstream_dependency_traversal.nodes`: `(depth ASC, symbol_id ASC)`
- `list_upstream_dependency_traversal.edges`: `(depth ASC, source_id ASC, target_id ASC)`

