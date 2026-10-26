# Changelog

The versions before 0.3.0 predate this file.

## 0.4.0

### BREAKING

- The package needs Node 22.13.0 or later, the first release whose
  permission model is stable.

- Both commands run under a capability bound that the consumer imposes:
  two scripts in the consumer's `package.json`, shown verbatim in the
  README, run Node with `--permission` and open only the paths each
  command touches. Gradle, Xcode and CI call the scripts, not the
  `node_modules/.bin` links.

- `fetch-binding-layer` extracts in process and refuses an archive that is
  not in the pax tar format: a GNU long-name entry, a link, or a path that
  leaves the archive root is refused with a message that names it. Every
  Prebuilt Bundle published before zingolib packed in the pax format holds
  long-name entries for its Maven paths, so none of them fetches at this
  version. Pin a commit published later.

### Added

- `lib/tar_archive.mjs` inflates and extracts the archive with `node:zlib`
  and a reader of the package's own, so the package spawns no process.
- `test/permission.test.mjs` runs both commands under the README's script
  lines against a consumer fixture, and runs the iOS fetch once more with
  the partial path closed to pin the refusal.

### Changed

- The iOS swap happens one level under `ios/binding-layer`: the package is
  replaced through `ZingoBindings.partial` beside it and the digest marker
  is written after the swap, since a directory the bound opens covers its
  children and never itself. The package path Xcode references does not
  move.

### Removed

- The `tar` spawn, and with it the only reason a consumer would open child
  processes to the package.

## 0.3.0

### BREAKING

- The Bundle Pin carries a `coordinates` object with the Maven coordinate
  of each platform whose bundle is a Maven repository, today `android`.
  `fetch-binding-layer` refuses a pin whose Android digest has no
  coordinate, which an earlier version wrote, and names the
  `pin-binding-layer <commit>` run that rewrites it at the pin's own commit.

- `pin-binding-layer` prints the platform, the digest and the coordinate;
  the descriptor leaves the line, and zingolib's publish no longer writes it
  into the manifest. A pin whose coordinate's version does not name the
  pin's commit is refused as malformed by both commands.

### Added

- `pin-binding-layer` writes the coordinate the Android bundle's manifest
  names into the pin and prints it after the digest. zingolib's publish
  annotates the manifest with the coordinate it reads from the Maven
  metadata in the bundle. The command refuses a Maven bundle whose manifest
  names no coordinate, which zingolib published before the annotation.
- `fetch-binding-layer` refuses a layout whose coordinate differs from the
  pin with a message that names both.
