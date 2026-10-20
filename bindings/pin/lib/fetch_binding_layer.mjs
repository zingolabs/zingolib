import { writeFile } from 'node:fs/promises';
import { basename, dirname, join } from 'node:path';
import { checkTag, MAVEN_PLATFORMS, PLATFORMS, repository } from './bundle_pin.mjs';
import { exists, replaceDirectory, text } from './disk.mjs';
import { placeMavenRepository, versionNamesCommit } from './maven_repository.mjs';
import { readPin } from './pin_file.mjs';
import { access, download, resolveBundle } from './registry.mjs';
import { placeSwiftPackage } from './swift_package.mjs';

export const MARKER = 'bundle';
export const ARCHIVE_SUFFIX = '.tar.gz';
const EXTRACTED = 'extracted';

export function storePath(root, platform, store) {
  return store ?? join(root, platform, 'build', 'binding-layer');
}

async function held(home) {
  const file = await text(join(home, MARKER));
  if (file === undefined || !file.endsWith(ARCHIVE_SUFFIX)) {
    return { kind: 'missing' };
  }
  const path = join(home, file);
  return (await exists(path)) ? { kind: 'held', path } : { kind: 'missing' };
}

function wrongVersion(commit) {
  return ({ version }) =>
    versionNamesCommit(version, commit)
      ? undefined
      : { kind: 'wrongVersion', platform: 'android', commit, version };
}

async function mavenCoordinate(archive, commit) {
  const placed = await placeMavenRepository(
    archive,
    join(dirname(archive), EXTRACTED),
    wrongVersion(commit),
  );
  if (placed.kind !== 'placed') {
    return placed;
  }
  return { kind: 'fetched', path: placed.path, coordinate: placed.coordinate };
}

export async function fetchBundle(pin, platform, store, always, registry) {
  const pinned = pin.digests[platform];
  if (!pinned) {
    return { kind: 'unpinnedPlatform', platform };
  }
  const home = join(store, pinned.slice('sha256:'.length));
  const disk = await held(home);
  if (disk.kind === 'held' && !always) {
    return { kind: 'fetched', path: disk.path };
  }
  const granted = await registry.access(repository(platform));
  const found =
    granted.kind === 'granted'
      ? await registry.resolveBundle(granted, pin.commit)
      : granted;
  const tag = checkTag(platform, pin.commit, pinned, found);
  if (tag.kind !== 'agrees') {
    return tag;
  }
  if (disk.kind === 'held') {
    return { kind: 'fetched', path: disk.path };
  }
  const [layer] = found.layers;
  if (found.layers.length !== 1 || !layer.file) {
    return { kind: 'badLayout', platform };
  }
  const file = basename(layer.file);
  const replaced = await replaceDirectory(home, async partial => {
    const blob = await registry.download(
      granted,
      layer.digest,
      join(partial, file),
    );
    if (blob.kind !== 'downloaded') {
      return blob;
    }
    await writeFile(join(partial, MARKER), `${file}\n`);
    return { kind: 'filled' };
  });
  return replaced.kind === 'replaced'
    ? { kind: 'fetched', path: join(home, file) }
    : replaced;
}

// The pin command reads each Maven platform's coordinate from the bundle
// itself, downloaded into the store the fetch reads, so the fetch after a
// pin move downloads nothing.
export async function coordinates(pin, storeOf, registry) {
  const found = {};
  for (const platform of MAVEN_PLATFORMS.filter(named => pin.digests[named])) {
    const fetched = await fetchBundle(pin, platform, storeOf(platform), false, registry);
    if (fetched.kind !== 'fetched') {
      return fetched;
    }
    const placed = await mavenCoordinate(fetched.path, pin.commit);
    if (placed.kind !== 'fetched') {
      return placed;
    }
    found[platform] = placed.coordinate;
  }
  return { kind: 'described', coordinates: found };
}

export async function pinnedBundle(root, values) {
  if (values.platform === undefined) {
    return { kind: 'unnamedPlatform' };
  }
  if (!PLATFORMS.includes(values.platform)) {
    return { kind: 'unknownPlatform', platform: values.platform };
  }
  const pin = await readPin(values.pin);
  if (pin.kind !== 'pin') {
    return pin;
  }
  const fetched = await fetchBundle(
    pin,
    values.platform,
    storePath(root, values.platform, values.store),
    values['check-tag'],
    { access, resolveBundle, download },
  );
  if (fetched.kind !== 'fetched') {
    return fetched;
  }
  if (values.platform === 'android') {
    const placed = await mavenCoordinate(fetched.path, pin.commit);
    const pinned = pin.coordinates.android;
    if (placed.kind === 'fetched' && placed.coordinate !== pinned) {
      return {
        kind: 'wrongCoordinate',
        platform: 'android',
        commit: pin.commit,
        pinned,
        found: placed.coordinate,
      };
    }
    return placed;
  }
  const placed = await placeSwiftPackage(
    fetched.path,
    pin.digests.ios,
    join(root, 'ios', 'binding-layer'),
  );
  return placed.kind === 'placed'
    ? { kind: 'fetched', path: placed.path }
    : placed;
}
