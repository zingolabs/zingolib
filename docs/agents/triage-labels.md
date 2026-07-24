# Triage Labels

The skills speak in terms of five canonical triage roles. This repo tracks issues
in **Linear** (Zingo Mobile team), so roles are expressed as a mix of Linear
**statuses** (where one fits the role) and **labels** (for the two roles statuses
can't express). See `issue-tracker.md` for the MCP tooling.

| Role in mattpocock/skills | How we express it in Linear            | Meaning                                  |
| ------------------------- | -------------------------------------- | ---------------------------------------- |
| `needs-triage`            | status **Backlog**                     | Maintainer needs to evaluate this issue  |
| `needs-info`              | label **`needs-info`**                 | Waiting on reporter for more information |
| `ready-for-agent`         | status **Todo** + label **`ready-for-agent`** | Fully specified, ready for an AFK agent  |
| `ready-for-human`         | status **Todo**                        | Requires human implementation            |
| `wontfix`                 | status **Canceled**                    | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), translate
it via this table: set the Linear **status** and/or apply the **label** shown.

Notes:

- `needs-info` and `ready-for-agent` are labels that don't exist in the team yet —
  create them on first use with `mcp__linear-server__create_issue_label`
  (`team: "Zingo Mobile"`), then apply via `save_issue`.
- `ready-for-agent` and `ready-for-human` share the **Todo** status; the
  `ready-for-agent` **label** is what distinguishes an AFK-ready issue from one
  that needs a human. An issue in **Todo** with no `ready-for-agent` label is
  `ready-for-human`.

Edit this table if the workflow changes.
