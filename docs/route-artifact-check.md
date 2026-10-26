# Static route and media receipts

`scripts/check_route_artifacts.py` is an offline prototype for Aether's existing
`[verify].commands` interface. It checks declared local files, without running a
browser, requesting a website, generating SIRs, or opening an Aether store.

Run the valid pinned example from the repository root:

```sh
python3 scripts/check_route_artifacts.py --root scripts/fixtures/routes --contract scripts/fixtures/routes/contract.json
python3 -m unittest discover -s scripts -p test_check_route_artifacts.py -v
```

A project owner can configure the same command in its own `.aether/config.toml`:

```toml
[verify]
commands = ["python3 scripts/check_route_artifacts.py --root scripts/fixtures/routes --contract scripts/fixtures/routes/contract.json"]
```

This change does not alter anyone's live configuration or automatically register
the prototype as an MCP tool. Replace the fixture paths with the project's
reviewed build-output contract before configuring a real project.

The contract names canonical routes, intentional aliases, approved entry links,
expected page and media mappings, and pinned SHA-256 asset receipts. The example
uses canonical `/agents` with an intentional `/realtor` redirect. A declared alias
is valid; an entrypoint may explicitly permit that alias too. A stale entry link
is judged against its declared allowed links, not against a general ban on aliases.

The checker verifies the build manifest's mapping, the expected media references
in the page, and actual local asset bytes against the pinned receipts. It exits
nonzero and emits structured findings for mismatches. Reads are restricted to the
declared root: at most 64 files, 1 MiB per file, 8 MiB total; route, alias and media
lists have independent bounds. An oversize read stops after one sentinel byte
beyond the applicable limit. Invalid roots, missing receipts and symlinks escaping
the declared root produce structured failures. It does not discover undeclared
routes or assets.

`artifact_sha256` binds the names and bytes of the consumed contract, manifest,
pages, entrypoints and assets. It is separate from Aether's `source_hash`, whose
parser-supported source inventory excludes HTML, JSON and SVG. Static changes
can therefore change the artifact receipt while leaving `source_hash` unchanged.
Neither hash proves that a server or CDN is serving those bytes. A deployment
integration must separately supply and verify its observed serving receipts.

Tests cover the intentional alias, explicitly approved alias entry, stale links,
wrong page/media mappings, wrong media references in page content, changed asset
bytes, invalid redirects, missing/oversized inputs, missing media receipts,
invalid roots, total byte limits, symlink escape and path traversal rejection.
JavaScript execution, dynamic routing, CSS/srcset media discovery, browser caches
and CDN headers are outside this prototype's coverage.
