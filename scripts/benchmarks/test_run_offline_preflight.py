"""Regression checks for the explicitly synthetic pre-flight evaluation harness."""

import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest

import evaluate_agent_trial as trial
import run_offline_preflight as offline


class OfflinePreflightTests(unittest.TestCase):
    def test_abbreviated_tool_commit_fails_before_creating_artifacts(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "never-created"
            with contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    offline.main(["--aetherd", "/unused-binary", "--output", str(output),
                                  "--tool-source-commit", "abc123"])
            self.assertEqual(error.exception.code, 2)
            self.assertFalse(output.exists())

    def test_task_echo_cannot_supply_its_own_required_evidence(self):
        task = {"task": "Find retired.rs and its reason", "required_evidence": ["retired.rs"]}
        score = offline.score_map({"context": "Task: " + task["task"]}, task)
        self.assertEqual(score["matched"], 0)
        self.assertEqual(score["missing_markers"], ["retired.rs"])
        truncated = offline.score_map({"context": "Task: Find retired.rs…"}, task)
        self.assertEqual(truncated["matched"], 0)
        score = offline.score_map({"context": "Task: " + task["task"] + "\nEvidence: `retired.rs` was deleted."}, task)
        self.assertTrue(score["all_required_evidence_present"])

    def test_missing_maps_cannot_be_replayed_as_success(self):
        manifest = {"repetitions": 1}
        corpus = {"initial_files": {}, "updated_files": {}, "deleted_files": [],
                  "initial_commit": "initial", "update_commit": "update", "notes": [],
                  "tasks": [{"id": "missing", "task": "missing map"}]}
        with self.assertRaises(KeyError):
            offline.make_replay(manifest, corpus, [], {"total_seconds": 10})

    def test_replay_accounts_full_measured_maintenance_in_both_aether_arms(self):
        manifest = {"schema_version": 1, "repository_commit": "a" * 40,
                    "starting_worktree_hash": "b" * 64, "protocol_hash": "c" * 64,
                    "evaluation_kind": "simulated_accounting_replay",
                    "simulation_limitations": offline.SIMULATION_NOTE,
                    "tasks": ["routing"], "arms": ["A", "B", "C"], "repetitions": 1,
                    "model_strata": ["none:deterministic-accounting-replay"]}
        corpus = {"initial_files": {}, "updated_files": {}, "deleted_files": [],
                  "initial_commit": "initial", "update_commit": "update", "notes": [],
                  "tasks": [{"id": "routing", "task": "route request"}]}
        runs = [{"task_id": "routing", "repetition": 1, "context": "supplied map",
                 "tool_result_json": "{\"context\":\"supplied map\"}",
                 "supplied_map_sha256": "d" * 64, "elapsed_seconds": 2}]
        rows = offline.make_replay(manifest, corpus, runs, {"total_seconds": 7})
        summary = trial.evaluate(manifest, rows)
        self.assertEqual(summary["evaluation_kind"], "simulated_accounting_replay")
        self.assertEqual(summary["arms"]["A"]["map_costs"]["map_maintenance_elapsed_seconds"]["total"], 0)
        self.assertEqual(summary["arms"]["B"]["map_costs"]["map_maintenance_elapsed_seconds"]["total"], 7)
        self.assertEqual(summary["arms"]["C"]["map_costs"]["map_maintenance_elapsed_seconds"]["total"], 7)
        self.assertEqual(summary["arms"]["C"]["map_costs"]["map_creation_elapsed_seconds"]["total"], 2)
        b = next(row for row in rows if row["arm"] == "B")
        c = next(row for row in rows if row["arm"] == "C")
        self.assertEqual(b["tool_assembly_elapsed_seconds"], 2)
        self.assertGreaterEqual(b["agent_elapsed_seconds"], 2)
        self.assertEqual(c["tool_assembly_elapsed_seconds"], 0)
        self.assertIn("not assessed", rows[0]["quality_measurement"])
        self.assertIn("estimated", rows[0]["token_measurement"])

    def test_corpus_contains_multifile_and_historical_tasks_in_three_languages(self):
        corpus = json.loads(offline.CORPUS.read_text(encoding="utf-8"))
        self.assertEqual(len(corpus["tasks"]), 6)
        self.assertEqual({task["category"] for task in corpus["tasks"]}, {"multi-file", "historical"})
        suffixes = {path.rsplit(".", 1)[-1] for path in corpus["initial_files"]}
        self.assertTrue({"rs", "py", "ts"} <= suffixes)
        self.assertEqual(len({task["id"] for task in corpus["tasks"]}), 6)

    def test_legacy_reanalysis_keeps_retry_costs_and_does_not_invent_c(self):
        rows = [{"agent": "fixture-agent", "arm": arm, "task": "task", "rep": 1,
                 "passed": arm == "none", "wall_seconds": 2,
                 "cost_usd_equivalent": .1, "usage": {"input_tokens": 10}}
                for arm in ("none", "aether")]
        rows.append({**rows[1], "passed": True, "wall_seconds": 3})
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "results.jsonl"
            path.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")
            summary = offline.summarize_legacy_ab(path)
        self.assertEqual(set(summary["arms"]), {"none", "aether"})
        arm = summary["arms"]["aether"]
        self.assertEqual(arm["attempts"], 2)
        self.assertEqual(arm["retained_cells"], 1)
        self.assertEqual(arm["retained_passed"], 1)
        self.assertEqual(arm["all_attempt_wall_seconds"], 5)
        self.assertEqual(arm["reported_usd_equivalent_sum"], .2)


if __name__ == "__main__":
    unittest.main()
