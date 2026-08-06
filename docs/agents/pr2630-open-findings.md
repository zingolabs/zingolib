# PR #2630: open review findings

The multi-agent review of the clap conversion confirmed ten findings.
Nine are resolved on the branch. The parse boundaries now refuse
misplaced session flags and malformed send payloads before any wallet
work. A manual `Debug` impl renders only the variant identifier, so no
memo or key can reach a `Debug` surface. The family enums carry
per-variant `about` metadata, their long help no longer advertises a
nested `help`, the offline harness panics on a parse error instead of
misreading it, the REPL reuses one cached clap model per session, the
usage lines are pinned to the minted names, and the CHANGELOG and ADR
prose follow the punctuation conventions. The wallet-free set has one
statement, `requires_wallet`: the help split derives from it over the
module's one-sample-per-variant list, the `standalone_commands` array
is gone, and a pin holds the rendered standalone section equal to the
derived names.

## Open findings

1. **The REPL leaks clap's process-oriented text.** Typing `--help` at
   the prompt prints the `CommandLine` struct's internal doc-comment as
   the program description. A typo prints `Usage: zingo-cli <COMMAND>`,
   naming a binary the prompt user is not typing, and the old
   "Type 'help'" pointer is gone. `parse_command_tokens` still renders
   the clap error verbatim.

2. **`help` takes one token.** The family syntax now shows in
   `help migration`, but the natural follow-up `help migration start`
   is still a parse error. The full argument syntax lives behind
   `migration start --help`.

## Pending decision

The `nym_arguments_meet_the_absent_feature_not_the_grammar` pin doubles
as a decision instrument. If the shared-grammar validation ruling
stands, that pin is deleted rather than the code changed.
