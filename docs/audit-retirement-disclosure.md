# Historical unresolved audit disclosure (local candidate)

This local change adds a historical section to aether_audit_report. Active
findings and summary retain their current filter semantics. Retired findings
are not reattached to a successor, restored into sir_audit, or resolved through
the active finding API. Canonical publication and lineage decisions are unchanged.

The historical scope is retained_retirement_archives_unfiltered: all statuses
were observed in immutable retirement evidence, independently of the active
crate/severity/category/status filters. Open and confirmed statuses count as
unresolved observations at retirement. A later source fix is not inferred.
The same finding ID may occur in multiple retirements, so counts describe
observations, not distinct findings or proof of current code safety.

Each sample includes retirement_id, original symbol_id and finding_id,
status_at_retirement, severity, category and a bounded description preview.
Use the original symbol's aether_symbol_timeline with include_retired_evidence
for full attribution/evidence; heuristic lineage is not an automatic reassignment.

## Bounded coverage and uncertainty

The reader uses one SQLite DEFERRED transaction for the historical scan. It
reads at most 129 retirement metadata rows, considers the newest 128 and reports
more_retirements if another exists. It reads at most 1 MiB of stored archive bytes per row
and 8 MiB total, returning 1-50 finding samples. UTF-16 stores can expand when
decoded to UTF-8; the limits count stored bytes. Opaque SIR bodies and other
non-audit sections are ignored by the typed deserializer.
Archive sizing uses octet_length directly on the column, so oversized contents
are not loaded just to measure them; the bundled SQLite is 3.46. See the
[SQLite byte-length contract](https://www.sqlite.org/lang_corefunc.html#octet_length).


scan_complete means the retained archive scan covered all available retirements
without skipped/invalid/oversized/budget-exhausted rows. It does not establish
complete pre-retention history or cover legacy findings never archived.
retention_available is false on pre-retention read-only schemas.
retirements_considered counts metadata rows; retirements_skipped discloses
unexamined/invalid bodies. unresolved_observations and samples_omitted apply to
successfully scanned archives only. A partial zero count is not an all-clear.

Missing/malformed audit sections, unexpected status, invalid IDs/attribution or
oversized severity/category fields make coverage incomplete. SQL errors fail
the report instead of becoming empty historical evidence. Description previews
are UTF-8 safe and explicitly labeled when truncated. No archive or active row
is changed by the reader. No schema migration or dependency is added.

## Regression and validation

The real mcp_tools regression seeds an open finding through the MCP endpoint,
publishes a changed-signature replacement through publish_and_reconcile_symbols,
checkpoints/copies the synthetic database after closing its writers, reopens
the restored fixture and queries the MCP endpoint. It requires the historical
finding to remain visible despite active filters matching nothing, and proves
no active finding is attached to the successor or silently resolved.

Focused Rust tests include the production reader and MCP response-conversion
modules directly in a small external local harness. They cover bounded and
malformed history and serialization, but do not execute the full store/MCP
initialization or canonical publication path. Do not substitute those results
for the real mcp_tools regression or the required crate Clippy/test gates.

Full qualification requires formatting, all-targets Clippy with warnings denied,
and per-crate tests for aether-store and aether-mcp, including MCP's verification
feature. Run in an isolated checkout with synthetic stores and no provider/GPU
calls. Never use a workspace-wide Cargo command. Select parallelism from fresh
CPU/RAM capacity and measured compiler peaks; preserve unrelated workloads.

The portable Linux gate job records source/tree hashes, exact commands, terminal
outcomes, test-selection evidence and resource measurements. A separately prepared
compatibility delta must pass its final-version MCP gates before it is qualified;
the frozen precursor's results alone do not qualify that delta. Store gates may
be retained only when its source and relevant inputs remain identical. Keep the
tested source and evidence together.

The existing rollout runbook still governs mixed writers, coordinated
backup/restore, legacy partial catalogs and activation. Passing these tests is
not a production rollout or authorization to change any live process/store.


## Older response compatibility

A new Rust decoder defaults an omitted historical field to scope=not_reported,
retention_available=false and scan_complete=false. An older server response is
unknown coverage, never a complete empty history. A real MCP deserialization
regression covers this case; strict older clients remain a separate compatibility
question. This does not upgrade any running client or make mixed older writers
safe on an upgraded store.
