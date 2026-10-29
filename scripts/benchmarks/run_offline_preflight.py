#!/usr/bin/env python3
"""Measure fixture pre-flight maps, then exercise explicitly simulated A/B/C accounting.

Runs only an existing local aetherd binary's structural index, remember and preflight
commands against a newly-created disposable fixture. Does not launch coding agents,
generate SIRs, change live stores, install models or request embeddings.
"""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

import evaluate_agent_trial as trial


HERE = Path(__file__).resolve().parent
CORPUS = HERE / "fixtures" / "preflight_corpus.json"
SIMULATION_NOTE = (
    "No coding agent, model generation, patch execution or live-provider trial ran. "
    "A/B/C outcomes are correct-by-construction accounting fixtures, not measured "
    "coding correctness, dependency coverage or regressions. Agent times measure "
    "Python transcript construction/hash overhead; B also replays the measured map "
    "assembly cost as an in-session optional tool wait. Agent tokens are UTF-8-byte/4 "
    "estimates of synthetic transcripts, not tokenizer/provider usage. MCP schema/client "
    "transport overhead is unmeasured because no MCP client or agent session ran. Actual map "
    "fidelity and process-inclusive assembly times are reported separately."
)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def digest(value):
    return hashlib.sha256(value).hexdigest()


def file_digest(path):
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(block)
    return hasher.hexdigest()


def estimated_tokens(text):
    return math.ceil(len(text.encode()) / 4)


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def hardware_metadata():
    metadata = {"platform": platform.platform(), "python": platform.python_version(),
                "logical_cpu_count": os.cpu_count()}
    try:
        cpu_lines = Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines()
        metadata["cpu_model"] = next(line.split(":", 1)[1].strip() for line in cpu_lines if line.startswith("model name"))
        memory_lines = Path("/proc/meminfo").read_text(encoding="utf-8").splitlines()
        metadata["memory_total"] = next(line.split(":", 1)[1].strip() for line in memory_lines if line.startswith("MemTotal:"))
    except (OSError, StopIteration):
        pass
    return metadata


def record_tool_source(output, expected_commit):
    repository = HERE.parent.parent
    observed_commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repository, text=True).strip()
    if observed_commit != expected_commit:
        raise ValueError("tool source commit differs from the checkout HEAD; preserve and identify the actual source")
    tracked = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=repository,
    ).split(b"\0")
    hashes = {}
    for raw in tracked:
        if raw:
            name = os.fsdecode(raw)
            if name.startswith(".aether/") or "/.aether/" in name:
                continue
            path = repository / name
            if path.is_file():
                hashes[name] = digest(path.read_bytes())
    diff = subprocess.check_output(["git", "diff", "--no-ext-diff", "--no-textconv", "--binary", "HEAD"], cwd=repository)
    (output / "implementation-source.diff").write_bytes(diff)
    metadata = {"repository_commit": observed_commit, "source_hash": digest(canonical(hashes)), "file_hashes": hashes,
                "snapshot_format": "SHA-256 of canonical compact sorted JSON mapping tracked and nonignored untracked relative paths to SHA-256 file bytes",
                "git_diff_sha256": digest(diff)}
    write_json(output / "implementation-source-snapshot.json", metadata)
    return metadata["source_hash"]


def evidence_text(payload, task):
    # Never give credit to a requested path/reason merely repeated in the task header.
    # Structured evidence fields are preferred; a context-only implementation remains
    # inspectable with the verbatim task removed from the rendered map.
    if not isinstance(payload, dict) or not isinstance(payload.get("context"), str):
        raise ValueError("preflight must return a JSON object containing context")
    return "\n".join(
        line for line in payload["context"].replace(task, "").splitlines()
        if not line.lstrip().startswith("Task:")
    )


def score_map(payload, task):
    text = evidence_text(payload, task["task"])
    required = task["required_evidence"]
    matched = [marker for marker in required if marker in text]
    missed = [marker for marker in required if marker not in text]
    return {
        "required": len(required), "matched": len(matched),
        "required_evidence_recall": len(matched) / len(required),
        "all_required_evidence_present": not missed,
        "matched_markers": matched, "missing_markers": missed,
    }


def run_command(command, workspace, receipts, *, env=None):
    started = time.perf_counter()
    completed = subprocess.run(
        [str(argument) for argument in command], cwd=workspace,
        text=True, capture_output=True, timeout=120, env=env,
    )
    elapsed = time.perf_counter() - started
    receipts.append({
        "command": [str(argument).replace(str(workspace), "<fixture>") for argument in command],
        "elapsed_seconds": elapsed, "exit_code": completed.returncode,
        "stdout": completed.stdout, "stderr": completed.stderr,
    })
    if completed.returncode:
        raise RuntimeError(f"fixture command failed ({completed.returncode}): {command[0]}")
    return completed.stdout, elapsed


def write_sources(workspace, sources):
    for name, source in sources.items():
        path = workspace / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding="utf-8")


def expected_snapshot(corpus):
    sources = {**corpus["initial_files"], **corpus["updated_files"]}
    for name in corpus["deleted_files"]:
        sources.pop(name, None)
    sources[".aether/config.toml"] = corpus["configuration"]
    sources[".gitignore"] = ".aether/\n"
    return {name: digest(text.encode()) for name, text in sorted(sources.items())}


def build_fixture(binary, workspace, corpus, receipts, cli_flags=()):
    # All mutations target this new temporary directory. Git has no remote; known
    # author dates make the two synthetic revision identities reproducible.
    environment = dict(os.environ, GIT_AUTHOR_NAME="Aether offline fixture",
                       GIT_AUTHOR_EMAIL="fixture@example.invalid",
                       GIT_COMMITTER_NAME="Aether offline fixture",
                       GIT_COMMITTER_EMAIL="fixture@example.invalid")
    (workspace / ".aether").mkdir()
    (workspace / ".aether/config.toml").write_text(corpus["configuration"], encoding="utf-8")
    (workspace / ".gitignore").write_text(".aether/\n", encoding="utf-8")
    write_sources(workspace, corpus["initial_files"])
    run_command(["git", "init", "-b", "fixture"], workspace, receipts)
    run_command(["git", "add", "."], workspace, receipts)
    environment.update(GIT_AUTHOR_DATE="2026-10-01T12:00:00Z", GIT_COMMITTER_DATE="2026-10-01T12:00:00Z")
    run_command(["git", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "commit", "-m", corpus["initial_commit"]], workspace, receipts, env=environment)
    _, initial_index = run_command([binary, *cli_flags, "--workspace", workspace, "--index-once"], workspace, receipts)
    write_sources(workspace, corpus["updated_files"])
    for name in corpus["deleted_files"]:
        (workspace / name).unlink()
    run_command(["git", "add", "."], workspace, receipts)
    environment.update(GIT_AUTHOR_DATE="2026-10-02T12:00:00Z", GIT_COMMITTER_DATE="2026-10-02T12:00:00Z")
    run_command(["git", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "commit", "-m", corpus["update_commit"]], workspace, receipts, env=environment)
    _, update_index = run_command([binary, *cli_flags, "--workspace", workspace, "--index-once"], workspace, receipts)
    notes_elapsed = 0.0
    for note in corpus["notes"]:
        _, elapsed = run_command([binary, *cli_flags, "--workspace", workspace, "remember", note], workspace, receipts)
        notes_elapsed += elapsed
    head, _ = run_command(["git", "rev-parse", "HEAD"], workspace, receipts)
    return {"initial_structural_index_seconds": initial_index,
            "updated_structural_index_seconds": update_index,
            "recorded_note_ingestion_seconds": notes_elapsed,
            "total_seconds": initial_index + update_index + notes_elapsed,
            "fixture_head": head.strip()}


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False, allow_nan=False) + "\n", encoding="utf-8")


def summarize_legacy_ab(path):
    """Keep old live-provider records descriptive and separate from this replay.

    Field semantics differ between agent families, so never add cached/reasoning
    counters into an invented universal token total. Retain every attempt's costs.
    """
    raw = path.read_bytes()
    rows = [json.loads(line) for line in raw.splitlines() if line.strip()]
    cells = {}
    for row in rows:
        key = (row["agent"], row["arm"], row["task"], row["rep"])
        cells.setdefault(key, []).append(row)
    arms = {}
    for arm in ("none", "aether"):
        groups = [attempts for key, attempts in cells.items() if key[1] == arm]
        retained = [attempts[-1] for attempts in groups]
        attempts = [row for group in groups for row in group]
        by_agent = {}
        for agent in sorted({row["agent"] for row in attempts}):
            agent_rows = [row for row in attempts if row["agent"] == agent]
            counters = {}
            for row in agent_rows:
                for name, value in (row.get("usage") or {}).items():
                    if type(value) in (int, float):
                        counters[name] = counters.get(name, 0) + value
            by_agent[agent] = counters
        wall = [row["wall_seconds"] for row in retained]
        arms[arm] = {
            "retained_cells": len(retained), "attempts": len(attempts),
            "retained_passed": sum(row["passed"] is True for row in retained),
            "retained_correctness_rate": sum(row["passed"] is True for row in retained) / len(retained),
            "all_attempt_wall_seconds": sum(row["wall_seconds"] for row in attempts),
            "retained_wall_seconds_p50": statistics.median(wall),
            "retained_wall_seconds_p95": percentile(wall, .95),
            "reported_usd_equivalent_sum": sum(row.get("cost_usd_equivalent") or 0 for row in attempts),
            "usage_counters_by_agent_as_recorded": by_agent,
        }
    return {
        "evaluation_kind": "historical-live-provider-ab-record-reanalysis",
        "source_sha256": digest(raw), "raw_records": len(rows), "unique_cells": len(cells),
        "tasks": sorted({row["task"] for row in rows}),
        "repetitions": sorted({row["rep"] for row in rows}),
        "agent_labels_and_recorded_model_names": {
            agent: sorted({name for row in rows if row["agent"] == agent
                           for name in (row.get("model_usage") or {})})
            for agent in sorted({row["agent"] for row in rows})
        },
        "retention_policy": "Latest recorded attempt per agent/arm/task/rep supplies outcome; ALL attempts retain reported costs/time.",
        "arms": arms,
        "limitations": "Existing eight-task A/B study only; no C arm, no complete index/map maintenance accounting, heterogeneous usage counters, permission/tool-discovery confounds, no independent regression assessment. USD equivalent is the harness's price-based estimate, not an invoice. Cannot establish current-branch or upfront-map benefit.",
    }


def make_replay(manifest, corpus, map_runs, maintenance):
    rows = []
    by_cell = {(run["task_id"], run["repetition"]): run for run in map_runs}
    ordinary = canonical({"sources": corpus["initial_files"], "updates": corpus["updated_files"],
                          "deleted": corpus["deleted_files"], "history": [corpus["initial_commit"], corpus["update_commit"]],
                          "notes": corpus["notes"]}).decode()
    for task in corpus["tasks"]:
        for repetition in range(1, manifest["repetitions"] + 1):
            run = by_cell[(task["id"], repetition)]
            for arm in ("A", "B", "C"):
                started = time.perf_counter()
                transcript = task["task"] + "\n" + (
                    run["context"] if arm == "C" else ordinary
                )
                if arm == "B":
                    transcript += "\nOptional CLI tool fixture:\n" + run["tool_result_json"]
                transcript_hash = digest(transcript.encode())
                elapsed = time.perf_counter() - started
                optional_tool_elapsed = run["elapsed_seconds"] if arm == "B" else 0
                rows.append({
                    "task_id": task["id"], "arm": arm, "repetition": repetition,
                    "model_stratum": "none:deterministic-accounting-replay",
                    **{key: manifest[key] for key in ("repository_commit", "starting_worktree_hash", "protocol_hash")},
                    "status": "completed", "correct": True,
                    "missed_dependencies": 0, "regressions": 0,
                    "agent_elapsed_seconds": elapsed + optional_tool_elapsed,
                    "agent_tokens": estimated_tokens(transcript),
                    "map_creation_elapsed_seconds": run["elapsed_seconds"] if arm == "C" else 0,
                    "map_creation_tokens": 0,
                    "map_maintenance_elapsed_seconds": maintenance["total_seconds"] if arm in ("B", "C") else 0,
                    "map_maintenance_tokens": 0,
                    "map_hash": run["supplied_map_sha256"] if arm == "C" else None,
                    "model_generation_elapsed_seconds": 0,
                    "tool_assembly_elapsed_seconds": optional_tool_elapsed,
                    "transcript_sha256": transcript_hash,
                    "quality_measurement": "assumed-complete accounting fixture; not assessed coding outcomes",
                    "token_measurement": "estimated ceil(UTF-8 bytes / 4); no model usage",
                    "agent_latency_measurement": "measured Python transcript assembly/hash plus replayed actual map assembly wall time in B; no live agent",
                })
    return rows


def write_report(output, fidelity, summary, map_runs):
    lines = [
        "# Offline pre-flight evidence and A/B/C accounting replay", "",
        "This run measured the actual local Aether map service on a small synthetic fixture. "
        "It did not run coding agents. The A/B/C records are a separately labelled accounting simulation.", "",
        f"Fixture commit: `{fidelity['repository_commit']}`. Tool source commit: `{fidelity['tool_source_commit']}` "
        "plus the retained dirty-source snapshot/diff. "
        f"Binary SHA-256: `{fidelity['aether_binary_sha256']}`.", "",
        "## Measured map fidelity", "",
        f"Six frozen tasks (three multi-file, three historical), three languages, "
        f"{fidelity['maps']} map executions. Required-marker recall: "
        f"**{fidelity['matched_markers']}/{fidelity['required_markers']} "
        f"({fidelity['required_evidence_recall']:.1%})**; "
        f"{fidelity['maps_with_all_required_evidence']}/{fidelity['maps']} maps contain every required marker. "
        "Task echoes receive no credit. Repeated executions are not additional independent tasks.", "",
        "| Task | First-run required evidence | Missing markers |", "| --- | ---: | --- |",
    ]
    for run in map_runs:
        if run["repetition"] == 1:
            missing = ", ".join(f"`{marker}`" for marker in run["missing_markers"]) or "none"
            lines.append(f"| {run['task_id']} | {run['matched']}/{run['required']} | {missing} |")
    latency = fidelity["assembly_process_seconds"]
    maintenance = fidelity["maintenance"]
    lines.extend([
        "", f"Map process wall latency: p50 **{latency['p50'] * 1000:.2f} ms**, "
        f"p95 **{latency['p95'] * 1000:.2f} ms**. These include executable startup, Git, "
        "SQLite access, assembly and serialization. Database latency is not isolated; "
        "there is no model-generation latency. OS caches were uncontrolled.", "",
        f"Measured fixture preparation: initial structural index {maintenance['initial_structural_index_seconds']:.4f}s; "
        f"updated structural index {maintenance['updated_structural_index_seconds']:.4f}s; "
        f"two note writes {maintenance['recorded_note_ingestion_seconds']:.4f}s; "
        f"total **{maintenance['total_seconds']:.4f}s**. No amortization is applied in the replay.", "",
        "Precision, citation correctness, actual code-change correctness and regressions were not graded. "
        "Marker recall is a diagnostic for this fixture and cannot establish production map quality.", "",
        "## Simulated A/B/C accounting", "", SIMULATION_NOTE, "",
        "| Arm | Replay cells | Assumed correct | Estimated transcript tokens (total) | Charged fixture seconds (total) |",
        "| --- | ---: | ---: | ---: | ---: |",
    ])
    for arm, values in summary["arms"].items():
        lines.append(f"| {arm} | {values['runs']} | {values['correct_runs']} | "
                     f"{values['total_tokens']['total']} | {values['total_elapsed_seconds']['total']:.4f} |")
    lines.extend([
        "", "A = scripted ordinary-source transcript; B = the same transcript plus optional-map tool text; "
        "C = supplied-map transcript. This fixed policy intentionally supplies no agent-behavior model. "
        "Outcome fields are fixture assumptions, so this table supports no correctness or token-saving claim.", "",
        "Token values estimate `ceil(UTF-8 bytes / 4)`. They are neither tokenizer counts nor charged usage. "
        "Both B and C pay the full measured index/update/note cost on every cell. B replays measured map "
        "assembly inside agent elapsed time as an optional tool call; C pays that assembly as upfront "
        "map creation. Each is charged once. Map/index model-token costs are zero because their paths used no models. "
        "Measured provider calls/tokens/dollars: **0/0/$0**. CPU, electricity and storage remain unpriced.", "",
    ])
    legacy_path = output / "legacy-ab-summary.json"
    if legacy_path.exists():
        legacy = json.loads(legacy_path.read_text(encoding="utf-8"))
        lines.extend(["## Existing live-provider A/B records, separately reanalyzed", "",
                      f"{legacy['raw_records']} attempts, {legacy['unique_cells']} unique cells. "
                      "Latest attempts provide outcomes; every attempt retains its time and reported cost.", "",
                      "| Existing arm | Passed / retained | All-attempt wall seconds | Harness USD-equivalent estimate |",
                      "| --- | ---: | ---: | ---: |"])
        for arm, values in legacy["arms"].items():
            lines.append(f"| {arm} | {values['retained_passed']}/{values['retained_cells']} | "
                         f"{values['all_attempt_wall_seconds']:.1f} | ${values['reported_usd_equivalent_sum']:.6f} |")
        lines.extend(["", legacy["limitations"], ""])
    lines.extend(["## Reproduction and limits", "",
                  "The checked-in corpus/config and runner create an isolated disposable repository; no active "
                  "stores are inputs. `protocol.json`, `source-snapshot.json`, binary/corpus hashes and command "
                  "receipts identify this run. `maps/`, `map-runs.json` and `map-fidelity-summary.json` retain "
                  "measured evidence. `manifest.json`, `results.jsonl` and `accounting-summary.json` retain "
                  "the simulated accounting dataset. See the benchmark README for the command.", ""])
    (output / "REPORT.md").write_text("\n".join(lines), encoding="utf-8")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--aetherd", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--tool-source-commit", required=True)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--legacy-results", type=Path)
    args = parser.parse_args(argv)
    if args.repetitions < 1:
        parser.error("--repetitions must be positive")
    if re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", args.tool_source_commit) is None:
        parser.error("--tool-source-commit must be a full lowercase Git commit hash")
    input_binary = args.aetherd.resolve(strict=True)
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("output must be a new or empty artifact directory; preserve previous run evidence")
    output.mkdir(parents=True, exist_ok=True)
    # Never accept a pre-existing workspace/store as a test input.
    if (output / ".aether").exists() or (output / ".git").exists():
        parser.error("output must be an artifact directory, not an existing workspace or store")
    # Cargo in another lane may replace its output during this run. Execute only
    # this captured test artifact, not a moving shared-target path.
    binary = output / "aetherd.bin"
    shutil.copy2(input_binary, binary)
    tool_source_hash = record_tool_source(output, args.tool_source_commit)
    receipts = []
    help_text, _ = run_command([binary, "--help"], output, receipts)
    cli_flags = ["--no-dashboard"] if "--no-dashboard" in help_text else []
    corpus_bytes = CORPUS.read_bytes()
    corpus = json.loads(corpus_bytes)
    source_hashes = {name: digest((HERE / name).read_bytes()) for name in
                     ("run_offline_preflight.py", "evaluate_agent_trial.py", "fixtures/preflight_corpus.json")}
    protocol = {"schema_version": 1, "kind": "structural-map-fidelity-and-simulated-accounting",
                "repetitions": args.repetitions, "budget": 2500, "arms": ["A", "B", "C"],
                "corpus_sha256": digest(corpus_bytes), "source_hashes": source_hashes,
                "tool_source_commit": args.tool_source_commit,
                "tool_source_hash": tool_source_hash,
                "binary_sha256": file_digest(binary),
                "cli_global_flags": cli_flags,
                "dashboard": "explicitly disabled" if cli_flags else "dashboard flag absent from this build",
                "scoring": "Case-sensitive required-marker presence in rendered context after stripping verbatim task echo. No patch correctness grading.",
                "simulation_limitations": SIMULATION_NOTE}
    write_json(output / "protocol.json", protocol)
    snapshot = expected_snapshot(corpus)
    write_json(output / "source-snapshot.json", {
        "format": "SHA-256 of canonical compact sorted JSON mapping relative UTF-8 paths to SHA-256 file-byte hashes",
        "included": "All final fixture source, test, .gitignore and .aether/config.toml files",
        "excluded": ".git metadata and generated .aether stores/caches",
        "files": snapshot,
    })
    manifest = {"schema_version": 1,
                "starting_worktree_hash": digest(canonical(snapshot)),
                "protocol_hash": digest((output / "protocol.json").read_bytes()),
                "evaluation_kind": "simulated_accounting_replay", "simulation_limitations": SIMULATION_NOTE,
                "tasks": [task["id"] for task in corpus["tasks"]], "arms": ["A", "B", "C"],
                "repetitions": args.repetitions,
                "model_strata": ["none:deterministic-accounting-replay"]}
    if args.legacy_results:
        write_json(output / "legacy-ab-summary.json", summarize_legacy_ab(args.legacy_results))
    map_runs = []
    started = time.perf_counter()
    try:
        with tempfile.TemporaryDirectory(prefix="aether-offline-fixture-", dir=output) as directory:
            workspace = Path(directory)
            maintenance = build_fixture(binary, workspace, corpus, receipts, cli_flags)
            manifest["repository_commit"] = maintenance["fixture_head"]
            trial.validate_manifest(manifest)
            write_json(output / "manifest.json", manifest)
            observed = {name: digest((workspace / name).read_bytes()) for name in snapshot}
            if observed != snapshot:
                raise ValueError("fixture starting source snapshot differs from frozen manifest")
            (output / "maps").mkdir(exist_ok=True)
            for repetition in range(1, args.repetitions + 1):
                # Rotate task order deterministically to avoid always giving the same
                # task the first process/cache position. This is not a cold-cache test.
                offset = (repetition - 1) % len(corpus["tasks"])
                tasks = corpus["tasks"][offset:] + corpus["tasks"][:offset]
                for task in tasks:
                    stdout, elapsed = run_command([
                        binary, *cli_flags, "--workspace", workspace, "preflight", task["task"],
                        "--format", "json", "--max-tokens", "2500", "--scope", task["scope"],
                    ], workspace, receipts)
                    payload = json.loads(stdout)
                    score = score_map(payload, task)
                    context = payload["context"]
                    artifact = f"maps/{task['id']}-{repetition}.json"
                    write_json(output / artifact, payload)
                    map_runs.append({"task_id": task["id"], "category": task["category"],
                                     "repetition": repetition, "artifact": artifact,
                                     "elapsed_seconds": elapsed, "context": context,
                                     "tool_result_json": stdout,
                                     "supplied_map_sha256": digest(context.encode()),
                                     "estimated_context_tokens": estimated_tokens(context), **score})
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        write_json(output / "command-receipts.json", receipts)
        write_json(output / "failure.json", {"error": str(error), "partial_map_runs": map_runs})
        print(f"offline evaluation failed: {error}", file=sys.stderr)
        return 1
    write_json(output / "command-receipts.json", receipts)
    write_json(output / "map-runs.json", map_runs)
    rows = make_replay(manifest, corpus, map_runs, maintenance)
    (output / "results.jsonl").write_text("".join(json.dumps(row, sort_keys=True) + "\n" for row in rows), encoding="utf-8")
    summary = trial.evaluate(manifest, rows)
    write_json(output / "accounting-summary.json", summary)
    durations = [run["elapsed_seconds"] for run in map_runs]
    required = sum(run["required"] for run in map_runs)
    matched = sum(run["matched"] for run in map_runs)
    fidelity = {
        "evaluation_kind": "measured-synthetic-fixture-map-fidelity",
        "dataset": str(CORPUS.relative_to(HERE)), "corpus_sha256": digest(corpus_bytes),
        "aether_binary_sha256": file_digest(binary), "repository_commit": maintenance["fixture_head"],
        "tool_source_commit": args.tool_source_commit, "tool_source_hash": tool_source_hash,
        "hardware": hardware_metadata(), "maps": len(map_runs), "tasks": len(corpus["tasks"]),
        "provider_calls": 0, "provider_tokens": 0, "provider_cost_usd": 0,
        "cost_limitations": "No provider called. CPU/electricity/storage costs unpriced. Text tokens are byte/4 estimates, not charged usage. B includes actual CLI map JSON; MCP discovery/schema/client transport overhead unmeasured.",
        "maintenance": maintenance, "required_markers": required, "matched_markers": matched,
        "required_evidence_recall": matched / required,
        "maps_with_all_required_evidence": sum(run["all_required_evidence_present"] for run in map_runs),
        "assembly_process_seconds": {"p50": statistics.median(durations), "p95": percentile(durations, .95),
                                     "min": min(durations), "max": max(durations), "total": sum(durations)},
        "quality_limits": "Marker recall measures only this six-task synthetic fixture. Precision, cited-edge accuracy, change correctness and regressions were not assessed. Repetitions repeat the same six tasks; they are not independent quality samples.",
        "latency_limits": "Wall times include executable startup, Git subprocesses, SQLite reads, rendering and serialization; do not isolate database latency. OS caches uncontrolled; no model-generation latency.",
        "trial_wall_seconds": time.perf_counter() - started,
    }
    write_json(output / "map-fidelity-summary.json", fidelity)
    write_report(output, fidelity, summary, map_runs)
    print(json.dumps(fidelity, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
