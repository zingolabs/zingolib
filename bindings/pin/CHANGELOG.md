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

- `pin-binding-layer` writes the coordinate the Android bundle's manifest
  names into the pin and prints it after the digest. zingolib's publish
  annotates the manifest with the coordinate it reads from the Maven
  metadata in the bundle. The command refuses a Maven bundle whose manifest
  names no coordinate, which zingolib published before the annotation.
- `fetch-binding-layer` refuses a layout whose coordinate differs from the
  pin with a message that names both.
