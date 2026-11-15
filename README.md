# stackrig (`strig`)

CLI that runs Coolify-generated compose stacks without Coolify. See `strig help`.

## Releases

Versioning is automated with [release-please](https://github.com/googleapis/release-please).

- Commits and PR titles must follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `feat!:`). `ci:`/`chore:`/`docs:` do not trigger a release.
- Only squash merges are allowed; the PR title becomes the commit message. The `pr-title` workflow enforces the format.
- Every push to `master` updates a release PR (version bump in `Cargo.toml`, changelog). Merging it creates the tag and GitHub release, and attaches `strig-linux-arm64`.

## Repo settings

`scripts/repo-settings.sh` applies merge settings, workflow permissions and branch protection via `gh api`. The `repo-settings` workflow runs it when the script changes, weekly, and on manual dispatch.

It needs the secret `REPO_SETTINGS_TOKEN`: a fine-grained PAT limited to this repo with **Administration: Read and write** (Metadata: Read is implied). Set it with `gh secret set REPO_SETTINGS_TOKEN`. The token expires; renew it when the weekly run fails.
