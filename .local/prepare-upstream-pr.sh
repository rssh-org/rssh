#!/usr/bin/env bash
set -euo pipefail

readonly BASE_BRANCH="main"
readonly FORK_REMOTE="fork"
readonly DEFAULT_PR_TITLE="feat(terminal): add workspace terminal pane splitting"

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

github_repo_from_url() {
  local url="${1%.git}"

  case "$url" in
    https://github.com/*)
      printf '%s\n' "${url#https://github.com/}"
      ;;
    git@github.com:*)
      printf '%s\n' "${url#git@github.com:}"
      ;;
    ssh://git@github.com/*)
      printf '%s\n' "${url#ssh://git@github.com/}"
      ;;
    *)
      return 1
      ;;
  esac
}

command -v gh >/dev/null || fail "GitHub CLI is required; install it from https://cli.github.com/"
gh auth status >/dev/null || fail "authenticate first with: gh auth login"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

branch="$(git branch --show-current)"
[[ -n "$branch" ]] || fail "detached HEAD; switch to the branch you want to submit"
[[ "$branch" != "$BASE_BRANCH" ]] || fail "refusing to submit the base branch '$BASE_BRANCH'"
git remote get-url origin >/dev/null || fail "missing the upstream remote 'origin'"

git diff --quiet || fail "working tree has unstaged changes; commit or stash them first"
git diff --cached --quiet || fail "index has staged changes; commit or stash them first"

upstream="$(github_repo_from_url "$(git remote get-url origin)")" || \
  fail "origin must point to a github.com repository"

if git remote get-url "$FORK_REMOTE" >/dev/null 2>&1; then
  fork_repo="$(github_repo_from_url "$(git remote get-url "$FORK_REMOTE")")" || \
    fail "remote '$FORK_REMOTE' must point to a github.com repository"
else
  github_login="$(gh api user --jq .login)"
  fork_repo="$github_login/${upstream#*/}"

  if gh repo view "$fork_repo" >/dev/null 2>&1; then
    git remote add "$FORK_REMOTE" "https://github.com/$fork_repo.git"
  else
    gh repo fork "$upstream" --remote --remote-name "$FORK_REMOTE"
  fi
fi

git push --set-upstream "$FORK_REMOTE" "$branch"

fork_owner="${fork_repo%%/*}"
compare_url="https://github.com/$upstream/compare/$BASE_BRANCH...$fork_owner:$branch?expand=1"

if [[ "${CREATE_PR:-0}" == "1" ]]; then
  pr_title="${PR_TITLE:-$DEFAULT_PR_TITLE}"
  pr_body="${PR_BODY:-Please review the terminal workspace pane splitting changes.}"
  existing_pr="$(gh pr list --repo "$upstream" --head "$fork_owner:$branch" \
    --base "$BASE_BRANCH" --state open --json url --jq '.[0].url // empty')"
  if [[ -n "$existing_pr" ]]; then
    printf '\nPull request already exists:\n%s\n' "$existing_pr"
  else
    gh pr create --repo "$upstream" --head "$fork_owner:$branch" \
      --base "$BASE_BRANCH" --title "$pr_title" --body "$pr_body"
  fi
else
  printf '\nOpen this URL to review and create the pull request:\n%s\n' "$compare_url"
  printf 'Set CREATE_PR=1 to create it with gh, optionally using PR_TITLE and PR_BODY.\n'
fi
