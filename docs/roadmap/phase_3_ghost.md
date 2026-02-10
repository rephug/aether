# docs/roadmap/phase_3_ghost.md

# Phase 3 — Ghost (Verification)

## Goal
Before marking an AI suggestion as “good”, verify it by running checks/tests in an isolated environment.

## Stage 3.1 — Host-based verification (fastest to ship)
- Run: syntax checks, formatters, typecheck, unit tests on host
- Return: Verified ✅ or Failed ❌ with logs
- No sandbox yet

Pass criteria:
- Patch can be applied to temp workspace and verified with deterministic outputs

## Stage 3.2 — Container verification (Docker baseline)
- Run verification in a container for reproducibility
- No microVM assumptions

Pass criteria:
- Same patch produces same result across machines (given same container image)

## Stage 3.3 — MicroVM acceleration (optional)
- Firecracker on Linux fast path
- Hyper-V on Windows reliable baseline
- Treat WSL2 + nested virt as optional, not guaranteed

Pass criteria:
- Snapshot/warm pool restores quickly AND remains stable over repeated runs

Non-goals:
- Shipping “universal determinism” on Windows
