# Issue tracker: Linear (via MCP)

Issues and PRDs for this repo live in **Linear**, in the **Zingo Mobile** team
(`team` id `e2a5ee38-9519-4447-bfec-581c2b9e838f`). All operations go through the
`linear-server` MCP tools — not a CLI. Most tool schemas are deferred; load them
with `ToolSearch` (e.g. `select:mcp__linear-server__save_issue`) before calling.

There is no GitHub-style issue tracker for triage purposes. External pull
requests on the `zingolabs/zingolib` GitHub repo are **not** a triage surface;
`/triage` reads only from Linear.

## Conventions

Always scope to `team: "Zingo Mobile"`.

- **Create an issue**: `mcp__linear-server__save_issue` with `title`, `description`
  (markdown — use real newlines, not `\n`), and `team`. Set `state` and `labels`
  per `triage-labels.md`.
- **Read an issue**: `mcp__linear-server__get_issue` by id, plus
  `mcp__linear-server__list_comments` for the discussion thread.
- **List issues**: `mcp__linear-server__list_issues` with `team: "Zingo Mobile"`
  and filters (`state`, `label`, `query`, `assignee`, `updatedAt`).
- **Comment on an issue**: `mcp__linear-server__save_comment` with the issue id
  and `body`.
- **Apply labels / change state**: `mcp__linear-server__save_issue` with the issue
  id and updated `labels` / `state`. Create a missing label first with
  `mcp__linear-server__create_issue_label`.
- **List labels / statuses**: `mcp__linear-server__list_issue_labels` and
  `mcp__linear-server__list_issue_statuses`, both scoped to the team.

### Team facts (snapshot — re-list to confirm)

- **Statuses**: Backlog, Todo, In Progress, In Review, Done, Duplicate, Canceled.
- **Labels**: Design, Feature, Bug, Improvement. Triage labels
  (`needs-info`, `ready-for-agent`) are created lazily — see `triage-labels.md`.

## When a skill says "publish to the issue tracker"

Create a Linear issue in the Zingo Mobile team with `save_issue`.

## When a skill says "fetch the relevant ticket"

Resolve the Linear issue with `get_issue` and pull its thread with `list_comments`.
