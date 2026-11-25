# stackrig (`strig`)

CLI that runs Coolify-generated compose stacks without Coolify. See `strig help`.

## Install (Homebrew)

```sh
brew tap tschuba/stackrig https://github.com/tschuba/stackrig
brew install stackrig        # latest release, built from source (needs Rust, installed by brew)
brew upgrade stackrig        # update
brew install --HEAD stackrig # current master
```

## Stacks behind Coolify's Traefik (inner Traefik)

Run strig stacks on another host (or next to Coolify) and let Coolify's Traefik forward their domains to one inner Traefik.
On the stack host (or with `--to <host>`) run `strig proxy up --trust <coolify-ip>`: it starts the inner Traefik on port 8080
(label `stackrig.proxy=true`; `--trust` = address Coolify's Traefik connects from, so `X-Forwarded-Proto` survives).
While that container exists, `render`/`up` emit one plain `http` router per service (no TLS, certresolver or redirect:
Coolify's Traefik terminates TLS); `STRIG_BEHIND_PROXY=1|0` forces the mode. Untested against a real setup.

Then `strig route <stack-dir> --from <coolify-host> --to <stack-host>` writes `stackrig-<stack>.yaml` into Coolify's Traefik (see `strig help route`).

## Releases

Versioning is automated with [release-please](https://github.com/googleapis/release-please).

- Commits and PR titles must follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `feat!:`). `ci:`/`chore:`/`docs:` do not trigger a release.
- Only squash merges are allowed; the PR title becomes the commit message. The `pr-title` workflow enforces the format.
- Every push to `master` updates a release PR (version bump in `Cargo.toml` and `Formula/stackrig.rb`, changelog). Merging it creates the tag and GitHub release, and attaches `strig-linux-arm64`.

## Repo settings

`scripts/repo-settings.sh` applies merge settings, workflow permissions and branch protection via `gh api`. The `repo-settings` workflow runs it when the script changes, weekly, and on manual dispatch.

It needs the secret `REPO_SETTINGS_TOKEN`: a fine-grained PAT limited to this repo with **Administration: Read and write** (Metadata: Read is implied). Set it with `gh secret set REPO_SETTINGS_TOKEN`. The token expires; renew it when the weekly run fails.
