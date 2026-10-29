"""Read-only route contracts between explicitly declared local project snapshots.

Runs the existing route-artifact verifier for providers, then checks static
consumer links against the provider's canonical route and approved aliases.
No browser, network request, model, store, or arbitrary command is executed.
"""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
from urllib.parse import unquote, urljoin, urlsplit

from check_route_artifacts import (
    Markup, check as check_provider, unique_object,
    MAX_FILE_BYTES, MAX_TOTAL_BYTES as PROVIDER_MAX_BYTES,
)

MAX_PROJECTS = 8
MAX_CONTRACTS = 32
MAX_CONSUMERS = 64
MAX_FILES = 128
MAX_BYTES = 32 * 1024 * 1024
MAX_PINS = 32
ID = re.compile(r"[a-z][a-z0-9_-]{0,47}\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
COVERAGE = (
    "Declared local project snapshots and static HTML anchor links only. Provider "
    "checks retain the route-artifact verifier's bounded coverage. Consumer scripts "
    "are rejected unless explicitly pinned and uninterpreted; imported dependencies "
    "and script-generated navigation are not analyzed. Receipts are independent of "
    "Aether source_hash. No runtime, deployed-byte, browser, server or CDN assertion."
)


def fields(value, required, optional=()):
    if not isinstance(value, dict):
        raise ValueError("expected a JSON object")
    if set(value) - set(required) - set(optional) or set(required) - set(value):
        raise ValueError(
            f"expected fields {sorted(required)}, optional {sorted(optional)}; "
            f"observed {sorted(value)}"
        )


def relative(value, allow_dot=False):
    if not isinstance(value, str) or not value or unquote(value) != value:
        raise ValueError("expected an unencoded relative POSIX path")
    path = PurePosixPath(value)
    if (value != path.as_posix() or path.is_absolute() or ".." in path.parts
            or any(char in value for char in "\\:\x00")
            or (value == "." and not allow_dot)):
        raise ValueError(f"invalid relative path: {value}")
    return value


def origin(value):
    if not isinstance(value, str) or not value.isascii():
        raise ValueError("origin must be an ASCII HTTP(S) origin")
    parsed = urlsplit(value)
    if (parsed.scheme not in {"http", "https"} or not parsed.hostname
            or parsed.username is not None or parsed.password is not None
            or parsed.path or parsed.query or parsed.fragment or "\\" in value
            or any(ord(char) <= 32 for char in value)):
        raise ValueError("origin must contain only HTTP(S) scheme, host and optional port")
    port = parsed.port
    if port is not None and not 1 <= port <= 65535:
        raise ValueError("invalid origin port")
    host = parsed.hostname.lower()
    if ":" in host:
        host = "[" + host + "]"
    default = 443 if parsed.scheme == "https" else 80
    return parsed.scheme + "://" + host + (f":{port}" if port and port != default else "")


def address(value, base, site):
    if (not isinstance(value, str) or not value or value != value.strip()
            or "\\" in unquote(value) or any(ord(char) <= 32 for char in value)
            or ".." in PurePosixPath(urlsplit(unquote(value)).path).parts):
        raise ValueError("unsupported URL spelling or traversal")
    parsed = urlsplit(urljoin(site + base, value))
    site_origin = origin(parsed.scheme + "://" + parsed.netloc)
    path = unquote(parsed.path) or "/"
    if any(ord(char) <= 32 for char in path):
        raise ValueError("encoded control/space in route path is unsupported")
    return site_origin + path


class EntryMarkup(Markup):
    def handle_starttag(self, tag, attrs):
        if len(attrs) != len(dict(attrs)):
            self.problems.append("duplicate HTML attributes require a dedicated verifier")
        super().handle_starttag(tag, attrs)


class Snapshot:
    def __init__(self, root):
        self.root = root
        self.cache = {}
        self.hashes = {}
        self.bytes_read = 0
        self.reads = 0

    def locate(self, name, directory=False):
        relative(name, allow_dot=directory)
        path = (self.root / name).resolve(strict=True)
        path.relative_to(self.root)
        if directory != path.is_dir() or (not directory and not path.is_file()):
            raise ValueError(f"expected {'directory' if directory else 'file'}: {name}")
        return path

    def read(self, name):
        if name in self.cache:
            return self.cache[name]
        path = self.locate(name)
        if self.reads >= MAX_FILES:
            raise ValueError("suite entry/metadata file-read limit exceeded")
        if self.bytes_read >= MAX_BYTES:
            raise ValueError("suite entry/metadata byte limit exhausted")
        allowed = min(MAX_FILE_BYTES, MAX_BYTES - self.bytes_read)
        with path.open("rb") as stream:
            data = stream.read(allowed + 1)
        self.reads += 1
        self.bytes_read += len(data)
        if len(data) > allowed:
            raise ValueError("suite entry/metadata byte limit exceeded")
        digest = hashlib.sha256(data).hexdigest()
        if name in self.hashes and self.hashes[name] != digest:
            raise ValueError(f"artifact changed during this inspection: {name}")
        self.hashes[name] = digest
        self.cache[name] = data
        return data

    def read_project(self, project, name):
        full = joined(project, name)
        self.locate(full).relative_to(self.locate(project["root"], directory=True))
        return self.read(full)

    def load(self, name):
        return json.loads(self.read(name), object_pairs_hook=unique_object)


def joined(project, name):
    relative(name)
    return (PurePosixPath(project["root"]) / name).as_posix()


def check_suite(root, suite_path):
    findings, projects, providers, bindings, snapshot = [], {}, {}, [], None

    def finding(code, project, path, detail, contract=None):
        item = {"code": code, "project": project, "path": path, "detail": detail}
        if contract is not None:
            item["contract"] = contract
        findings.append(item)

    try:
        root = Path(root).resolve(strict=True)
        if not root.is_dir():
            raise ValueError("suite root must be a directory")
        snapshot = Snapshot(root)
        suite_name = Path(suite_path).resolve(strict=True).relative_to(root).as_posix()
        suite = snapshot.load(suite_name)
        fields(suite, {"version", "projects", "contracts"})
        if type(suite["version"]) is not int or suite["version"] != 1:
            raise ValueError("unsupported suite version")
        declared = suite["projects"]
        if not isinstance(declared, dict) or not 1 <= len(declared) <= MAX_PROJECTS:
            raise ValueError("suite requires 1 to 8 named projects")
        roots = set()
        for name, project in sorted(declared.items()):
            if not ID.fullmatch(name):
                raise ValueError("project IDs must be lowercase ASCII identifiers")
            fields(project, {"root", "origin"}, {"route_contract", "asset_sha256"})
            relative(project["root"], allow_dot=True)
            location = snapshot.locate(project["root"], directory=True)
            if location in roots:
                raise ValueError("project roots must resolve to distinct directories")
            roots.add(location)
            project = dict(project)
            project["origin"] = origin(project["origin"])
            pins = project.get("asset_sha256", {})
            if not isinstance(pins, dict) or len(pins) > MAX_PINS:
                raise ValueError("project asset pins must be a bounded object")
            for path, digest in pins.items():
                relative(path)
                if not isinstance(digest, str) or not SHA256.fullmatch(digest):
                    raise ValueError("project pins require lowercase SHA-256")
            if "route_contract" in project:
                relative(project["route_contract"])
            projects[name] = project

        contracts = suite["contracts"]
        if not isinstance(contracts, list) or not 1 <= len(contracts) <= MAX_CONTRACTS:
            raise ValueError("suite requires 1 to 32 shared contracts")
        seen, consumers_count = set(), 0
        for contract in contracts:
            fields(contract, {"id", "provider", "canonical", "consumers"})
            name = contract["id"]
            if not isinstance(name, str) or not ID.fullmatch(name) or name in seen:
                raise ValueError("contract IDs must be distinct lowercase identifiers")
            seen.add(name)
            if not isinstance(contract["provider"], str) or contract["provider"] not in projects:
                raise ValueError("contract names an unknown provider project")
            provider = projects[contract["provider"]]
            if "route_contract" not in provider:
                raise ValueError("provider project requires a route_contract")
            canonical = contract["canonical"]
            if (not isinstance(canonical, str) or not canonical.startswith("/")
                    or canonical.startswith("//") or "?" in canonical or "#" in canonical
                    or address(canonical, "/", provider["origin"]) != provider["origin"] + canonical):
                raise ValueError("contract canonical must be an absolute local route path")
            consumers = contract["consumers"]
            if not isinstance(consumers, list) or not consumers:
                raise ValueError("each contract requires consumer entrypoints")
            consumers_count += len(consumers)
            if consumers_count > MAX_CONSUMERS:
                raise ValueError("suite consumer count exceeds 64")
            for entry in consumers:
                fields(entry, {"project", "path"}, {"url", "allow_aliases", "ignore_hrefs", "script_policy"})
                if not isinstance(entry["project"], str) or entry["project"] not in projects:
                    raise ValueError("consumer names an unknown project")
                relative(entry["path"])
                aliases = entry.get("allow_aliases", [])
                ignored = entry.get("ignore_hrefs", [])
                if (not isinstance(aliases, list) or len(aliases) > 16
                        or any(not isinstance(x, str) for x in aliases)
                        or len(set(aliases)) != len(aliases)
                        or not isinstance(ignored, list) or len(ignored) > 64
                        or any(not isinstance(x, str) for x in ignored)):
                    raise ValueError("consumer alias/ignore lists must be bounded explicit strings")
                if entry.get("script_policy", "reject") not in {"reject", "pinned_uninterpreted"}:
                    raise ValueError("unknown consumer script policy")

        # Each child verifier is independently bounded to 64 files/8 MiB.
        # Suite-owned reads below have a separate 128-file/32-MiB ceiling.
        for name, project in sorted(projects.items()):
            for path, expected in sorted(project.get("asset_sha256", {}).items()):
                full = joined(project, path)
                try:
                    if hashlib.sha256(snapshot.read_project(project, path)).hexdigest() != expected:
                        finding("cached_asset_mismatch", name, path, "project pin does not match local bytes")
                except (OSError, ValueError) as error:
                    finding("invalid_or_missing_artifact", name, path, str(error))
            if "route_contract" not in project:
                continue
            result = check_provider(root / project["root"], root / joined(project, project["route_contract"]))
            providers[name] = result
            for path, digest in result["file_sha256"].items():
                full = joined(project, path)
                if full in snapshot.hashes and snapshot.hashes[full] != digest:
                    finding("artifact_changed_during_check", name, path, "provider read differs from suite pin read")
                snapshot.hashes[full] = digest
            for problem in result["findings"]:
                finding(problem["code"], name, problem["path"], problem["detail"])

        for contract in contracts:
            name, provider_id = contract["id"], contract["provider"]
            provider = projects[provider_id]
            if not providers[provider_id]["ok"]:
                finding("provider_unverified", provider_id, provider["route_contract"], "provider checks failed; links cannot establish a valid destination", name)
                continue
            facts = json.loads(snapshot.read_project(provider, provider["route_contract"]), object_pairs_hook=unique_object)
            if origin(facts.get("origin", "https://fixture.invalid")) != provider["origin"]:
                finding("provider_origin_mismatch", provider_id, provider["route_contract"], "project origin differs from route-contract origin", name)
                continue
            matches = [route for route in facts["routes"] if route["canonical"] == contract["canonical"]]
            if len(matches) != 1:
                finding("undeclared_provider_route", provider_id, contract["canonical"], "shared canonical route is not uniquely declared by provider", name)
                continue
            route = matches[0]
            canonical = provider["origin"] + route["canonical"]
            declared_aliases = set(route.get("aliases", []))
            for entry in contract["consumers"]:
                project_id = entry["project"]
                project = projects[project_id]
                path = entry["path"]
                try:
                    aliases = entry.get("allow_aliases", [])
                    if set(aliases) - declared_aliases:
                        raise ValueError("allowed aliases must be declared by provider")
                    allowed = {canonical, *(provider["origin"] + alias for alias in aliases)}
                    base = entry.get("url", "/" + path)
                    if (not isinstance(base, str) or not base.startswith("/") or base.startswith("//")
                            or address(base, "/", project["origin"]) != project["origin"] + base):
                        raise ValueError("consumer URL must be an absolute local path without query/fragment")
                    ignored = {address(value, base, project["origin"]) for value in entry.get("ignore_hrefs", [])}
                    protected_paths = {route["canonical"], *declared_aliases}
                    if any(urlsplit(value).path in protected_paths for value in ignored):
                        raise ValueError("ignore_hrefs cannot waive a provider route/alias, even on another origin")
                    markup = EntryMarkup(snapshot.read_project(project, path).decode("utf-8"))
                    for problem in markup.problems:
                        finding("unsupported_static_content", project_id, path, problem, name)
                    if markup.scripts:
                        pins = project.get("asset_sha256", {})
                        if entry.get("script_policy", "reject") != "pinned_uninterpreted":
                            finding("unsupported_dynamic_content", project_id, path, "consumer JavaScript is not analyzed", name)
                        elif path not in pins:
                            raise ValueError("uninterpreted scripts require a pinned consumer page")
                        else:
                            for script in markup.scripts:
                                if script:
                                    url = address(script, base, project["origin"])
                                    if origin(urlsplit(url).scheme + "://" + urlsplit(url).netloc) != project["origin"]:
                                        raise ValueError("external scripts require a dedicated verifier")
                                    asset = urlsplit(url).path.lstrip("/")
                                    if asset not in pins:
                                        raise ValueError("every uninterpreted script requires a project pin")
                                    snapshot.read_project(project, asset)
                    links = {address(value, base, project["origin"]) for value in markup.links if not value.startswith("#")}
                    unexpected = sorted(links - allowed - ignored)
                    if not links.intersection(allowed):
                        finding("missing_consumer_link", project_id, path, f"expected a link to {sorted(allowed)}", name)
                    if unexpected:
                        finding("stale_consumer_link", project_id, path, f"unapproved links: {unexpected}; expected {sorted(allowed)}", name)
                    bindings.append({"contract": name, "provider": provider_id, "consumer": project_id,
                                     "path": path, "canonical": canonical, "allowed": sorted(allowed),
                                     "observed": sorted(links)})
                except (OSError, ValueError, TypeError, KeyError, UnicodeError) as error:
                    finding("invalid_consumer_artifact", project_id, path, str(error), name)
    except (OSError, ValueError, TypeError, KeyError, AttributeError, UnicodeError, RecursionError) as error:
        finding("invalid_suite", None, str(suite_path), str(error))

    hashes = snapshot.hashes if snapshot else {}
    identity = hashlib.sha256(json.dumps(hashes, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return {"version": 1, "ok": not findings, "findings": findings,
            "bindings": bindings, "providers": providers, "file_sha256": hashes,
            "artifact_sha256": identity,
            "entry_bytes_read": snapshot.bytes_read if snapshot else 0,
            "provider_bytes_read": sum(result["bytes_read"] for result in providers.values()),
            "coverage": COVERAGE,
            "limits": {"projects": MAX_PROJECTS, "shared_contracts": MAX_CONTRACTS,
                       "consumers": MAX_CONSUMERS, "entry_files": MAX_FILES,
                       "entry_bytes": MAX_BYTES, "provider_bytes_each": PROVIDER_MAX_BYTES}}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--suite", type=Path, required=True)
    args = parser.parse_args(argv)
    result = check_suite(args.root, args.suite)
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
