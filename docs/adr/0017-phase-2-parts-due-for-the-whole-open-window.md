# Phase 2 parts are due for the whole open window, and the random target is advisory

Status: accepted. A deliberate ZIP 318 privacy relaxation the product owner
accepted for responsiveness rather than a bug fix. Scoped to Phase 2 (scheduled
parts).

A scheduled part is due to broadcast for its **entire open window**, meaning it
is in the current bucket, whose boundary sits at or below the tip, rather than
only once the synced tip reaches its random `target_height`. The target is still
computed, stored, and exposed through `BroadcastWindow::latest_target_unix_time`,
read-only, as the user-reminder hint, and it no longer gates sendability. And
`plan_schedule` opens the **first cohort in the current bucket** (floored at
the activation bucket), so the first batch is sendable the instant Phase 2 is
scheduled, its window already open.

## Why

Two product-reported symptoms, both by design before this change:

- **The first batch waited for the next bucket.** `plan_schedule` started
  cohorts at `first_permitted_bucket` = `max(bucket_index(now) + 1, activation
  bucket)`, up to a whole bucket (`M` = 144 blocks, ~3h at the 75-second target
  spacing) before the window even opened.
- **The Send button hid inside an open window.** Each part drew a random target
  uniform in `[boundary, boundary + M)`, and a part was due only once the tip
  reached it, leaving a random dead zone `[boundary, target)`, up to ~3h wide,
  where a manual visit found nothing due.

Every part anchors to its bucket **boundary**, never its target, so making the
target advisory changes no proof, witness, or anchor. Current-bucket anchoring
is already exercised by catch-up, which shifts overdue parts into the current
bucket via `place_immediate`, anchoring them to a boundary the tip has already
passed.

## Trade-off

The random target was the mechanism that unlinked a part's public send time
from its predictable, network-wide boundary height. Made advisory, a client
that sends eagerly at each window opening clusters its sends at boundaries and
leaks that correlation. Dispersion across the window now rests on users
following the reminder (aimed at the random target) rather than on an enforced
gate. That is the accepted privacy cost of the responsiveness win.

Two things are deliberately left unchanged to bound the relaxation:

- `first_permitted_bucket` (the `+ 1` chooser) still governs **expiry rebuild
  and reassignment**, which target a fresh *future* window rather than the
  current one. Only initial scheduling, and `reschedule_parts`, which re-runs it
  before any part is sent, start in the current bucket.
- `upcoming_windows` still lists **strictly future** windows. The current cohort is
  surfaced through `due_now`, not `upcoming_windows`, so the platform scheduler is
  still armed only for windows that have not opened.
