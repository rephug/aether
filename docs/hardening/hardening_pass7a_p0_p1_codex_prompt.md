# AETHER Hardening Pass 7a — P0/P1 Critical Fixes

You are working on the AETHER project at the repository root. This prompt contains
verified bug fixes from two independent Gemini deep code reviews, cross-validated
by Claude against the actual source at commit c5d84ed. Apply ALL fixes below, then
run the validation gate.

**CRITICAL: Do NOT change any public API signatures, struct field names, or trait
method signatures. All fixes are internal implementation changes only.**

**Read `docs/hardening/hardening_pass7_session_context.md` before starting.**

---

## Preflight

```bash
git status --porcelain
# Must be clean. If not, stop and report dirty files.

git fetch origin
git pull --ff-only origin main
```

## Branch + Worktree

```bash
git worktree add ../aether-hardening-pass7a feature/hardening-pass7a -b feature/hardening-pass7a
cd ../aether-hardening-pass7a
```

## Build Environment

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

---

## Fix A1: Concurrency Panic in Symbol Access Debouncing (P0)

**The bug:** In `increment_symbol_access_debounced`, `Instant::now()` is captured
before acquiring the mutex (line 1254). Under concurrent load, Thread A captures
`now`, gets preempted, Thread B acquires the lock and updates a symbol's
`last_accessed` to a time *after* Thread A's `now`. When Thread A resumes and
calls `tracker.retain(|_, last_accessed| now.duration_since(*last_accessed) < window)`,
`duration_since` panics because the argument is later than `self`.

**Why it matters:** Hard crash of `aetherd` and `aether-query` under any concurrent
MCP tool usage. The safe variant `saturating_duration_since` is already used 12
lines below at line 1273 — this is an inconsistency.

### File: `crates/aether-store/src/lib.rs`

Find the `tracker.retain` call inside `increment_symbol_access_debounced` (around
line 1261):

```rust
// BEFORE (line 1261):
tracker.retain(|_, last_accessed| now.duration_since(*last_accessed) < debounce_window);

// AFTER:
tracker.retain(|_, last_accessed| now.saturating_duration_since(*last_accessed) < debounce_window);
```

One-word change: `duration_since` → `saturating_duration_since`. Do NOT change
the `saturating_duration_since` call at line 1273 — it is already correct.

---

## Fix A2: Read-Only Violations in Increment Functions + Blast Radius (P0)

**The bug:** When agents invoke read-focused MCP tools (`aether_explain`,
`aether_ask`, `aether_recall`, `aether_blast_radius`) on the read-only
`aether-query` server, the tools call `increment_symbol_access` and
`increment_project_note_access`, which unconditionally start write transactions.
The database connection is opened with `SQLITE_OPEN_READ_ONLY`, so the write
attempt crashes with `attempt to write a readonly database`.

Additionally, `aether_blast_radius_logic` passes `auto_mine: true` unconditionally,
which triggers writes to the graph store.

### File: `crates/aether-store/src/lib.rs`

**Step A2a:** In `increment_symbol_access` (around line 1203), add a read-only
guard after the conn lock:

```rust
// BEFORE (around line 1221-1222):
        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;

// AFTER:
        let conn = self.conn.lock().unwrap();
        if conn.is_readonly(rusqlite::DatabaseName::Main).unwrap_or(false) {
            return Ok(());
        }
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
```

**Step A2b:** In `increment_project_note_access` (around line 2240), add the
identical read-only guard:

```rust
// BEFORE (around line 2249-2250):
        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;

// AFTER:
        let conn = self.conn.lock().unwrap();
        if conn.is_readonly(rusqlite::DatabaseName::Main).unwrap_or(false) {
            return Ok(());
        }
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
```

### File: `crates/aether-mcp/src/lib.rs`

**Step A2c:** In `aether_blast_radius_logic`, change `auto_mine: true` to respect
read-only mode (around line 1748):

```rust
// BEFORE (around line 1748):
            auto_mine: true,

// AFTER:
            auto_mine: !self.state.read_only,
```

**NOTE:** The Gemini review also recommended adding `require_writable()` to
`aether_drift_report_logic`. This is NOT needed — `DriftAnalyzer::report()` opens
its own connections and does not write to the DB. Do NOT add this guard.

---

## Fix A3: Timestamp Unit Mismatch in Causal Ranking Fallback (P0)

**The bug:** `resolve_change_metadata` falls back to `after.created_at` when no git
commit metadata exists. `created_at` in the `sir_history` table is stored in
**seconds** (set via `current_unix_timestamp()` which returns `duration.as_secs()`).
The `timestamp_ms` field is consumed at line 320:
`now_ms.saturating_sub(change_metadata.timestamp_ms)`, where `now_ms` is in
milliseconds. Subtracting seconds from milliseconds yields ~19,675 days, decaying
`recency_weight` to 0.0 for all non-git-backed SIR events.

**Why it matters:** Causal analysis silently ranks every symbol that doesn't have
git commit metadata with zero recency, making the entire causal scoring unreliable.

### File: `crates/aether-analysis/src/causal.rs`

Find `resolve_change_metadata` (around line 682). In the fallback `ChangeMetadata`
(around line 697):

```rust
// BEFORE (line 697):
        timestamp_ms: after.created_at.max(0),

// AFTER:
        timestamp_ms: after.created_at.max(0).saturating_mul(1000),
```

One-method-call addition: `.saturating_mul(1000)` converts seconds to milliseconds.

---

## Fix A4: Missing Transaction on Semantic Record Upsert (P0)

**The bug:** Pass 6a added a `DELETE FROM semantic_records` before the `INSERT` to
prevent stale record accumulation. However, the DELETE and INSERT are two separate
`conn.execute()` calls without a transaction. If the process crashes between the
DELETE and the INSERT, the existing semantic record is permanently lost and the new
one is never written.

### File: `crates/aether-store/src/document_store.rs`

Find the `insert_semantic_record` function (around line 182). Wrap the DELETE and
INSERT in a transaction:

```rust
// BEFORE (around line 190-191):
        let conn = self.conn.lock().unwrap();
        // Remove stale records for this unit+schema before inserting the new version.
        // This prevents accumulation when content_hash changes (which changes record_id).
        conn.execute(
            "DELETE FROM semantic_records WHERE unit_id = ?1 AND schema_name = ?2 AND record_id != ?3",
            params![
                record.unit_id.as_str(),
                record.schema_name.as_str(),
                record.record_id.as_str(),
            ],
        )?;
        conn.execute(

// AFTER:
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        // Remove stale records for this unit+schema before inserting the new version.
        // This prevents accumulation when content_hash changes (which changes record_id).
        tx.execute(
            "DELETE FROM semantic_records WHERE unit_id = ?1 AND schema_name = ?2 AND record_id != ?3",
            params![
                record.unit_id.as_str(),
                record.schema_name.as_str(),
                record.record_id.as_str(),
            ],
        )?;
        tx.execute(
```

Then find the closing of the INSERT execute (around 15 lines below). Change the
remaining `conn.execute` to `tx.execute` and add the commit:

```rust
// BEFORE (end of the INSERT, around line 230):
        )?;
        Ok(())

// AFTER:
        )?;
        tx.commit()?;
        Ok(())
```

**NOTE:** Changing `let conn` to `let mut conn` is required because
`conn.transaction()` takes `&mut self`.

---

## Fix A5: `std::thread::sleep` Blocking Tokio in SurrealDB Init (P1)

**The bug:** Inside `pub async fn open()`, when a SurrealKV lock error occurs,
the code retries with `std::thread::sleep(Duration::from_millis(50))`. This is a
synchronous blocking sleep on an async Tokio worker thread, halting all pending
tasks on that worker.

### File: `crates/aether-store/src/graph_surreal.rs`

Find the retry loop in `open()` (around line 50):

```rust
// BEFORE (line 50):
                            std::thread::sleep(Duration::from_millis(50));

// AFTER:
                            tokio::time::sleep(Duration::from_millis(50)).await;
```

One-line change. The function is already `async fn`, so `.await` is valid.

---

## Scope Guard

- Do NOT modify any MCP tool schemas, CLI argument shapes, or public API contracts.
- Do NOT add new crates or new workspace dependencies.
- Do NOT touch SQLite schema migrations, SurrealDB schema definitions, or LanceDB table schemas.
- Do NOT rename any public functions or types.
- Do NOT add `require_writable()` to `drift_report_logic` — the Gemini scan was
  wrong about it writing to the DB.
- If any fix cannot be applied because the code structure differs from what's
  described, report exactly what you found and skip that fix.

---

## Validation Gate

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

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
cargo test -p aether-query
cargo test -p aetherd
cargo test -p aether-dashboard
cargo test -p aether-document
cargo test -p aether-memory
cargo test -p aether-graph-algo
```

All tests MUST pass. If `cargo clippy` warns about unused imports after changes,
remove them.

---

## Commit Message

```
fix: P0/P1 hardening pass 7a — debounce panic, read-only guards, causal timestamp, semantic tx, SurrealDB sleep

- Fix duration_since panic in debounce retain → saturating_duration_since (P0)
- Add read-only guards to increment_symbol_access / increment_project_note_access (P0)
- Gate auto_mine on read_only flag in blast radius (P0)
- Convert causal fallback created_at seconds to milliseconds (P0)
- Wrap semantic record DELETE + INSERT in SQLite transaction (P0)
- Replace std::thread::sleep with tokio::time::sleep in SurrealDB init retry (P1)
```

---

## Post-Fix Commands

```bash
git add -A
git commit -m "fix: P0/P1 hardening pass 7a — debounce panic, read-only guards, causal timestamp, semantic tx, SurrealDB sleep"
git push origin feature/hardening-pass7a
gh pr create --base main --head feature/hardening-pass7a \
  --title "Hardening pass 7a: P0/P1 critical fixes from Gemini deep review" \
  --body "5 fixes: debounce concurrency panic, read-only mode violations, causal timestamp unit mismatch, semantic record transaction safety, SurrealDB async sleep."
```

After merge:

```bash
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-hardening-pass7a
git branch -d feature/hardening-pass7a
git worktree prune
```
