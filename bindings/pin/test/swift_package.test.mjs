import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, readdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { mock, test } from 'node:test';
import { promisify } from 'node:util';
import { MARKER, PACKAGE, placeSwiftPackage, placement } from '../lib/swift_package.mjs';

const DIGEST = `sha256:${'1'.repeat(64)}`;
const NEXT = `sha256:${'2'.repeat(64)}`;

async function scratch() {
  return mkdtemp(join(tmpdir(), 'bundle-pin-swift-'));
}

async function archive(withManifest) {
  const root = await scratch();
  await mkdir(join(root, PACKAGE, 'build'), { recursive: true });
  if (withManifest) {
    await writeFile(join(root, PACKAGE, 'Package.swift'), '// swift-tools-version:5.9\n');
  }
  const tarball = join(root, 'ZingoBindings.tar.gz');
  await promisify(execFile)('tar', ['--create', '--gzip', '--format=pax', '--file', tarball, '-C', root, PACKAGE]);
  return tarball;
}

test('placement holds the package when the marker names the digest and the manifest is present', () => {
  assert.equal(placement(DIGEST, DIGEST, true), 'held');
  assert.equal(placement(NEXT, DIGEST, true), 'extract');
  assert.equal(placement(undefined, DIGEST, true), 'extract');
  assert.equal(placement(DIGEST, DIGEST, false), 'extract');
});

test('placeSwiftPackage extracts the package to the fixed path and writes the marker', async () => {
  const home = join(await scratch(), 'binding-layer');
  const placed = await placeSwiftPackage(await archive(true), DIGEST, home);
  assert.deepEqual(placed, { kind: 'placed', path: join(home, PACKAGE) });
  assert.equal(await readFile(join(home, MARKER), 'utf8'), `${DIGEST}\n`);
  assert.match(await readFile(join(home, PACKAGE, 'Package.swift'), 'utf8'), /swift-tools-version/);
});

test('placeSwiftPackage extracts nothing when the checkout holds the pinned digest', async () => {
  const home = join(await scratch(), 'binding-layer');
  const tarball = await archive(true);
  await placeSwiftPackage(tarball, DIGEST, home);
  const extract = mock.fn();
  const placed = await placeSwiftPackage(tarball, DIGEST, home, extract);
  assert.equal(placed.kind, 'placed');
  assert.equal(extract.mock.callCount(), 0);
});

test('placeSwiftPackage extracts again when the pin moves', async () => {
  const home = join(await scratch(), 'binding-layer');
  const tarball = await archive(true);
  await placeSwiftPackage(tarball, DIGEST, home);
  const placed = await placeSwiftPackage(tarball, NEXT, home);
  assert.equal(placed.kind, 'placed');
  assert.equal(await readFile(join(home, MARKER), 'utf8'), `${NEXT}\n`);
});

test('placeSwiftPackage reports a bundle that holds no package manifest', async () => {
  const home = join(await scratch(), 'binding-layer');
  const placed = await placeSwiftPackage(await archive(false), DIGEST, home);
  assert.deepEqual(placed, { kind: 'badPackage', platform: 'ios' });
  assert.deepEqual(await readdir(home), []);
});
