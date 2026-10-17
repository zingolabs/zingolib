# Triage Labels

The skills speak in terms of five canonical triage roles. This repo tracks issues
in **GitHub Issues**, so every role is a GitHub **label**, with `wontfix` also
closing the issue. See `issue-tracker.md` for the `gh` tooling.

| Role in mattpocock/skills | How we express it on GitHub                  | Meaning                                  |
| ------------------------- | -------------------------------------------- | ---------------------------------------- |
| `needs-triage`            | label **`needs-triage`**                     | Maintainer needs to evaluate this issue  |
| `needs-info`              | label **`needs-info`**                       | Waiting on reporter for more information |
| `ready-for-agent`         | label **`ready-for-agent`**                  | Fully specified, ready for an AFK agent  |
| `ready-for-human`         | label **`ready-for-human`**                  | Requires human implementation            |
| `wontfix`                 | label **`wontfix`** + close as "not planned" | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), translate
it via this table: apply the **label** shown with `gh issue edit --add-label`,
and remove the role it replaces.

Notes:

- `wontfix` exists in the repo. The other four are created on first use with
  `gh label create <label> --description <meaning>`.
- An issue carries one triage role at a time. Moving an issue from
  `needs-triage` to `ready-for-agent` removes the first label and adds the
  second.

Edit this table if the workflow changes.
