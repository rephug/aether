# AETHER Stress Test Harness

## Purpose

This suite benchmarks AETHER indexing and query behavior on real repositories across size tiers.
It validates Phase 8 reliability and scale goals by exercising:

- Pass 1 structural indexing (`--index-once`)
- Pass 2 inference path (`--index-once --full`, sampled)
- fsck integrity checks after indexing phases
- Query and MCP latency (lexical search, call-chain, `aether_get_sir`)
- Simulated crash recovery on medium tier

Reports are generated in JSON and Markdown for reproducible regression tracking.

## Prerequisites

- `git`
- Rust/Cargo toolchain (for `aetherd` / `aether-query` builds)
- `curl`
- `/usr/bin/time` (for peak RSS capture)
- Provider-specific requirements:
  - `ollama`: local Ollama running with `qwen3.5:9b`
  - `gemini`: `GEMINI_API_KEY`
  - `nim`: `NVIDIA_NIM_API_KEY`
  - `tiered`: `NVIDIA_NIM_API_KEY` + local Ollama

## Disk and Memory Guidance

### Disk by tier

- `--tier small`: about 200MB
- `--tier medium`: about 600MB
- `--tier all`: about 5GB

### Memory guidance

- Small: around 2-4GB recommended
- Medium: around 6-8GB recommended
- Large/all: 10GB+ recommended (12GB WSL2 can still OOM on full runs)

## Provider Modes

| Flag | Provider config | Requirements |
|------|------------------|--------------|
| `--provider ollama` | `qwen3_local` / `qwen3.5:9b` | Ollama up at `127.0.0.1:11434` |
| `--provider pass1-only` | none | no inference dependency |
| `--provider gemini` | `gemini` / `gemini-flash-latest` | `GEMINI_API_KEY` |
| `--provider nim` | `openai_compat` / NIM endpoint | `NVIDIA_NIM_API_KEY` |
| `--provider tiered` | cloud primary + local fallback | `NVIDIA_NIM_API_KEY` + Ollama |

## Storage Considerations

Default bench dir is `/mnt/d/aether-bench`.

Why this default:

- Avoids `/tmp` (often tmpfs/RAM-backed in WSL2)
- Reduces pressure on WSL2 ext4.vhdx when space is limited

Tradeoff:

- `/mnt/*` on WSL2 usually uses 9P and can be slower than native ext4
- Pass 1 timings on `/mnt/d` can be slower than production-native ext4 runs

For production-representative timings, use `--bench-dir` on native ext4 if space permits.

## How To Run

From repo root:

```bash
# Default: small tier, Ollama provider
./tests/stress/run_benchmark.sh --tier small

# Structural-only run (no inference)
./tests/stress/run_benchmark.sh --tier small --provider pass1-only

# Hybrid provider mode
./tests/stress/run_benchmark.sh --tier small --provider tiered

# Full tier set
./tests/stress/run_benchmark.sh --tier all

# Reuse existing clones
./tests/stress/run_benchmark.sh --tier medium --skip-clone

# Custom bench directory
./tests/stress/run_benchmark.sh --tier small --bench-dir /home/rephu/bench-data
```

## Report Output

- Markdown report: `tests/stress/reports/aether_scale_report_<timestamp>.md`
- JSON report: `tests/stress/reports/aether_scale_report_<timestamp>.json`

Report includes:

- Pass 1 timing, peak RSS, symbol and edge counts
- Pass 2 sampled latency and success metrics
- Query latency percentiles
- Crash recovery summary
- Provider config snapshot and storage backend context

## Interpreting Provider Split

- Non-tiered providers: split is derived from `SIR_STORED ... provider=...` lines.
- Tiered provider: current logs expose only aggregated `tiered`, not cloud/local per symbol.
  Report marks this as unavailable until per-symbol routing telemetry is added.

## Adding Benchmark Repositories

Edit `tests/stress/repos.toml` and add a new `[[repo]]` block with:

- `name`
- `url`
- `commit` (`HEAD` allowed; resolved SHA is recorded)
- `tier` (`small`, `medium`, `large`)
- `language`
- `approx_symbols`
- `clone_size_mb`

No TOML parser dependency is used; keep field names and simple quoted values.

## Known Limitations

- Pass 2 metrics are sampled from first observed outcomes and may stop early.
- Tiered mode does not currently expose per-symbol cloud vs fallback usage.
- Shellcheck is optional and may be unavailable.
- Full benchmark runs can be long and resource intensive on large repos.

