Fixes: 

Part of: 

## The one semantic change

<!-- One sentence. If it needs "and" to join two behaviours, split the pull request. -->

## Follow-ups

<!-- The issues filed for work this pull request found and left out, one per line. -->

## Renames

<!-- One `old -> new` line for each mechanical rename. -->

## Why this cannot be split

<!-- Required above 450 added lines. AGENTS.md, "Pull request scope", defines the count. -->

A pull request into `dev` merges by merge commit alone, which the ruleset on
`dev` enforces: a release line's tagged candidate has to stay reachable from
`dev` (`docs/sop/SOP001-release.md`, step 6).
