# Cross-project route contracts

Aether's offline route suite checks one declared destination across separate
local project snapshots. A website can keep canonical /agents and an intentional
/realtor alias while an app or documentation project links to it. The suite checks
the provider's route/page/media receipts and every declared consumer entrypoint.

This is a deterministic companion for the existing [verify].commands workflow.
It is separate from the SIR-based "aetherd contract check" command. It opens no
Aether store, calls no model/provider, requests no website, and executes no scripts.

## Run the two-project example

From the Aether repository root:

    python3 -B scripts/check_route_suite.py --root scripts/fixtures/route-suite --suite scripts/fixtures/route-suite/suite.json

The JSON report has ok: true and exit status 0. The website intentionally serves
realtor.html as canonical /agents and permits same-page /realtor. The app has a
different origin and links explicitly to https://website.fixture.invalid/agents.
The hostnames end in .invalid; they are identifiers, never fetched.

Change the app link in a disposable copy to /agents and rerun. It fails: that
relative URL points to the app's origin, not the website. A good website link
alongside a stale /old-realtor link also fails. Wrong provider page/media mappings
and pinned-asset changes fail even when every consumer link looks correct.
Exit status 1 plus project/path/contract findings is suitable for a CI gate.

## Suite format

The fixture's suite.json is a complete example. Version 1 contains only:

- projects: lowercase IDs mapped to root and origin, with optional route_contract
  and asset_sha256. Roots are relative POSIX directories under the explicit
  --root; files and symlinks must remain inside their declared project. Distinct
  IDs must resolve to distinct roots. No project discovery or recursive scan runs.
- contracts: named IDs with provider, canonical and nonempty consumers. A provider
  must have a route_contract that declares that exact canonical route. The
  project's origin must agree with the route contract; the suite cannot silently
  rebind a destination to another host.
- consumers: project and path, optional url, allow_aliases, ignore_hrefs and
  script_policy. The url is the source page's absolute local URL path, defaulting
  to "/" plus the artifact path. It is an assertion about that snapshot, not
  runtime route discovery.

Each consumer must contain an approved destination link. Every other non-fragment
anchor needs an explicit ignore_hrefs entry. Allowed destinations derive from the
provider origin/canonical plus only explicitly approved allow_aliases. Merely
declaring a provider alias does not automatically permit new app links to it.
An ignore cannot waive a known provider route/alias, including on the wrong host.
Ignore lists assert that those exact links are unrelated; review them as policy.

URLs resolve against the consumer's own origin and page path. HTTP(S) origins
use exact host/port identity; host-prefix matching is never used. Default ports,
query strings and fragments are normalized for route identity. Credentials,
traversal spellings and ambiguous base/duplicate-attribute markup fail closed.
Query-driven server behavior is not interpreted.

## JavaScript and input identities

Consumer scripts and event handlers fail by default. Explicit
script_policy: "pinned_uninterpreted" requires the consumer page and every local
referenced script in that project's asset_sha256 map. External scripts need a
dedicated verifier. Changed pinned bytes fail. The opt-in checks identity only:
it does not validate script behavior, imported dependencies or dynamic navigation.
A script can therefore contain a wrong dynamic URL and still satisfy an explicitly
uninterpreted byte receipt. Use an application verifier for that behavior.

The report includes per-file SHA-256, provider results, consumer bindings and an
aggregate artifact_sha256 binding the suite definition and consumed file names/
bytes. Consumer-only changes alter that receipt, independently of Aether's
parser-supported source_hash. Provider metadata read again for shared bindings
must match its earlier receipt, or the suite fails instead of mixing versions.
A hash is observation identity, not proof of Git/index, deployment or CDN freshness.
Use frozen local snapshots/build outputs: no OS-level atomic filesystem snapshot
or hostile concurrent-filesystem race guarantee is established.

Reads are bounded: at most 8 projects, 32 shared contracts and 64 consumers.
Each provider retains its verifier's 64-file/8-MiB/1-MiB-per-file ceiling.
Suite-owned entry/metadata/pin reads have a separate 128-file/32-MiB ceiling,
with a 1-MiB per-file cap; an oversize read stops after one sentinel byte.
The combined byte ceiling is therefore approximately 96 MiB, not unbounded
with repository size. Only declared files are read, and inspected data is never
used as a shell command.

## Use with Aether and release CI

An owner may configure the exact checker command in a dedicated snapshot
workspace's .aether/config.toml:

    [verify]
    commands = ["python3 /path/to/aether/scripts/check_route_suite.py --root . --suite suite.json"]

Invoke that exact allowlisted command through the existing Aether verifier:

    aetherd --workspace /path/to/snapshot-workspace --verify --verify-mode host --verify-command "python3 /path/to/aether/scripts/check_route_suite.py --root . --suite suite.json"

The equivalent Full-profile MCP entry is aether_verify with the same exact
commands array and mode: "host". Compact does not expose that tool. Both retain
the existing writable-server/allowlist checks; this suite adds no trust grant.
Use forward slashes in Windows paths and quote paths containing spaces according
to the host command parser. Do not copy the example path into a live configuration
without adapting and reviewing it. The Python process itself is read-only; normal
Aether verification bookkeeping has its existing workspace/store behavior.

For actual prevention, an application's separately approved mandatory pre-publish
job must run the suite and refuse publication on nonzero exit. Optional Aether
agent guidance or an advisory verify call does not establish a release gate.
This change installs no website guard, modifies no other project, and changes no
live process or store. Runtime routing, browser rendering and CDN behavior need
separate application evidence.

## Validation

    python3 -B -m unittest discover -s scripts -p test_check_route_suite.py -v

The suite covers two/three-project checks, same-page aliases, the intentional
realtor filename, wrong host with correct path, stale-plus-good links, provider
media/cache drift, bounded reads, symlink/project boundaries, duplicate/invalid
definitions, pinned uninterpreted scripts, portable receipts and CLI exit codes.
The same tests run in the existing offline-helper CI job.
