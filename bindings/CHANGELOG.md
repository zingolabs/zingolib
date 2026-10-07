# Binding Layer changelog

The workbench tool `binding-changelog` writes every section below from the lines
that the audited crate changelogs gained after the commit the section names
as `Since`, up to the commit the tool read; the section heading names that
commit. The check in CI regenerates the newest section against the
checked-out commit and fails when the committed section differs. The
audited crates are the two Binding Layer crates and their direct
dependencies in this repository, as `cargo tree` reports them: `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.
A change in another crate of this repository appears only where one of
those changelogs records it.
