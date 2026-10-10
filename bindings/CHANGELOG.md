# Binding Layer changelog

The workbench tool `binding-changelog` writes every section below, one per entry
of `bindings/published.toml`, from the lines that the entry's audited crate changelogs
gained after the previous entry's commit, named as `Since`, up to the
entry's commit, which the section heading names. The check in CI
regenerates every section and fails when the committed file differs. The
audited crates of an entry are the two Binding Layer crates and their
direct dependencies in this repository at the entry's commit, as the
publish workflow recorded them from `cargo tree`; each section names them
in its `Audited` line. A change in another crate of this repository appears
only where one of those changelogs records it.
