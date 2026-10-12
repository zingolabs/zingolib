// The pinned Prebuilt Bundle of one platform: fetchBundle downloads it into
// the store when the store lacks it, and pinnedBundle places the iOS package
// under <root>/ios/binding-layer where the Xcode project references it.

import { mkdir, readdir, rename } from 'node:fs/promises';
import { basename, join } from 'node:path';
import { checkTag, PLATFORMS, repository } from './bundle_pin.mjs';
import { readPin } from './pin_file.mjs';
import { download, resolveBundle } from './registry.mjs';
import { placeSwiftPackage } from './swift_package.mjs';

async function held(home) {
  const files = await readdir(home).catch(failure => {
    if (failure.code === 'ENOENT') {
      return [];
    }
    throw failure;
  });
  return files.length === 1
    ? { kind: 'held', path: join(home, files[0]) }
    : { kind: 'missing' };
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
  const found = await registry.resolveBundle(repository(platform), pin.commit);
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
  const partial = `${home}.partial`;
  const file = basename(layer.file);
  await mkdir(partial, { recursive: true });
  const blob = await registry.download(
    repository(platform),
    layer.digest,
    join(partial, file),
  );
  if (blob.kind !== 'downloaded') {
    return blob;
  }
  await rename(partial, home);
  return { kind: 'fetched', path: join(home, file) };
}

export async function pinnedBundle(root, values) {
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
    { resolveBundle, download },
  );
  if (fetched.kind !== 'fetched' || values.platform !== 'ios') {
    return fetched;
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
