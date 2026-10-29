# Standard operating procedures

A standard operating procedure (SOP) is the written form of a recurring
task of this repository: the steps, in order, with the names and the
checks each step uses. An SOP is edited in place as the task changes, and
one SOP covers one task. Where a decision stands behind a procedure, the
decision is a zingo-adrs record and the SOP cites it. The record says why,
and the SOP says how.

## Numbering

An SOP is a file `SOPNNN-<topic>.md` in this directory. The number is
assigned once, from the next free one, and never changes, so a citation
by number holds wherever it was written, in a record, an issue or a
comment. A renamed topic keeps its number. A retired SOP keeps its file
with a first line saying what replaced it, and its number is not reused.

## Index

| Number | Procedure | Governing record |
| --- | --- | --- |
| SOP001 | [Releasing from dev](SOP001-release.md) | zingo-adrs zingolib/0064 |
