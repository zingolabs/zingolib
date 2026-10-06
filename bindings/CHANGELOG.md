# Binding Layer changelog

The workbench tool `binding-changelog` writes every section below, one per
published commit, from the lines that the audited crate changelogs gained
since the previous published commit. The audited crates are the two Binding
Layer crates and their direct dependencies in this repository, as
`cargo tree` reports them: `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.
A change in another crate of this repository appears only where one of
those changelogs records it.
