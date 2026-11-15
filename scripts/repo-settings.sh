#!/usr/bin/env bash
# Idempotent: applies the repo settings the release workflows need. Usage: scripts/repo-settings.sh
set -euo pipefail
R=$(gh repo view --json nameWithOwner -q .nameWithOwner)
B=$(gh repo view --json defaultBranchRef -q .defaultBranchRef.name)

# squash-only, PR title becomes the commit message (read by release-please)
gh api -X PATCH "repos/$R" \
  -F allow_squash_merge=true -F allow_merge_commit=false -F allow_rebase_merge=false \
  -f squash_merge_commit_title=PR_TITLE -f squash_merge_commit_message=BLANK \
  -F delete_branch_on_merge=true >/dev/null

# let release-please open PRs
gh api -X PUT "repos/$R/actions/permissions/workflow" \
  -f default_workflow_permissions=write -F can_approve_pull_request_reviews=true

# PR required + title check required (no admin enforcement: you can still bypass)
gh api -X PUT "repos/$R/branches/$B/protection" --input - >/dev/null <<J
{
  "required_status_checks": { "strict": false, "contexts": ["lint"] },
  "enforce_admins": false,
  "required_pull_request_reviews": { "required_approving_review_count": 0 },
  "restrictions": null
}
J
echo "ok: $R ($B)"
