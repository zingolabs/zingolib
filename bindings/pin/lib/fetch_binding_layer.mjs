import { writeFile } from 'node:fs/promises';
import { basename, dirname, join } from 'node:path';
import { checkTag, PLATFORMS, repository } from './bundle_pin.mjs';
import { exists, replaceDirectory, text } from './disk.mjs';
import { placeMavenRepository, versionNamesCommit } from './maven_repository.mjs';
import { readPin } from './pin_file.mjs';
import { access, download, resolveBundle } from './registry.mjs';
import { placeSwiftPackage } from './swift_package.mjs';

export const MARKER = 'bundle';
export const ARCHIVE_SUFFIX = '.tar.gz';
const EXTRACTED = 'extracted';

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
    values.store ?? join(root, values.platform, 'build', 'binding-layer'),
    values['check-tag'],
    { access, resolveBundle, download },
  );
  if (fetched.kind !== 'fetched') {
    return fetched;
  }
  if (values.platform === 'android') {
    return mavenCoordinate(fetched.path, pin.commit);
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
