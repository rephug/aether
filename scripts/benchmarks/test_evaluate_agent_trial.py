"""Offline fixtures for complete, fail-closed trial accounting."""

import contextlib
import copy
import io
import itertools
import json
from pathlib import Path
import tempfile
import unittest

import evaluate_agent_trial as trial


COMMIT = "a" * 40


def manifest_fixture():
    return {
        "schema_version": 1,
        "repository_commit": COMMIT,
        "starting_worktree_hash": "c" * 64,
        "protocol_hash": "d" * 64,
        "tasks": ["multi-file-routing", "historical-decision"],
        "arms": ["A", "B", "C"],
        "repetitions": 2,
        "model_strata": ["fixed-model:low", "fixed-model:standard"],
    }


def result_fixture():
    manifest = manifest_fixture()
    rows = []
    for task, arm, repetition, stratum in itertools.product(
        manifest["tasks"], manifest["arms"], range(1, 3), manifest["model_strata"]
    ):
        rows.append({
            "task_id": task,
            "arm": arm,
            "repetition": repetition,
            "model_stratum": stratum,
            "repository_commit": COMMIT,
            "starting_worktree_hash": manifest["starting_worktree_hash"],
            "protocol_hash": manifest["protocol_hash"],
            "status": "completed",
            "correct": True,
            "missed_dependencies": 0,
            "regressions": 0,
            "agent_elapsed_seconds": 10,
            "agent_tokens": 100,
            "map_creation_elapsed_seconds": 2 if arm == "C" else 0,
            "map_creation_tokens": 20 if arm == "C" else 0,
            "map_maintenance_elapsed_seconds": 1 if arm == "C" else 0,
            "map_maintenance_tokens": 5 if arm == "C" else 0,
            "map_hash": "b" * 64 if arm == "C" else None,
        })
    return rows


class TrialEvaluationTests(unittest.TestCase):
    def test_simulated_replay_cannot_lose_its_limitations_in_the_summary(self):
        manifest = manifest_fixture()
        manifest["evaluation_kind"] = "simulated_accounting_replay"
        with self.assertRaises(trial.TrialError):
            trial.evaluate(manifest, result_fixture())
        manifest["simulation_limitations"] = "No agents ran; outcomes are fixed fixtures."
        summary = trial.evaluate(manifest, result_fixture())
        self.assertEqual(summary["evaluation_kind"], "simulated_accounting_replay")
        self.assertEqual(summary["simulation_limitations"], manifest["simulation_limitations"])
        manifest["evaluation_kind"] = "unknown"
        with self.assertRaises(trial.TrialError):
            trial.evaluate(manifest, result_fixture())

    def test_complete_balanced_trial_charges_all_map_costs_per_run(self):
        report = trial.evaluate(manifest_fixture(), result_fixture())
        self.assertEqual(report["expected_runs"], 24)
        a = report["arms"]["A"]
        c = report["arms"]["C"]
        self.assertEqual(a["runs"], 8)
        self.assertEqual(a["total_tokens"]["total"], 800)
        self.assertEqual(c["total_tokens"]["total"], 1000)
        self.assertEqual(c["total_elapsed_seconds"]["total"], 104)
        self.assertEqual(c["map_costs"]["map_creation_tokens"]["total"], 160)
        self.assertEqual(report["model_strata"]["fixed-model:low"]["C"]["runs"], 4)

    def test_missing_duplicate_skipped_and_undeclared_cells_fail_closed(self):
        complete = result_fixture()
        duplicate = complete + [copy.deepcopy(complete[0])]
        skipped = copy.deepcopy(complete)
        skipped[0]["status"] = "skipped"
        undeclared = copy.deepcopy(complete)
        undeclared[0]["task_id"] = "easier-replacement"
        for rows in (complete[:-1], duplicate, skipped, undeclared):
            with self.subTest(rows=len(rows), first=rows[0]["status"]):
                with self.assertRaises(trial.TrialError):
                    trial.evaluate(manifest_fixture(), rows)

    def test_errors_and_permission_denials_remain_in_denominator_and_costs(self):
        rows = result_fixture()
        rows[0].update(status="permission_denied", correct=False, agent_tokens=15)
        rows[1].update(status="error", correct=False, agent_tokens=7)
        report = trial.evaluate(manifest_fixture(), rows)["arms"]["A"]
        self.assertEqual(report["runs"], 8)
        self.assertEqual(report["correct_runs"], 6)
        self.assertEqual(report["correctness_rate"], 0.75)
        self.assertEqual(report["status_counts"], {
            "completed": 6, "error": 1, "permission_denied": 1,
        })
        self.assertEqual(report["total_tokens"]["total"], 622)
        self.assertEqual(report["total_elapsed_seconds"]["total"], 80)

    def test_optional_tools_pay_index_preparation_even_before_the_agent_starts(self):
        rows = result_fixture()
        for row in rows:
            if row["arm"] == "B":
                row.update(map_maintenance_elapsed_seconds=7, map_maintenance_tokens=60)
        report = trial.evaluate(manifest_fixture(), rows)["arms"]["B"]
        self.assertEqual(report["total_elapsed_seconds"]["total"], 136)
        self.assertEqual(report["total_tokens"]["total"], 1280)
        self.assertEqual(report["map_costs"]["map_creation_tokens"]["total"], 0)
        self.assertEqual(report["map_costs"]["map_maintenance_tokens"]["total"], 480)

    def test_failed_map_creation_is_an_observed_failure_with_consumed_costs(self):
        rows = result_fixture()
        failed = next(row for row in rows if row["arm"] == "C")
        failed.update(status="error", correct=False, map_hash=None,
                      agent_elapsed_seconds=0, agent_tokens=0,
                      map_creation_tokens=12, map_creation_elapsed_seconds=3)
        report = trial.evaluate(manifest_fixture(), rows)["arms"]["C"]
        self.assertEqual(report["runs"], 8)
        self.assertEqual(report["correct_runs"], 7)
        self.assertEqual(report["status_counts"]["error"], 1)
        self.assertEqual(report["total_tokens"]["total"], 892)
        self.assertEqual(report["total_elapsed_seconds"]["total"], 95)

    def test_missing_costs_and_wrong_provenance_are_invalid(self):
        cases = [
            (0, "map_creation_tokens", None, True),
            (0, "repository_commit", "c" * 40, False),
            (0, "starting_worktree_hash", "e" * 64, False),
            (0, "protocol_hash", "e" * 64, False),
            (0, "starting_worktree_hash", None, True),
            (0, "protocol_hash", None, True),
            (0, "model_stratum", "unregistered-model", False),
            (0, "map_maintenance_tokens", 2, False),
            (4, "map_creation_tokens", 2, False),
            (8, "map_hash", None, False),
            (0, "correct", "true", False),
            (0, "agent_tokens", True, False),
            (0, "agent_elapsed_seconds", float("nan"), False),
            (0, "status", "permission_denied", False),
        ]
        for index, field, value, remove in cases:
            rows = result_fixture()
            # The first C row is after two arms * two repetitions * two strata.
            self.assertEqual(rows[8]["arm"], "C")
            if remove:
                del rows[index][field]
            else:
                rows[index][field] = value
            with self.subTest(field=field, value=value):
                with self.assertRaises(trial.TrialError):
                    trial.evaluate(manifest_fixture(), rows)

    def test_manifest_requires_full_worktree_and_protocol_hashes(self):
        for field in ("starting_worktree_hash", "protocol_hash"):
            for value in (None, "short", "E" * 64):
                manifest = manifest_fixture()
                if value is None:
                    del manifest[field]
                else:
                    manifest[field] = value
                with self.subTest(field=field, value=value):
                    with self.assertRaises(trial.TrialError):
                        trial.evaluate(manifest, result_fixture())

    def test_database_and_model_generation_times_are_separate_and_not_imputed(self):
        rows = result_fixture()
        rows[0].update(db_elapsed_seconds=0.5, model_generation_elapsed_seconds=4)
        report = trial.evaluate(manifest_fixture(), rows)["arms"]["A"]
        times = report["component_times"]
        self.assertEqual(times["db_elapsed_seconds"]["measured_runs"], 1)
        self.assertEqual(times["db_elapsed_seconds"]["unmeasured_runs"], 7)
        self.assertEqual(times["db_elapsed_seconds"]["measurements"]["mean"], 0.5)
        self.assertEqual(times["model_generation_elapsed_seconds"]["measurements"]["total"], 4)
        self.assertEqual(report["total_elapsed_seconds"]["total"], 80)
        unmeasured = trial.evaluate(manifest_fixture(), rows)["arms"]["B"]["component_times"]
        self.assertIsNone(unmeasured["db_elapsed_seconds"]["measurements"])

    def test_cli_invalid_input_emits_no_partial_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            manifest = path / "manifest.json"
            results = path / "results.jsonl"
            manifest.write_text(json.dumps(manifest_fixture()), encoding="utf-8")
            results.write_text(json.dumps(result_fixture()[0]) + "\n", encoding="utf-8")
            stdout = io.StringIO()
            stderr = io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = trial.main([str(manifest), str(results)])
            self.assertEqual(code, 1)
            self.assertEqual(stdout.getvalue(), "")
            self.assertIn("missing 23 declared cells", stderr.getvalue())

    def test_duplicate_json_keys_are_rejected(self):
        with self.assertRaises(trial.TrialError):
            trial._load_json('{"status":"error","status":"completed"}', "fixture")


if __name__ == "__main__":
    unittest.main()
