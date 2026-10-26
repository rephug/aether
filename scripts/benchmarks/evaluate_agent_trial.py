#!/usr/bin/env python3
"""Validate and summarize complete A/B/C trials. Reads JSON only; invokes no models."""

import argparse
from collections import Counter
import itertools
import json
import math
from pathlib import Path
import re
import statistics
import sys


ARMS = ("A", "B", "C")
STATUSES = ("completed", "error", "permission_denied")
MAP_CREATION_COSTS = (
    "map_creation_elapsed_seconds",
    "map_creation_tokens",
)
INDEX_MAINTENANCE_COSTS = (
    "map_maintenance_elapsed_seconds",
    "map_maintenance_tokens",
)
MAP_COSTS = MAP_CREATION_COSTS + INDEX_MAINTENANCE_COSTS
HASH_PROVENANCE = ("starting_worktree_hash", "protocol_hash")
COMPONENT_TIMES = ("db_elapsed_seconds", "model_generation_elapsed_seconds")
EVALUATION_KINDS = ("recorded_agent_trial", "simulated_accounting_replay")


class TrialError(ValueError):
    """The inputs cannot support a complete, comparable trial summary."""


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise TrialError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _load_json(text, location):
    try:
        return json.loads(text, object_pairs_hook=_unique_object)
    except (ValueError, TypeError) as error:
        raise TrialError(f"{location}: {error}") from error


def _object(value, location):
    if not isinstance(value, dict):
        raise TrialError(f"{location} must be an object")
    return value


def _labels(value, location):
    if (
        not isinstance(value, list)
        or not value
        or any(not isinstance(item, str) or not item.strip() for item in value)
        or len(set(value)) != len(value)
    ):
        raise TrialError(f"{location} must be a nonempty list of distinct nonempty strings")
    return value


def _number(value, location, integer=False):
    valid_type = type(value) is int if integer else type(value) in (int, float)
    if not valid_type or value < 0 or not math.isfinite(value):
        kind = "integer" if integer else "number"
        raise TrialError(f"{location} must be a finite nonnegative {kind}")
    return value


def _required(row, field, location):
    if field not in row:
        raise TrialError(f"{location}: missing {field}")
    return row[field]


def validate_manifest(manifest):
    manifest = _object(manifest, "manifest")
    if type(manifest.get("schema_version")) is not int or manifest["schema_version"] != 1:
        raise TrialError("manifest.schema_version must be 1")
    if manifest.get("evaluation_kind", "recorded_agent_trial") not in EVALUATION_KINDS:
        raise TrialError(f"manifest.evaluation_kind must be one of {EVALUATION_KINDS}")
    if manifest.get("evaluation_kind") == "simulated_accounting_replay":
        note = manifest.get("simulation_limitations")
        if not isinstance(note, str) or not note.strip():
            raise TrialError("a simulated accounting replay requires simulation_limitations")
    commit = manifest.get("repository_commit")
    if not isinstance(commit, str) or re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit) is None:
        raise TrialError("manifest.repository_commit must be a full lowercase Git commit hash")
    for field in HASH_PROVENANCE:
        value = _required(manifest, field, "manifest")
        if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None:
            raise TrialError(f"manifest.{field} must be a lowercase SHA-256 hash")
    tasks = _labels(manifest.get("tasks"), "manifest.tasks")
    strata = _labels(manifest.get("model_strata"), "manifest.model_strata")
    arms = _labels(manifest.get("arms"), "manifest.arms")
    if set(arms) != set(ARMS):
        raise TrialError("manifest.arms must declare A, B and C exactly once")
    repetitions = _number(manifest.get("repetitions"), "manifest.repetitions", integer=True)
    if repetitions == 0:
        raise TrialError("manifest.repetitions must be positive")
    return {
        (task, arm, repetition, stratum)
        for task, arm, repetition, stratum in itertools.product(
            tasks, ARMS, range(1, repetitions + 1), strata
        )
    }


def validate_results(manifest, rows):
    expected = validate_manifest(manifest)
    seen = set()
    validated = []
    for index, raw in enumerate(rows, start=1):
        location = f"result {index}"
        row = _object(raw, location)
        task = _required(row, "task_id", location)
        arm = _required(row, "arm", location)
        repetition = _number(
            _required(row, "repetition", location), f"{location}.repetition", integer=True
        )
        stratum = _required(row, "model_stratum", location)
        if any(not isinstance(value, str) for value in (task, arm, stratum)):
            raise TrialError(f"{location}: task_id, arm and model_stratum must be strings")
        cell = (task, arm, repetition, stratum)
        if cell not in expected:
            raise TrialError(f"{location}: undeclared cell {cell}")
        if cell in seen:
            raise TrialError(f"{location}: duplicate cell {cell}")
        seen.add(cell)
        if _required(row, "repository_commit", location) != manifest["repository_commit"]:
            raise TrialError(f"{location}: repository_commit differs from the manifest")
        for field in HASH_PROVENANCE:
            if _required(row, field, location) != manifest[field]:
                raise TrialError(f"{location}: {field} differs from the manifest")
        status = _required(row, "status", location)
        if status not in STATUSES:
            raise TrialError(f"{location}: status must be one of {STATUSES}; skips are invalid")
        correct = _required(row, "correct", location)
        if type(correct) is not bool:
            raise TrialError(f"{location}.correct must be a boolean")
        if status != "completed" and correct:
            raise TrialError(f"{location}: failed runs cannot be correct")
        for field in (
            "missed_dependencies", "regressions", "agent_tokens",
            "map_creation_tokens", "map_maintenance_tokens",
        ):
            _number(_required(row, field, location), f"{location}.{field}", integer=True)
        for field in (
            "agent_elapsed_seconds", "map_creation_elapsed_seconds",
            "map_maintenance_elapsed_seconds",
        ):
            _number(_required(row, field, location), f"{location}.{field}")
        if arm in ("A", "B") and any(row[field] != 0 for field in MAP_CREATION_COSTS):
            raise TrialError(f"{location}: A/B must explicitly record zero upfront map creation costs")
        if arm == "A" and any(row[field] != 0 for field in INDEX_MAINTENANCE_COSTS):
            raise TrialError(f"{location}: A must explicitly record zero Aether index costs")
        map_hash = row.get("map_hash")
        if arm == "C":
            map_hash = _required(row, "map_hash", location)
            # A failed map build is an observed failure, not a missing trial.
            # Explicit null records that no map was delivered; spent costs remain.
            if map_hash is None and status != "completed":
                pass
            elif not isinstance(map_hash, str) or re.fullmatch(r"[0-9a-f]{64}", map_hash) is None:
                raise TrialError(f"{location}: C requires a lowercase SHA-256 map_hash")
        elif map_hash is not None:
            raise TrialError(f"{location}: A/B cannot declare a supplied map_hash")
        for field in COMPONENT_TIMES:
            if field in row:
                _number(row[field], f"{location}.{field}")
        validated.append(row)
    missing = sorted(expected - seen)
    if missing:
        raise TrialError(f"missing {len(missing)} declared cells; first: {missing[0]}")
    return validated


def _measure(values):
    return {
        "total": sum(values),
        "mean": statistics.mean(values),
        "median": statistics.median(values),
    }


def _summarize_arm(rows):
    count = len(rows)
    passed = sum(row["correct"] for row in rows)
    elapsed = [
        row["agent_elapsed_seconds"] + row["map_creation_elapsed_seconds"]
        + row["map_maintenance_elapsed_seconds"]
        for row in rows
    ]
    tokens = [
        row["agent_tokens"] + row["map_creation_tokens"] + row["map_maintenance_tokens"]
        for row in rows
    ]
    components = {}
    for field in COMPONENT_TIMES:
        measured = [row[field] for row in rows if field in row]
        components[field] = {
            "measured_runs": len(measured),
            "unmeasured_runs": count - len(measured),
            "measurements": _measure(measured) if measured else None,
        }
    return {
        "runs": count,
        "correct_runs": passed,
        "correctness_rate": passed / count,
        "status_counts": {status: Counter(row["status"] for row in rows)[status] for status in STATUSES},
        "missed_dependencies": _measure([row["missed_dependencies"] for row in rows]),
        "regressions": _measure([row["regressions"] for row in rows]),
        "total_elapsed_seconds": _measure(elapsed),
        "total_tokens": _measure(tokens),
        "agent_elapsed_seconds": _measure([row["agent_elapsed_seconds"] for row in rows]),
        "agent_tokens": _measure([row["agent_tokens"] for row in rows]),
        "map_costs": {field: _measure([row[field] for row in rows]) for field in MAP_COSTS},
        "component_times": components,
    }


def evaluate(manifest, rows):
    rows = validate_results(manifest, rows)
    return {
        "schema_version": 1,
        "evaluation_kind": manifest.get("evaluation_kind", "recorded_agent_trial"),
        **(
            {"simulation_limitations": manifest["simulation_limitations"]}
            if manifest.get("evaluation_kind") == "simulated_accounting_replay"
            else {}
        ),
        "repository_commit": manifest["repository_commit"],
        **{field: manifest[field] for field in HASH_PROVENANCE},
        "cost_policy": (
            "Every B/C run pays full index construction/maintenance costs and every C run "
            "pays full upfront map creation costs; no amortization or double counting."
        ),
        "timing_note": (
            "Total elapsed is charged agent + map work per run, not whole-trial wall time. "
            "Optional database/model-generation measurements are separate, may overlap, "
            "and are not inferred from elapsed time."
        ),
        "expected_runs": len(rows),
        "arms": {arm: _summarize_arm([row for row in rows if row["arm"] == arm]) for arm in ARMS},
        "model_strata": {
            stratum: {
                arm: _summarize_arm([
                    row for row in rows if row["arm"] == arm and row["model_stratum"] == stratum
                ]) for arm in ARMS
            } for stratum in manifest["model_strata"]
        },
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("results", type=Path)
    args = parser.parse_args(argv)
    try:
        manifest = _load_json(args.manifest.read_text(encoding="utf-8"), str(args.manifest))
        rows = []
        for line_number, line in enumerate(args.results.read_text(encoding="utf-8").splitlines(), 1):
            if line.strip():
                rows.append(_load_json(line, f"{args.results}:{line_number}"))
        summary = evaluate(manifest, rows)
    except (OSError, TrialError) as error:
        print(f"invalid trial: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, indent=2, allow_nan=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
