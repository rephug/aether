"""Exercise a real Aether binary on disposable synthetic workspaces, without APIs.

Keeps fixtures and logs for inspection. Never opens an existing catalog. Linux runs
also verify that the daemon actually registered native inotify watches.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import sqlite3
import subprocess
import time
import traceback


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--backend", choices=("sqlite", "surreal"), default="sqlite")
    parser.add_argument("--timeout", type=float, default=15)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    help_text = subprocess.check_output([str(binary), "--help"], text=True)
    runtime_options = ["--no-dashboard"] if "--no-dashboard" in help_text else []
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    workspace = output / "workspace"
    workspace.mkdir()
    daemon = None
    daemon_log = None
    report = {
        "binary": str(binary), "backend": args.backend,
        "binary_sha256": hashlib.file_digest(binary.open("rb"), "sha256").hexdigest(),
        "output": str(output), "platform": platform.platform(),
        "provider": "mock", "embeddings": False, "cases": [],
    }
    started = time.monotonic()

    def save():
        report["elapsed_seconds"] = round(time.monotonic() - started, 3)
        (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")

    def write(path, source):
        dest = workspace / path
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(source)

    def git(*argv):
        return subprocess.check_output([
            "git", "-c", "user.name=Aether synthetic test",
            "-c", "user.email=aether-test@example.invalid", *argv,
        ], cwd=workspace, stderr=subprocess.STDOUT, text=True).strip()

    def sql(query, params=()):
        with sqlite3.connect((workspace / ".aether/meta.sqlite").as_uri() + "?mode=ro", uri=True) as conn:
            return conn.execute(query, params).fetchall()

    def rows_for(path):
        return sql("SELECT id,qualified_name FROM symbols WHERE file_path=? ORDER BY id", (path,))

    def targets(path):
        return sql("SELECT target_qualified_name FROM symbol_edges WHERE file_path=? AND edge_kind='calls' ORDER BY 1", (path,))

    def wait_for(condition, timeout=None):
        end = time.monotonic() + (timeout or args.timeout)
        last_error = None
        while time.monotonic() < end:
            if daemon is not None and daemon.poll() is not None:
                raise AssertionError(f"daemon exited unexpectedly: {daemon.returncode}")
            try:
                value = condition()
                if value:
                    return value
            except (sqlite3.Error, FileNotFoundError) as exc:
                last_error = str(exc)
            time.sleep(0.1)
        raise AssertionError(f"condition timed out after {timeout or args.timeout}s; last error: {last_error}")

    def case(name, action):
        before = time.monotonic()
        result = {"name": name}
        try:
            detail = action()
            result.update(result="passed", detail=detail)
        except Exception as exc:
            result.update(result="failed", error=str(exc), traceback=traceback.format_exc())
        result["seconds"] = round(time.monotonic() - before, 3)
        report["cases"].append(result)
        save()
        print(json.dumps(result), flush=True)
        return result["result"] == "passed"

    def run_cli(name, *argv):
        with (output / f"{name}.log").open("w") as log:
            proc = subprocess.run([str(binary), "--workspace", str(workspace), *runtime_options, *argv],
                                  stdout=log, stderr=subprocess.STDOUT, timeout=240)
        assert proc.returncode == 0, f"{name} exited {proc.returncode}; see log"

    def stop():
        nonlocal daemon, daemon_log
        if daemon is not None:
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait(timeout=10)
            report["daemon_stopped"] = daemon.poll() is not None
            daemon = None
        if daemon_log is not None:
            daemon_log.close()
            daemon_log = None

    try:
        write(".gitignore", ".aether/\ntarget/\nnode_modules/\nignored.rs\n")
        write(".aether/config.toml", f'''[inference]
provider = "mock"
[storage]
graph_backend = "{args.backend}"
mirror_sir_files = true
[embeddings]
enabled = false
vector_backend = "sqlite"
[watcher]
git_debounce_secs = 0.2
realtime_provider = "mock"
[sir_quality]
triage_pass = true
deep_pass = true
triage_provider = "mock"
deep_provider = "mock"
triage_priority_threshold = 0.0
deep_priority_threshold = 0.0
triage_confidence_threshold = 1.0
deep_confidence_threshold = 1.0
triage_max_symbols = 4
deep_max_symbols = 4
''')
        write("src/lib.rs", "pub fn helper() -> i32 { 1 }\npub fn greet() -> i32 { helper() }\n")
        write("tracked/deep/present.rs", "pub fn tracked_directory_symbol() {}\n")
        write("web/app.ts", "export function greet(): number { return 1; }\n")
        write("pkg/api/__init__.py", "from .old import f\n")
        write("pkg/api/old.py", "def f():\n    return 1\n")
        write("pkg/api/new.py", "def f():\n    return 2\n")
        write("app.py", "from pkg.api import f\n\ndef run():\n    return f()\n")
        write("service/main.go", "package service\nfunc Greet() int { return 1 }\n")
        write("java/App.java", "package demo;\npublic class App { public int greet() { return 1; } }\n")
        write("dotnet/App.cs", "namespace Demo;\npublic class App { public int Greet() { return 1; } }\n")
        write("target/hidden.rs", "fn must_not_index_target() {}\n")
        write("node_modules/hidden.ts", "function must_not_index_modules() {}\n")
        write("ignored.rs", "fn must_not_index_gitignore() {}\n")
        git("init", "-b", "main")
        git("add", ".")
        git("commit", "-m", "synthetic baseline")

        def full_scan():
            run_cli("initial-full", "--index-once", "--full")
            languages = {row[0] for row in sql("SELECT DISTINCT language FROM symbols")}
            assert languages == {"rust", "typescript", "python", "go", "java", "csharp"}, languages
            assert sql("SELECT count(*) FROM symbols WHERE qualified_name LIKE '%must_not_index%'") == [(0,)]
            assert ("pkg.api.old::f",) in targets("app.py"), targets("app.py")
            assert sql("SELECT count(*) FROM symbols s LEFT JOIN sir r ON s.id=r.id WHERE r.id IS NULL") == [(0,)]
            assert sql("SELECT DISTINCT provider FROM sir") == [("mock",)]
            assert sql("PRAGMA quick_check") == [("ok",)]
            return {"symbols": sql("SELECT count(*) FROM symbols")[0][0], "languages": sorted(languages),
                    "sir_rows": sql("SELECT count(*) FROM sir")[0][0]}

        if not case("full_scan_six_languages_and_mock_SIR", full_scan):
            return 1

        def start():
            nonlocal daemon, daemon_log
            daemon_log = (output / "daemon.log").open("w")
            daemon = subprocess.Popen([str(binary), "--workspace", str(workspace), *runtime_options,
                                       "--print-events", "--print-sir", "--debounce-ms", "100"],
                                      stdout=daemon_log, stderr=subprocess.STDOUT, start_new_session=True)
            report["daemon_pid"] = daemon.pid
            wait_for(lambda: "started semantic workers" in (output / "daemon.log").read_text(), 120)
            if platform.system() == "Linux":
                def inotify():
                    descriptors = []
                    for fd in Path(f"/proc/{daemon.pid}/fd").iterdir():
                        try:
                            if "inotify" in os.readlink(fd):
                                info = Path(f"/proc/{daemon.pid}/fdinfo/{fd.name}").read_text()
                                descriptors.append(info.count("inotify wd:"))
                        except FileNotFoundError:
                            pass
                    return descriptors if sum(descriptors) >= 8 else None
                counts = wait_for(inotify)
                time.sleep(0.3)
                return {"native_backend": "inotify", "registered_watches": sum(counts)}
            return {"native_backend": platform.system()}

        if not case("native_watcher_registered", start):
            return 1

        def lock_conflict():
            with (output / "second-daemon.log").open("w") as log:
                proc = subprocess.run([str(binary), "--workspace", str(workspace), *runtime_options],
                                      stdout=log, stderr=subprocess.STDOUT, timeout=15)
            assert proc.returncode != 0, "second daemon unexpectedly acquired workspace"
            assert "already watching" in (output / "second-daemon.log").read_text().lower()
        case("second_daemon_rejected", lock_conflict)

        def edit_body():
            before = sql("SELECT s.id,r.source_hash FROM symbols s JOIN sir r ON s.id=r.id WHERE s.qualified_name='helper'")[0]
            write("src/lib.rs", "pub fn helper() -> i32 { 2 }\npub fn greet() -> i32 { helper() }\n")
            def regenerated():
                rows = sql("SELECT s.id,r.source_hash FROM symbols s JOIN sir r ON s.id=r.id WHERE s.qualified_name='helper' AND r.sir_status='fresh'")
                return len(rows) == 1 and rows[0][0] == before[0] and rows[0][1] != before[1]
            wait_for(regenerated)
            after = sql("SELECT s.id,r.source_hash FROM symbols s JOIN sir r ON s.id=r.id WHERE s.qualified_name='helper'")[0]
            assert before[0] == after[0] and before[1] != after[1], (before, after)
            return {"stable_symbol_id": before[0], "source_hash_updated": True}
        case("body_edit_regenerates_SIR_with_stable_identity", edit_body)

        def create():
            write("created.rs", "pub fn created() {}\n")
            wait_for(lambda: rows_for("created.rs"))
        case("file_create", create)

        def atomic_save():
            write("created.rs.tmp", "pub fn atomically_saved() {}\n")
            os.replace(workspace / "created.rs.tmp", workspace / "created.rs")
            wait_for(lambda: [name for _, name in rows_for("created.rs")] == ["atomically_saved"])
        case("atomic_editor_save", atomic_save)

        def rename():
            (workspace / "created.rs").rename(workspace / "renamed.rs")
            wait_for(lambda: not rows_for("created.rs") and rows_for("renamed.rs"))
        case("file_rename", rename)

        def delete():
            (workspace / "renamed.rs").unlink()
            wait_for(lambda: not rows_for("renamed.rs"))
            assert sql("SELECT count(*) FROM symbol_edges WHERE file_path='renamed.rs'") == [(0,)]
        case("file_delete", delete)

        def reexport():
            write("pkg/api/__init__.py", "from .new import f\n")
            wait_for(lambda: ("pkg.api.new::f",) in targets("app.py") and ("pkg.api.old::f",) not in targets("app.py"))
        case("imports_only_reexport_updates_unchanged_caller", reexport)

        def moved_directory():
            staging = output / "staging/nested"
            staging.mkdir(parents=True)
            (staging / "moved.rs").write_text("pub fn moved_into_workspace() {}\n")
            (output / "staging").rename(workspace / "incoming")
            wait_for(lambda: rows_for("incoming/nested/moved.rs"))
        case("prepopulated_nested_directory_move", moved_directory)

        def nested_create():
            write("incoming/nested/later.rs", "pub fn created_in_moved_directory() {}\n")
            wait_for(lambda: rows_for("incoming/nested/later.rs"))
        case("new_file_inside_moved_nested_directory", nested_create)

        def moved_away():
            assert rows_for("tracked/deep/present.rs"), "must start with an indexed descendant"
            (workspace / "tracked").rename(output / "moved-away")
            wait_for(lambda: sql("SELECT count(*) FROM symbols WHERE file_path LIKE 'tracked/%'") == [(0,)])
        case("directory_move_out_retires_descendants", moved_away)

        def branch_switch():
            git("add", ".")
            git("commit", "-m", "synthetic edits")
            git("switch", "-c", "alternate")
            write("src/lib.rs", "pub fn helper() -> i32 { 3 }\npub fn branch_only() {}\n")
            git("add", "src/lib.rs")
            git("commit", "-m", "synthetic alternate branch")
            wait_for(lambda: sql("SELECT count(*) FROM symbols WHERE qualified_name='branch_only'") == [(1,)])
            git("switch", "main")
            wait_for(lambda: sql("SELECT count(*) FROM symbols WHERE qualified_name='branch_only'") == [(0,)]
                     and sql("SELECT count(*) FROM symbols WHERE qualified_name='greet' AND language='rust'") == [(1,)])
        case("native_git_branch_switch", branch_switch)

        def ignored():
            write("target/later.rs", "pub fn must_not_index_later() {}\n")
            time.sleep(0.5)
            assert sql("SELECT count(*) FROM symbols WHERE qualified_name LIKE '%must_not_index%'") == [(0,)]
        case("ignored_directories_stay_ignored", ignored)

        stop()
        report["watcher_symbols_before_restart"] = sql("SELECT count(*) FROM symbols")[0][0]

        def offline_restart():
            write("offline.rs", "pub fn created_while_stopped() {}\n")
            run_cli("restart-full", "--index-once", "--full")
            assert rows_for("offline.rs")
            assert rows_for("incoming/nested/moved.rs")
            assert rows_for("incoming/nested/later.rs")
            assert sql("SELECT count(*) FROM symbols WHERE file_path LIKE 'tracked/%'") == [(0,)]
            assert sql("PRAGMA quick_check") == [("ok",)]
        case("cold_restart_reconciles_offline_changes", offline_restart)

        def repeat():
            before = sql("SELECT id,file_path,language,kind,qualified_name,signature_fingerprint FROM symbols ORDER BY id")
            history = set(sql("SELECT symbol_id,version,sir_hash,sir_json FROM sir_history"))
            run_cli("repeat-full", "--index-once", "--full")
            assert before == sql("SELECT id,file_path,language,kind,qualified_name,signature_fingerprint FROM symbols ORDER BY id")
            assert history <= set(sql("SELECT symbol_id,version,sir_hash,sir_json FROM sir_history"))
            run_cli("status", "status")
            return {"stable_symbols": len(before), "preserved_history_rows": len(history)}
        case("repeat_full_scan_preserves_identity_and_history", repeat)
    finally:
        stop()
        report["passed"] = sum(item["result"] == "passed" for item in report["cases"])
        report["failed"] = sum(item["result"] == "failed" for item in report["cases"])
        save()
    return int(report["failed"] != 0)


if __name__ == "__main__":
    raise SystemExit(main())
