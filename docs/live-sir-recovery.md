# Live SIR scheduling and provider recovery

The watcher records edits that arrive while a symbol's SIR is being generated. One
follow-up serves the latest observed body after the current flight finishes. A
removed and recreated ID waits for the old flight, and completion tickets prevent
an old completion from clearing another flight.

## Provider outages

A leaf inference failure leaves one delayed obligation for the symbol. Cooldown is
5, 10, 20, 40, 80, 160, then 300 seconds per failed task, capped at 300 seconds.
Recovery continues while the daemon runs until the source receives a current SIR
or the symbol is removed. Time is monotonic. Existing worker polling discovers due
work without another edit, explicit request or restart.

Recovery task starts are spaced at least one second apart across the shared
scheduler. When both recovery and ordinary work are available, service alternates
so neither backlog can monopolize workers. This is a limit on recovery task starts,
not a global limit on every provider HTTP call: each task still has the pipeline's
existing bounded internal attempts, and ordinary first-attempt work is unchanged.
Repeated explicit SIR requests do not bypass provider cooldown. A new edit replaces
old retry debt with the latest-body obligation.

Provider failures are identified separately from aggregate processing failures.
Superseded results and downstream embedding failures do not trigger repeated leaf
inference merely because they increment an aggregate failure count. Other task
errors retain the scheduling patch's one attempt plus two immediate lower-priority
retries, followed by a stop until a new edit/request/restart. A task already
recovering from a provider failure keeps its delayed obligation through temporary
task errors. Errors while recording an observed provider failure also retain that
classification. Explicit unresolved-leaf accounting also preserves existing recovery
when a source cannot be prepared or a leaf write fails. First-attempt non-provider
preparation/persistence failures keep their prior handling.

## Manual SIRs and freshness

A current SIR written during cooldown satisfies the retry. Source-bound fresh SIRs
are skipped based on their source hash rather than whole-second timestamps. A known
mismatching hash regenerates; legacy SIRs without a recorded source hash retain the
older timestamp fallback. Explicit forced generation retains its force behavior.
The normal watcher does not precompute a force flag before job preparation, which
would otherwise permit a manual injection to become an overwrite baseline.

Existing source validation, symbol-deletion checks and compare-and-set persistence
remain in force. This does not establish ownership for old annotations without
source binding. Retry scheduling is in-memory; startup reconciliation remains
responsible for finding work after daemon restart.

## Verification

Deterministic tests advance an injected monotonic clock rather than sleeping through
cooldowns. Coverage includes an unchanged source whose provider stays unavailable
past three tasks and later recovers, polling before deadlines, deadline cap, fairness,
new edits and removal during cooldown, manual injection during failed inference,
during cooldown and after retry preflight, and Python identity split/rejoin during
an active flight. Separate stats checks distinguish swallowed preparation/persistence
failures from generic task errors. The original exhaustive scheduling model remains
alongside recovery coverage. Cooldown tests do not sleep through deadlines; the
pipeline's existing internal inference retries still use their normal short waits.
