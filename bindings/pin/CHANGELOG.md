# Changelog

The versions before 0.3.0 predate this file.

## 0.3.0

### BREAKING

- The Bundle Pin carries a `coordinates` object with the Maven coordinate
  of each platform whose bundle is a Maven repository, today `android`.
  `fetch-binding-layer` refuses a pin whose Android digest has no
  coordinate, which an earlier version wrote, and names the
  `pin-binding-layer <commit>` run that rewrites it at the pin's own commit.

### Added

- `pin-binding-layer` downloads the Android bundle into the store the fetch
  reads, writes the coordinate it finds there into the pin, and prints the
  coordinate after the digest. The command takes `--store <dir>` as the
  fetch does.
- `fetch-binding-layer` refuses a layout whose coordinate differs from the
  pin with a message that names both.
