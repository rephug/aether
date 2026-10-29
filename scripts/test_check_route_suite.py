import copy
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import check_route_suite as suite_checker
from check_route_suite import check_suite, Snapshot, MAX_FILE_BYTES

SITE = "https://site.fixture.invalid"
APP = "https://app.fixture.invalid"


class RouteSuiteTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in ["site/assets", "app"]:
            (self.root / name).mkdir(parents=True)
        self.write("site/assets/agents.svg", "<svg>agents</svg>")
        self.write("site/agents.html", '<img src="/assets/agents.svg">')
        self.write("site/index.html", '<a href="/agents">Agents</a>')
        self.write("app/start.html", f'<a href="{SITE}/agents">Agents</a>')
        self.provider = {
            "version": 1, "origin": SITE, "manifest": "manifest.json",
            "routes": [{"canonical": "/agents", "aliases": ["/realtor"],
                        "page": "agents.html", "media": ["assets/agents.svg"],
                        "entrypoints": [{"path": "index.html"}]}],
            "asset_sha256": {"assets/agents.svg": self.digest("site/assets/agents.svg")},
        }
        self.manifest = {
            "/agents": {"page": "agents.html", "media": ["assets/agents.svg"]},
            "/realtor": {"redirect": "/agents"},
        }
        self.suite = {
            "version": 1,
            "projects": {
                "website": {"root": "site", "origin": SITE, "route_contract": "routes.json"},
                "app": {"root": "app", "origin": APP},
            },
            "contracts": [{"id": "agents-entry", "provider": "website", "canonical": "/agents",
                           "consumers": [{"project": "app", "path": "start.html"}]}],
        }

    def write(self, name, text):
        (self.root / name).write_text(text, encoding="utf-8", newline="\n")

    def digest(self, name):
        return hashlib.sha256((self.root / name).read_bytes()).hexdigest()

    def save(self):
        self.write("suite.json", json.dumps(self.suite))
        self.write("site/routes.json", json.dumps(self.provider))
        self.write("site/manifest.json", json.dumps(self.manifest))

    def check(self):
        self.save()
        return check_suite(self.root, self.root / "suite.json")

    def codes(self, result):
        return {finding["code"] for finding in result["findings"]}

    def entry(self):
        return self.suite["contracts"][0]["consumers"][0]

    def test_two_projects_share_verified_canonical_route(self):
        result = self.check()
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["bindings"][0]["canonical"], SITE + "/agents")
        self.assertIn("site/assets/agents.svg", result["file_sha256"])
        self.assertIn("app/start.html", result["file_sha256"])
        self.assertIn("suite.json", result["file_sha256"])

    def test_alias_needs_explicit_consumer_approval(self):
        self.write("app/start.html", f'<a href="{SITE}/realtor">Agents</a>')
        self.assertIn("stale_consumer_link", self.codes(self.check()))
        self.entry()["allow_aliases"] = ["/realtor"]
        self.assertTrue(self.check()["ok"])

    def test_same_page_alias_and_realtor_filename_are_intentional(self):
        self.provider["routes"][0].update(page="realtor.html", alias_modes={"/realtor": "same_page"})
        self.write("site/realtor.html", '<link rel="canonical" href="/agents"><img src="/assets/agents.svg">')
        self.manifest["/agents"]["page"] = "realtor.html"
        self.manifest["/realtor"] = {"page": "realtor.html", "canonical": "/agents"}
        self.write("app/start.html", f'<a href="{SITE}/realtor">Agents</a>')
        self.entry()["allow_aliases"] = ["/realtor"]
        self.assertTrue(self.check()["ok"])

    def test_good_link_cannot_hide_stale_cross_project_link(self):
        self.write("app/start.html", f'<a href="{SITE}/agents">Good</a><a href="{SITE}/old-realtor">Stale</a>')
        result = self.check()
        self.assertIn("stale_consumer_link", self.codes(result))
        self.assertEqual(next(x for x in result["findings"] if x["code"] == "stale_consumer_link")["project"], "app")

    def test_correct_path_on_wrong_origin_fails(self):
        self.write("app/start.html", '<a href="/agents">Wrong app host</a>')
        self.assertEqual(self.codes(self.check()), {"missing_consumer_link", "stale_consumer_link"})

    def test_same_origin_projects_allow_relative_canonical_link(self):
        self.suite["projects"]["app"]["origin"] = SITE
        self.write("app/start.html", '<a href="agents?from=app#top">Agents</a>')
        self.entry()["url"] = "/start"
        self.assertTrue(self.check()["ok"])

    def test_default_port_is_normalized_without_host_prefix_matching(self):
        self.write("app/start.html", f'<a href="{SITE}:443/agents">Agents</a>')
        self.assertTrue(self.check()["ok"])
        self.write("app/start.html", '<a href="https://site.fixture.invalid.evil/agents">Wrong</a>')
        self.assertFalse(self.check()["ok"])

    def test_unrelated_links_need_explicit_ignore(self):
        self.write("app/start.html", f'<a href="{SITE}/agents">Agents</a><a href="/about">About</a>')
        self.assertFalse(self.check()["ok"])
        self.entry()["ignore_hrefs"] = ["/about"]
        self.assertTrue(self.check()["ok"])

    def test_ignore_cannot_waive_known_route_even_on_wrong_origin(self):
        self.entry()["ignore_hrefs"] = ["/agents"]
        self.assertIn("invalid_consumer_artifact", self.codes(self.check()))

    def test_unknown_alias_is_configuration_failure(self):
        self.entry()["allow_aliases"] = ["/made-up-alias"]
        self.assertIn("invalid_consumer_artifact", self.codes(self.check()))

    def test_wrong_provider_mapping_fails_even_when_consumer_link_is_good(self):
        self.manifest["/agents"]["page"] = "wrong-page.html"
        result = self.check()
        self.assertIn("wrong_page_mapping", self.codes(result))
        self.assertIn("provider_unverified", self.codes(result))

    def test_provider_extra_media_and_changed_asset_fail(self):
        self.write("site/agents.html", '<img src="/assets/agents.svg"><img src="/wrong.svg">')
        self.assertIn("unexpected_page_media", self.codes(self.check()))
        self.write("site/agents.html", '<img src="/assets/agents.svg">')
        self.write("site/assets/agents.svg", "<svg>cached realtor image</svg>")
        self.assertIn("cached_asset_mismatch", self.codes(self.check()))

    def test_canonical_must_be_declared_in_provider_contract(self):
        self.suite["contracts"][0]["canonical"] = "/new-agents"
        self.assertIn("undeclared_provider_route", self.codes(self.check()))

    def test_provider_origin_cannot_be_rebound_in_suite(self):
        self.suite["projects"]["website"]["origin"] = "https://other.fixture.invalid"
        self.assertIn("provider_origin_mismatch", self.codes(self.check()))

    def test_all_consumers_checked_and_good_consumer_does_not_mask_bad(self):
        (self.root / "docs").mkdir()
        self.write("docs/entry.html", f'<a href="{SITE}/wrong-agents">Wrong</a>')
        self.suite["projects"]["docs"] = {"root": "docs", "origin": "https://docs.fixture.invalid"}
        self.suite["contracts"][0]["consumers"].append({"project": "docs", "path": "entry.html"})
        result = self.check()
        self.assertFalse(result["ok"])
        self.assertEqual(len(result["bindings"]), 2)
        self.assertEqual({x["consumer"] for x in result["bindings"]}, {"app", "docs"})

    def test_scripts_rejected_even_with_good_static_anchor(self):
        self.write("app/start.html", f'<a href="{SITE}/agents">Good</a><script>location.href="/wrong"</script>')
        self.assertIn("unsupported_dynamic_content", self.codes(self.check()))

    def test_pinned_uninterpreted_scripts_check_identity_not_behavior(self):
        self.write("app/start.html", f'<a href="{SITE}/agents">Good</a><script src="/app.js"></script>')
        self.write("app/app.js", 'location.href = "/unverified-dynamic-target";')
        self.entry()["script_policy"] = "pinned_uninterpreted"
        self.assertIn("invalid_consumer_artifact", self.codes(self.check()))
        self.suite["projects"]["app"]["asset_sha256"] = {
            "start.html": self.digest("app/start.html"), "app.js": self.digest("app/app.js")}
        result = self.check()
        self.assertTrue(result["ok"], result)
        self.assertIn("not analyzed", result["coverage"])
        self.write("app/app.js", "/* changed script bytes */")
        self.assertIn("cached_asset_mismatch", self.codes(self.check()))

    def test_external_uninterpreted_scripts_require_dedicated_verifier(self):
        self.write("app/start.html", f'<a href="{SITE}/agents">Good</a><script src="https://cdn.fixture.invalid/app.js"></script>')
        self.entry()["script_policy"] = "pinned_uninterpreted"
        self.suite["projects"]["app"]["asset_sha256"] = {"start.html": self.digest("app/start.html")}
        self.assertIn("invalid_consumer_artifact", self.codes(self.check()))

    def test_base_and_duplicate_attributes_fail_closed(self):
        for markup in [
            f'<base href="https://wrong.invalid"><a href="{SITE}/agents">Good-looking</a>',
            f'<a href="{SITE}/wrong" href="{SITE}/agents">Ambiguous</a>',
        ]:
            self.write("app/start.html", markup)
            self.assertIn("unsupported_static_content", self.codes(self.check()))

    def test_malformed_schema_duplicate_ids_and_empty_consumers_fail(self):
        cases = []
        one = copy.deepcopy(self.suite); one["contracts"] = []; cases.append(one)
        one = copy.deepcopy(self.suite); one["contracts"][0]["consumers"] = []; cases.append(one)
        one = copy.deepcopy(self.suite); one["contracts"].append(copy.deepcopy(one["contracts"][0])); cases.append(one)
        one = copy.deepcopy(self.suite); one["projects"]["app"]["orgin"] = APP; cases.append(one)
        one = copy.deepcopy(self.suite); one["version"] = True; cases.append(one)
        one = copy.deepcopy(self.suite); one["contracts"][0]["provider"] = "missing"; cases.append(one)
        for case in cases:
            self.suite = case
            self.assertIn("invalid_suite", self.codes(self.check()))

    def test_duplicate_json_key_is_not_silently_accepted(self):
        self.save()
        self.write("suite.json", '{"version":1,"version":1}')
        self.assertIn("invalid_suite", self.codes(check_suite(self.root, self.root / "suite.json")))

    def test_root_and_artifact_paths_cannot_escape(self):
        for name in ["../outside.html", "C:/outside.html", r"..\outside.html", "/absolute.html"]:
            self.entry()["path"] = name
            self.assertIn("invalid_suite", self.codes(self.check()))
        self.entry()["path"] = "start.html"
        self.suite["projects"]["app"]["root"] = "../outside"
        self.assertIn("invalid_suite", self.codes(self.check()))

    def test_same_physical_project_root_is_rejected(self):
        self.suite["projects"]["app"]["root"] = "site"
        self.assertIn("invalid_suite", self.codes(self.check()))

    def test_consumer_symlink_cannot_escape_its_project_even_within_suite(self):
        target = self.root / "site" / "other.html"
        target.write_text(f'<a href="{SITE}/agents">Good but wrong project</a>')
        path = self.root / "app" / "link.html"
        try:
            path.symlink_to(target)
        except (OSError, NotImplementedError):
            self.skipTest("symlink creation unavailable; Linux CI exercises this boundary")
        self.entry()["path"] = "link.html"
        self.assertIn("invalid_consumer_artifact", self.codes(self.check()))

    def test_missing_file_and_invalid_root_return_structured_failures(self):
        self.entry()["path"] = "missing.html"
        self.assertIn("invalid_consumer_artifact", self.codes(self.check()))
        self.assertIn("invalid_suite", self.codes(check_suite(self.root / "missing", self.root / "suite.json")))

    def test_bounded_reader_stops_after_one_sentinel_byte(self):
        self.write("big.html", "x" * (MAX_FILE_BYTES + 10))
        reader = Snapshot(self.root)
        with self.assertRaises(ValueError):
            reader.read("big.html")
        self.assertEqual(reader.bytes_read, MAX_FILE_BYTES + 1)

    def test_suite_metadata_budget_is_enforced(self):
        self.save()
        with patch.object(suite_checker, "MAX_BYTES", 100):
            result = check_suite(self.root, self.root / "suite.json")
        self.assertFalse(result["ok"])
        self.assertEqual(result["entry_bytes_read"], 101)

    def test_receipt_is_portable_and_changes_with_consumer_input(self):
        before = self.check()
        with tempfile.TemporaryDirectory() as target:
            copy_root = Path(target) / "snapshot"
            shutil.copytree(self.root, copy_root)
            moved = check_suite(copy_root, copy_root / "suite.json")
            self.assertEqual(before["artifact_sha256"], moved["artifact_sha256"])
        self.write("app/start.html", f'<a href="{SITE}/agents">Changed wording</a>')
        after = self.check()
        self.assertTrue(after["ok"])
        self.assertNotEqual(before["artifact_sha256"], after["artifact_sha256"])

    def test_provider_contract_changed_during_inspection_is_not_rebound(self):
        real = suite_checker.check_provider
        def changing(*args):
            result = real(*args)
            changed = copy.deepcopy(self.provider)
            changed["origin"] = "https://changed.fixture.invalid"
            self.write("site/routes.json", json.dumps(changed))
            return result
        self.save()
        with patch.object(suite_checker, "check_provider", side_effect=changing):
            result = check_suite(self.root, self.root / "suite.json")
        self.assertFalse(result["ok"])
        self.assertIn("artifact changed", str(result))

    def test_cli_exit_and_json_are_gate_compatible(self):
        self.save()
        command = [sys.executable, "-B", str(Path(suite_checker.__file__)),
                   "--root", str(self.root), "--suite", str(self.root / "suite.json")]
        passed = subprocess.run(command, capture_output=True, text=True, timeout=20)
        self.assertEqual(passed.returncode, 0, passed.stderr)
        self.assertTrue(json.loads(passed.stdout)["ok"])
        self.write("app/start.html", f'<a href="{SITE}/wrong-agents">Wrong</a>')
        failed = subprocess.run(command, capture_output=True, text=True, timeout=20)
        self.assertEqual(failed.returncode, 1, failed.stderr)
        self.assertFalse(json.loads(failed.stdout)["ok"])


    def test_collection_limits_are_enforced_before_artifact_reads(self):
        original = copy.deepcopy(self.suite)
        cases = []
        one = copy.deepcopy(original)
        one["projects"] = {f"project{i}": {"root": "app", "origin": APP} for i in range(9)}
        cases.append(one)
        one = copy.deepcopy(original)
        one["contracts"] = [copy.deepcopy(one["contracts"][0]) for _ in range(33)]
        cases.append(one)
        one = copy.deepcopy(original)
        one["contracts"][0]["consumers"] = [copy.deepcopy(self.entry()) for _ in range(65)]
        cases.append(one)
        for value in cases:
            self.suite = value
            self.assertIn("invalid_suite", self.codes(self.check()))

    def test_invalid_origins_fail_without_network_or_credential_handling(self):
        for value in ["https://user:password@site.invalid", "file:///site", "https://site.invalid/path",
                      "https://site.invalid?query", "https://site.invalid:99999"]:
            self.suite["projects"]["app"]["origin"] = value
            self.assertIn("invalid_suite", self.codes(self.check()))

    def test_excessively_nested_json_returns_a_structured_failure(self):
        self.save()
        self.write("suite.json", "[" * 2000 + "]" * 2000)
        result = check_suite(self.root, self.root / "suite.json")
        self.assertIn("invalid_suite", self.codes(result))

    def test_failed_provider_does_not_suppress_an_independent_contract(self):
        self.save()
        shutil.copytree(self.root / "site", self.root / "catalog")
        other = copy.deepcopy(self.provider)
        other["origin"] = "https://catalog.fixture.invalid"
        self.write("catalog/routes.json", json.dumps(other))
        self.suite["projects"]["catalog"] = {
            "root": "catalog", "origin": other["origin"], "route_contract": "routes.json"}
        self.write("app/catalog.html", '<a href="https://catalog.fixture.invalid/agents">Catalog</a>')
        self.suite["contracts"].append({
            "id": "catalog-entry", "provider": "catalog", "canonical": "/agents",
            "consumers": [{"project": "app", "path": "catalog.html"}]})
        self.manifest["/agents"]["page"] = "wrong.html"
        result = self.check()
        self.assertFalse(result["ok"])
        self.assertTrue(result["providers"]["catalog"]["ok"])
        self.assertEqual([x["contract"] for x in result["bindings"]], ["catalog-entry"])


if __name__ == "__main__":
    unittest.main()
