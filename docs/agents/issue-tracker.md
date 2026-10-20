# Issue tracker: GitHub Issues (via `gh`)

Issues for this repo live in **GitHub Issues** on `zingolabs/zingolib`. All
operations go through the `gh` CLI. Linear holds the Zingo Mobile team's
issues only, and nothing in this repo is filed there.

## Conventions

Always pass `--repo zingolabs/zingolib` so a command run from a submodule or a
worktree still reaches the right tracker.

- **Create an issue**: `gh issue create --title <title> --body-file <file>
  --label <label>`. Write the body to a file first (markdown, real newlines),
  never inline through a heredoc. Apply the component label that fits
  (`Pepper-Sync`, `sync`, `zingo-cli`, ...) beside `bug` or `enhancement`, and
  a triage label per `triage-labels.md`.
- **Read an issue**: `gh issue view <number> --comments`.
- **List issues**: `gh issue list` with `--label`, `--state`, `--search`,
  `--assignee`.
- **Comment on an issue**: `gh issue comment <number> --body-file <file>`.
- **Apply or remove labels**: `gh issue edit <number> --add-label <label>` and
  `--remove-label <label>`. Create a missing label first with `gh label create
  <label> --description <text>`.
- **Close**: `gh issue close <number>`, with `--reason "not planned"` for
  `wontfix`.
- **List labels**: `gh label list --limit 100`.

A pull request that fixes an issue says so in its body (`Fixes #<number>`) so
the merge closes it.

## When a skill says "publish to the issue tracker"

Create a GitHub issue on `zingolabs/zingolib` with `gh issue create`.

## When a skill says "fetch the relevant ticket"

Resolve the issue with `gh issue view <number> --comments`.
