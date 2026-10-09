# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- The crate joins the zingo-netutils workspace. Its own lockfile, `[patch]`
  stanza and nextest configuration move to that workspace, whose CI job tests
  the crate without `live-mixnet`.
