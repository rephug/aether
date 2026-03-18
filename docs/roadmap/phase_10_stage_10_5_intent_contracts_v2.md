# Phase 10 — The Conductor

## Stage 10.5 — Intent Contracts (Semantic Type Checking)

### Purpose

Declare and enforce semantic expectations on symbols. Type systems enforce structural contracts ("takes a u64, returns a Result"). Intent Contracts enforce semantic contracts ("must reject zero amounts," "must not modify account balance," "preserves idempotency"). On every SIR regeneration, AETHER compares the new SIR against the symbol's contracts and flags violations — catching semantic regressions before tests, before code review, before production.

### Prerequisites

- Stage 10.1 merged (fingerprint history, prompt hashing)
- Stage 10.2 merged (staleness scoring, Δ_sem computation)
- Stage 10.4 recommended but not required (Seismograph metrics feed into contract violation alerting, but contracts work independently)
- **Data requirement for implicit inference:** At least 10+ SIR versions per symbol in `sir_history` table. Explicit contracts work immediately.

### What Problem This Solves

Today, if a developer changes `validate_amount()` to skip the daily limit check, nothing in AETHER flags it. The SIR regeneration will accurately describe the new behavior ("validates that amount is positive") but nobody notices the *omission*. Tests might catch it — if they exist and test that case. Intent Contracts make the expectation explicit: "this function MUST check the daily limit." If the regenerated SIR no longer mentions it, AETHER flags a violation.

### In scope

#### Explicit contracts

A developer (or agent) declares contracts on a symbol:

```
aetherd contract add payments::validate_amount \
    --must "reject zero or negative amounts" \
    --must "check against daily transaction limit" \
    --must-not "modify account balance" \
    --preserves "idempotency"
```

Stored in a new `intent_contracts` SQLite table:

```sql
CREATE TABLE IF NOT EXISTS intent_contracts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    symbol_id TEXT NOT NULL,
    clause_type TEXT NOT NULL,        -- 'must', 'must_not', 'preserves'
    clause_text TEXT NOT NULL,
    clause_embedding BLOB,            -- 3072-dim f32 vector, embedded on creation
    created_at INTEGER NOT NULL,
    created_by TEXT NOT NULL,          -- 'human', 'agent', 'inferred'
    active INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_contracts_symbol
    ON intent_contracts(symbol_id) WHERE active = 1;
```

Clauses are embedded at creation time for fast cosine comparison during verification.

#### Contract verification — two-stage cascade (Deep Think finding B1)

Runs **synchronously inside the SIR pipeline** after each SIR upsert (batch ingest, watcher, inject). Not a separate background job — contract violations are detected at generation time.

**Stage 1 — Embedding pre-filter:**
For each active contract clause on the regenerated symbol:
- Compute cosine similarity between clause embedding and new SIR embedding
- If similarity > 0.88 → PASS (clause clearly satisfied)
- If similarity < 0.50 → FAIL (clause clearly violated)
- If 0.50 ≤ similarity ≤ 0.88 → ambiguous, send to Stage 2

**Stage 2 — LLM judge (ambiguous cases only):**
Dispatch to flash-lite:
```
Contract clause: "{clause_text}"
Implementation SIR: "{sir_intent}. {sir_behavior}"
Does the implementation satisfy this contract clause?
Return JSON: {"violated": true/false, "reason": "..."}
```

**Expected filter rate:** ~90% resolved by embedding pre-filter. LLM judge called for ~10% of clauses — a few hundred calls per nightly batch on a 5K codebase. Under $0.10.

**Negation awareness:** Pure embedding similarity is blind to negation ("rejects null" vs "accepts null" have ~0.95 cosine similarity). The LLM judge stage exists specifically to catch these cases.

#### Violation handling — leaky bucket + HITL (Deep Think finding B4)

Never alert on the first failure (LLM phrasing jitter):

1. First violation: set `violation_streak = 1` on the contract clause. No alert.
2. Second consecutive violation (next regeneration also fails): flag as active violation. Emit structured log event. Surface in dashboard.
3. Developer can "Dismiss" in dashboard → violation is stored in `dismissed_violations` table and injected as a negative few-shot example into the Stage 2 LLM judge prompt (feedback loop that improves accuracy over time).
4. If a dismissed violation recurs 3+ more times: re-surface with escalated severity.

Stored in `intent_violations`:

```sql
CREATE TABLE IF NOT EXISTS intent_violations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    contract_id INTEGER NOT NULL REFERENCES intent_contracts(id),
    symbol_id TEXT NOT NULL,
    sir_version INTEGER NOT NULL,
    violation_type TEXT NOT NULL,      -- 'embedding_fail', 'llm_judge_fail'
    confidence REAL,
    reason TEXT,
    detected_at INTEGER NOT NULL,
    dismissed INTEGER NOT NULL DEFAULT 0,
    dismissed_at INTEGER,
    dismissed_reason TEXT
);
```

#### Implicit contract inference (Deep Think finding B2)

**Rating: Hard / Research-grade. Build after explicit contracts are proven.**

Analyze SIR history to detect stable semantic properties:

1. Extract discrete claims/sentences from the last 10+ SIR versions for a symbol (from `sir_history` table)
2. Embed each claim using the configured embedding provider
3. Run DBSCAN clustering on the claim embeddings
4. If a dense cluster contains representations from ≥80% of historical versions → it's a stable invariant
5. The medoid (actual historical sentence closest to the cluster centroid) is promoted to an implicit contract with `created_by = "inferred"`

**Challenge:** SIRs from different models (flash-lite, flash, Sonnet) phrase the same concept differently. Embedding comparison handles this better than text matching, but DBSCAN parameters (eps, min_samples) need per-codebase tuning.

**Deferred to 10.5b:** Build explicit contracts first. Add implicit inference once enough SIR history exists and the explicit contract pipeline is proven stable.

#### Cross-symbol contract propagation — semantic foreign keys (Deep Think finding B3)

When symbol A has a contract and calls symbol B, inject the contract context into B's SIR generation prompt:

*"Context: Downstream caller `payments::validate_amount` requires the property 'check daily limit'. Ensure your summary clarifies if this behavior is preserved."*

This fits into the existing `SirEnrichmentContext` used by the triage/deep passes. The LLM handles the reasoning — no formal verification needed.

Implementation:
- When building a triage/deep prompt for symbol B, query `intent_contracts` for all symbols that depend on B
- If any have active contracts, append the relevant clauses to the enrichment context
- The LLM naturally addresses them in the generated SIR

#### CLI commands

- `aetherd contract add <symbol> --must "..." [--must-not "..."] [--preserves "..."]` — add contract clauses
- `aetherd contract list [symbol]` — list active contracts (all or per-symbol)
- `aetherd contract remove <contract_id>` — deactivate a contract clause
- `aetherd contract check [symbol]` — force-run verification for a symbol or all symbols with contracts
- `aetherd contract infer <symbol>` — run implicit inference for a symbol (10.5b)

#### Dashboard page

One new page: **Contract Health**
- List of all active contracts grouped by symbol
- Current status: ✅ satisfied, ⚠️ first violation, ❌ active violation
- Click to see violation history, dismiss violations
- Summary stats: total contracts, satisfaction rate, most violated contracts

### New config: `[contracts]`

```toml
[contracts]
enabled = false
embedding_pass_threshold = 0.88
embedding_fail_threshold = 0.50
judge_model = ""                    # defaults to [inference] provider if empty
judge_provider = ""
streak_threshold = 2                # consecutive violations before alerting
implicit_inference_enabled = false  # 10.5b feature
implicit_min_versions = 10          # minimum SIR history versions for inference
implicit_persistence_threshold = 0.80  # % of versions that must contain the invariant
```

### Out of scope

- Automatic contract generation from tests (future — map test assertions to contracts)
- Contract versioning / changelog
- Cross-repository contract propagation
- UI for editing contracts (CLI only for MVP)

### Pass criteria

1. `aetherd contract add payments::validate_amount --must "reject zero amounts"` creates a contract with embedded clause.
2. `aetherd contract list` shows active contracts.
3. Contract verification runs inline during SIR upsert — a batch ingest triggers verification for all symbols with contracts.
4. Embedding pre-filter resolves ~90% of clauses without LLM call (verify by counting LLM judge invocations).
5. Second consecutive violation triggers alert. First violation is silent (leaky bucket).
6. Dismissed violations stored and do not re-alert (unless 3+ recurrences).
7. Cross-symbol propagation: contract clauses from callers appear in callee's enrichment context during triage/deep pass.
8. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings` pass.
9. `cargo test -p aetherd` passes.

### Estimated Codex runs: 2-3 (explicit contracts only), +1-2 for 10.5b (implicit inference)
