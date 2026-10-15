# AETHER review in CI

`aetherd init-agent --platform claude` or `--platform all` (the default, so a bare `aetherd init-agent` too) writes `.github/workflows/aether-review.yml` into the target repository. On every pull request it builds a structural AETHER index of the PR's merge result and posts one comment, updated in place on later pushes. When the analysis finishes, the report also goes to the job summary, which is where pull requests from forks find it. It contains:

- **Usually changed together, but not in this PR:** for each changed source file (up to 15), the files that git history says change with it at medium risk or above (`aetherd blast-radius`, after `aetherd mine-coupling`), when the PR doesn't touch them (deleting or renaming a file counts as touching it).
- **Risky symbols in changed files:** symbols from the workspace's 200 most critical, bottleneck and risk-hotspot symbols in the dependency graph (`aetherd health`) that live in the changed files, and any dependency cycle through them. In CI every symbol's risk score carries the same offset for having no SIRs and no access history, so compare the scores with each other rather than with an indexed workstation's.
- **Crate health** (Cargo workspaces with a `[workspace]` table only): `aetherd health-score` for the crates the PR touches, or the five lowest-scoring crates.

A section whose `aetherd` command failed says "Unavailable", with the reason in the report's notes, so a failure never reads as a clean result.

The review is advisory. The job is `continue-on-error: true`, so the workflow run stays green even when the job fails. The job's own check can still show a failure, for example when the first `aetherd` build fails, so **don't make it a required status check**. The comment gives no pass/fail verdict.

## No API keys, no SIR generation

The workflow indexes with `aetherd --workspace . --index-once --inference-provider mock`. Without `--full` this builds only the structural index (symbols and edges), so no SIRs are generated and no model is called; coupling comes from the separate `aetherd mine-coupling` step, which reads git history. Embeddings stay disabled in the default configuration. The workflow is given no secrets; its only token is the job's own `GITHUB_TOKEN`, used to post the comment. The checkout doesn't persist that token in `.git/config`, so the `aetherd` build and run can't read it from there.

## How the workflow gets `aetherd`

AETHER has no release binaries yet, so the workflow builds `aetherd` from source and caches it under the runner's tool cache (`$RUNNER_TOOL_CACHE/aether-ci`):

1. `AETHER_REF` (default `main`) is resolved to a commit: a full SHA is used as is, otherwise `git ls-remote` looks for the branch, then the tag. An unknown ref fails the job instead of falling back.
2. `actions/cache/restore` looks for `aetherd-<os>-<commit>`, and otherwise for the most recent `aetherd-<os>-*` entry. GitHub searches the run's own scope first (for a pull request, caches that pull request saved) and then the default branch's, so a pull request that once built its own `aetherd` keeps using that build.
3. **Pull requests** use any cached `aetherd`, even one built from an older AETHER commit, and build only when nothing is cached. **Scheduled runs (weekly) and manual runs** (`workflow_dispatch`) rebuild whenever the resolved commit has no exact cache entry.
4. A build installs `protobuf-compiler` and runs `cargo +stable install --locked --git https://github.com/rephug/aether --rev <commit> --root "$RUNNER_TOOL_CACHE/aether-ci" aetherd` (`--locked` uses AETHER's committed `Cargo.lock`; the binary is stripped). `actions/cache/save` stores it immediately, so a later failure or a cancelled run doesn't throw the build away.

Why the schedule matters: GitHub scopes a cache saved by a `pull_request` run to that pull request. Caches saved by scheduled or manual runs belong to the default branch, and every pull request can restore those. Run the workflow once by hand from the default branch after adding it (Actions → AETHER review → Run workflow) so that the first pull requests find a build. Without that, each new pull request builds `aetherd` on its first run. Runs for the same pull request (or branch) queue rather than cancel each other, so such a first build always finishes and is saved, at the cost of later pushes waiting for it.

**Build time and cost:** `aetherd` is a large release build (about a thousand locked crates, including the LanceDB, DataFusion, Arrow and SurrealDB stacks). It has not been measured on GitHub's runners yet; the job allows 120 minutes. The weekly scheduled run rebuilds whenever `AETHER_REF` has moved, which with the default `main` is most weeks; on private repositories that is billable runner time. Pin `AETHER_REF`, or remove the `schedule` trigger and run the workflow by hand when you want a newer `aetherd`. Pinning `AETHER_REF` to a commit SHA avoids rebuilds when AETHER's `main` moves, and also means the job runs code you chose, with a token that can write PR comments.

Alternatives considered: a prebuilt binary (none exists yet); a Docker image (none is published, and it would still need building somewhere); building without a cache (a long build on every push). When AETHER publishes release binaries, a download should replace steps 3 and 4.

## Comment updates

The comment starts with the hidden marker `<!-- aether-review -->` and names the commit it describes. On each run the workflow looks for a comment by `github-actions[bot]` containing the marker and edits it; if there is none, it creates one. This uses the preinstalled `gh` CLI with `GITHUB_TOKEN`, so no third-party action is involved.

## Limits

- **Pull requests from forks:** GitHub gives those runs a read-only token, so the comment step is skipped; the report is still in the job summary. The workflow deliberately does not use `pull_request_target`, which would run the fork's code with a write token.
- **Large repositories:** the structural index and coupling mining run from scratch on each push (coupling scans up to `coupling.commit_window` commits, 500 by default). Caching `.aether/` keyed by the base commit is a possible later optimization.
- **Repositories with their own `.aether/config.toml`:** its settings apply. If it enables embeddings with a remote provider, loading that provider fails for lack of a key, coupling mining and `blast-radius` fail with it, and the coupling section says "Unavailable". No API call is made.
- **Action versions:** the workflow uses `actions/checkout@v5` and `actions/cache/restore` and `actions/cache/save` at `v4`. Bump them as GitHub retires older runtimes.

## Opting in to SIR-powered review

Not enabled by default (decision #118). To review with SIRs, a repository would add an inference API key as a secret, pass it to the analysis step, and index with `--full` and a real provider. That costs money on every push and sends source code to the provider, so it is left to each project; the structural review needs neither.

## Turning it off

Disable the workflow in the repository's Actions settings (Actions → AETHER review → Disable workflow), or narrow when it runs with a `paths:` or `branches:` filter under `on.pull_request`. Deleting the file also works, but a later `aetherd init-agent` run writes it again. `init-agent` skips files that already exist unless run with `--force`, so an edited workflow is not overwritten.
