# Phase 6 — The Chronicler

## Stage 6.2 — Multi-Signal Coupling Detection

### Purpose
Discover coupled files using three independent signals fused into a single score — something no existing tool does. Surface "blast radius" warnings when an agent or developer touches a file.

### What It Borrows and What It Adds
- **Borrowed from EngramPro:** Git co-change mining concept, blast radius analysis.
- **Borrowed from Zep/Graphiti:** Temporal awareness of relationships.
- **NOVEL — Multi-Signal Fusion:** EngramPro and all academic literature compute coupling from *one signal* (git co-change OR static dependency OR semantic similarity). AETHER fuses all three because it has all three data layers in the same process. When signals agree, extremely high confidence. When they disagree (high temporal coupling, no static dependency, low semantic similarity), the disagreement itself is a signal — flag as hidden operational coupling needing investigation.

### Design Decisions
- **Scope:** File-level coupling only (not symbol-level). Symbol-level would require tracking which symbols changed per commit, which is expensive and noisy. File-level co-change is the proven signal (per EngramPro, per academic research on temporal coupling).
- **Storage:** New CozoDB edge type `CO_CHANGES_WITH` with weight = co-change frequency.
- **Mining window:** Configurable, default last 500 commits.
- **Minimum threshold:** Files must co-change in ≥3 commits to create an edge (eliminates noise from bulk reformatting commits).

### Schema: CozoDB `co_change_edges` relation

```
:create co_change_edges {
    file_a: String,
    file_b: String,
    =>
    co_change_count: Int,
    total_commits_a: Int,
    total_commits_b: Int,
    git_coupling: Float,             -- co_change_count / max(total_commits_a, total_commits_b)
    static_signal: Float,            -- 1.0 if AST dependency exists, else 0.0
    semantic_signal: Float,          -- max SIR embedding similarity between files
    fused_score: Float,              -- weighted combination of all three
    coupling_type: String,           -- "structural" | "temporal" | "semantic" | "hidden_operational" | "multi"
    last_co_change_commit: String,
    last_co_change_at: Int,
    mined_at: Int
}
```

**Single-signal coupling score (git only):**
```
git_coupling = co_change_count / max(total_commits_a, total_commits_b)
```
Range 0.0–1.0.

**Multi-signal fusion (applied after mining):**
For each file pair in `co_change_edges`, also compute:
```
static_signal:   1.0 if any CALLS/DEPENDS_ON edge exists between symbols in file_a and file_b (from CozoDB dependency graph), else 0.0
semantic_signal: max cosine similarity between any SIR embedding in file_a and any SIR embedding in file_b (from LanceDB)
temporal_signal: git_coupling score above
```

**Fused coupling score:**
```
fused_score = 0.5 * temporal_signal + 0.3 * static_signal + 0.2 * semantic_signal
```

Weights chosen because temporal co-change is the strongest empirical predictor of coupling (per Cataldo et al. 2009), static dependency is ground truth when present, and semantic similarity catches conceptual coupling.

**Signal disagreement flag:** If `temporal_signal >= 0.5` AND `static_signal == 0.0` AND `semantic_signal < 0.3`, set `coupling_type = "hidden_operational"` — files change together for reasons not visible in code structure. Worth a project note.

Risk levels (on fused_score): ≥ 0.7 = Critical. ≥ 0.4 = High. ≥ 0.2 = Medium. Below = Low.

### SQLite: `coupling_mining_state` table

```sql
CREATE TABLE IF NOT EXISTS coupling_mining_state (
    id              INTEGER PRIMARY KEY DEFAULT 1,
    last_commit_hash TEXT,
    last_mined_at   INTEGER,
    commits_scanned INTEGER DEFAULT 0
);
```

Tracks where the miner left off for incremental updates.

### Mining Algorithm

```
Phase 1 — Git temporal signal:
1. Read last_commit_hash from coupling_mining_state.
2. Walk commits from HEAD back to last_commit_hash (or 500 commits, whichever is fewer) via gix.
3. For each commit:
   a. Get changed files (diff parent → commit).
   b. Filter out: lockfiles, .gitignore, auto-generated files, files matching [coupling.exclude] patterns.
   c. For each pair (file_a, file_b) where file_a < file_b (canonical ordering):
      - Increment co_change_count in memory map.
4. After walk:
   a. Compute git_coupling = co_change_count / max(total_commits_a, total_commits_b).
   b. Filter pairs with co_change_count >= threshold.

Phase 2 — Static + semantic signal enrichment:
5. For each surviving pair (file_a, file_b):
   a. Static signal: query CozoDB for any CALLS or DEPENDS_ON edge where source symbol is in file_a and target symbol is in file_b (or vice versa). If any edge exists, static_signal = 1.0, else 0.0.
   b. Semantic signal: for all SIR embeddings belonging to symbols in file_a, compute max cosine similarity against all SIR embeddings belonging to symbols in file_b (via LanceDB). Set semantic_signal = max_sim. If no SIR exists for either file, semantic_signal = 0.0.
   c. Compute fused_score = 0.5 * git_coupling + 0.3 * static_signal + 0.2 * semantic_signal.
   d. Classify coupling_type:
      - static_signal > 0 AND git_coupling >= 0.2 → "multi"
      - static_signal > 0 AND git_coupling < 0.2 → "structural"
      - static_signal == 0 AND semantic_signal >= 0.3 → "semantic"
      - static_signal == 0 AND semantic_signal < 0.3 AND git_coupling >= 0.5 → "hidden_operational"
      - else → "temporal"
6. Upsert results into CozoDB co_change_edges.
7. Update coupling_mining_state with HEAD commit hash.
```

**Performance note:** Phase 2 is the expensive part (CozoDB + LanceDB queries per pair). For large repos, batch the Datalog and LanceDB queries. Phase 2 can be deferred (store git_coupling immediately, enrich lazily on first blast_radius query for that pair).

### MCP Tool: `aether_blast_radius`

**Request:**
```json
{
  "file": "crates/aether-store/src/lib.rs",
  "min_risk": "medium"      -- "low" | "medium" | "high" | "critical"
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "target_file": "crates/aether-store/src/lib.rs",
  "mining_state": {
    "commits_scanned": 487,
    "last_mined_at": 1708000000000
  },
  "coupled_files": [
    {
      "file": "crates/aether-mcp/src/lib.rs",
      "risk_level": "critical",
      "fused_score": 0.89,
      "coupling_type": "multi",
      "signals": {
        "temporal": 0.89,
        "static": 1.0,
        "semantic": 0.72
      },
      "co_change_count": 48,
      "total_commits": 54,
      "last_co_change": "a1b2c3d",
      "notes": []                    -- Project notes referencing this file (from 6.1)
    },
    {
      "file": "crates/aether-mcp/tests/mcp_tools.rs",
      "risk_level": "high",
      "fused_score": 0.63,
      "coupling_type": "hidden_operational",
      "signals": {
        "temporal": 0.72,
        "static": 0.0,
        "semantic": 0.15
      },
      "co_change_count": 31,
      "total_commits": 43,
      "last_co_change": "e4f5g6h",
      "notes": ["MCP integration tests must mirror store API changes"]
    }
  ],
  "test_guards": []             -- Populated in Stage 6.3
}
```

**Behavior:**
- If coupling has never been mined, run mining first (may take a few seconds).
- If stale (>100 new commits since last mine), re-mine incrementally.
- Cross-reference coupled files against project notes (from 6.1) that reference those files.

### CLI Surface

```bash
# Mine coupling data
aether mine-coupling --commits 500

# Check blast radius
aether blast-radius crates/aether-store/src/lib.rs --min-risk medium

# Show most coupled file pairs
aether coupling-report --top 20
```

### Config

```toml
[coupling]
enabled = true
commit_window = 500
min_co_change_count = 3
exclude_patterns = ["*.lock", "*.generated.*", ".gitignore"]
```

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Not a git repo | `mining_state: null`, `coupled_files: []`, no error |
| Fewer than 10 commits | Mine all available, warn "limited history" |
| File not found in any commits | Return empty `coupled_files` |
| Merge commits | Skip merge commits (>1 parent), count only regular commits |
| Binary files in diff | Exclude from coupling analysis |
| Bulk formatting commit (50+ files changed) | Skip commits where >30 files changed (configurable) |

### Pass Criteria
1. Mining scans git history via `gix` and populates CozoDB `co_change_edges` with git_coupling scores.
2. Multi-signal enrichment computes static_signal from CozoDB dependency edges and semantic_signal from LanceDB SIR embeddings.
3. Fused_score correctly combines all three signals with specified weights.
4. `coupling_type` classification correctly identifies "hidden_operational" (high temporal, no static, low semantic).
5. Incremental mining picks up where it left off.
6. `aether_blast_radius` returns coupled files sorted by fused_score with per-signal breakdown.
7. Excluded patterns filter correctly.
8. Bulk commits (>30 files) are skipped.
9. Graceful degradation: if no SIR exists for a file, semantic_signal defaults to 0.0.
10. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_2_temporal_coupling.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-2-temporal-coupling off main.
3) Create worktree ../aether-phase6-stage6-2 for that branch and switch into it.
4) In crates/aether-store or a new module in aether-memory:
   - Add coupling_mining_state SQLite table and migration.
   - Add co_change_edges CozoDB relation (with git_coupling, static_signal, semantic_signal, fused_score, coupling_type fields).
   - Implement Phase 1 mining: walk gix commits, extract changed files, compute co-change pairs and git_coupling scores.
   - Implement Phase 2 enrichment: for each pair, query CozoDB for CALLS/DEPENDS_ON edges (static_signal), query LanceDB for max SIR embedding cosine similarity (semantic_signal).
   - Implement fused_score = 0.5*temporal + 0.3*static + 0.2*semantic.
   - Implement coupling_type classification (multi, structural, semantic, hidden_operational, temporal).
   - Implement incremental mining (resume from last_commit_hash).
5) Add MCP tool aether_blast_radius in crates/aether-mcp/src/lib.rs:
   - Input: file path + min_risk filter.
   - Output: coupled files with fused_score, per-signal breakdown, coupling_type, cross-referenced against project_notes file_refs.
   - Auto-mine if never mined or stale (>100 new commits).
6) Add CLI commands in crates/aetherd:
   - `aether mine-coupling --commits <n>`
   - `aether blast-radius <file> --min-risk <level>`
   - `aether coupling-report --top <n>`
7) Add [coupling] config section in aether-config.
8) Add tests:
   - Unit tests with synthetic git history (create temp repo with known co-changes).
   - Verify coupling scores match expected values.
   - Verify bulk commit filtering.
   - Integration test for MCP tool schema.
9) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
10) Commit with message: "Add temporal coupling detection with blast radius MCP tool"
```
