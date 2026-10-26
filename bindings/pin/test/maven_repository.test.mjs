import assert from 'node:assert/strict';
import { readdir } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { mock, test } from 'node:test';
import {
  coordinateIn,
  coordinateOf,
  placeMavenRepository,
  REPOSITORY,
  versionNamesCommit,
} from '../lib/maven_repository.mjs';
import { archive, COMMIT, COORDINATE, metadata, scratch, VERSION } from './maven_fixture.mjs';

test('coordinateIn reads the one version of the artifact metadata', () => {
  assert.deepEqual(coordinateIn(metadata([VERSION])), {
    version: VERSION,
    coordinate: COORDINATE,
  });
});

test('coordinateIn reads nothing from metadata with two versions or none', () => {
  assert.equal(coordinateIn(metadata([VERSION, '6.0.0-cafe'])), undefined);
  assert.equal(coordinateIn(metadata([])), undefined);
  assert.equal(coordinateIn('<metadata><groupId>g</groupId></metadata>'), undefined);
});

test('versionNamesCommit accepts the version that ends with sixteen characters of the commit', () => {
  assert.equal(versionNamesCommit(VERSION, COMMIT), true);
  assert.equal(versionNamesCommit('6.0.0-ba733', COMMIT), false);
  assert.equal(versionNamesCommit(VERSION, 'c'.repeat(40)), false);
});

test('placeMavenRepository extracts the layout and reports its coordinate', async () => {
  const home = join(await scratch(), 'extracted');
  const placed = await placeMavenRepository(await archive([VERSION]), home);
  assert.deepEqual(placed, {
    kind: 'placed',
    path: join(home, REPOSITORY),
    version: VERSION,
    coordinate: COORDINATE,
  });
  assert.deepEqual(await coordinateOf(join(home, REPOSITORY)), {
    version: VERSION,
    coordinate: COORDINATE,
  });
});

test('placeMavenRepository extracts nothing when the layout is already in place', async () => {
  const home = join(await scratch(), 'extracted');
  const tarball = await archive([VERSION]);
  await placeMavenRepository(tarball, home);
  const extract = mock.fn();
  const placed = await placeMavenRepository(tarball, home, undefined, extract);
  assert.equal(placed.kind, 'placed');
  assert.equal(placed.coordinate, COORDINATE);
  assert.equal(extract.mock.callCount(), 0);
});

test('placeMavenRepository leaves nothing when the archive holds no artifact metadata', async () => {
  const home = join(await scratch(), 'extracted');
  const placed = await placeMavenRepository(await archive(undefined), home);
  assert.deepEqual(placed, { kind: 'badRepository', platform: 'android' });
  assert.deepEqual(await readdir(dirname(home)), []);
});

test('placeMavenRepository leaves nothing when the refusal rejects the layout', async () => {
  const home = join(await scratch(), 'extracted');
  const refused = { kind: 'wrongVersion', version: VERSION };
  const placed = await placeMavenRepository(await archive([VERSION]), home, () => refused);
  assert.deepEqual(placed, refused);
  assert.deepEqual(await readdir(dirname(home)), []);
});

test('placeMavenRepository refuses a held layout that the refusal rejects', async () => {
  const home = join(await scratch(), 'extracted');
  const tarball = await archive([VERSION]);
  await placeMavenRepository(tarball, home);
  const refused = { kind: 'wrongVersion', version: VERSION };
  const placed = await placeMavenRepository(tarball, home, () => refused);
  assert.deepEqual(placed, refused);
});

test('coordinateOf reads nothing from an absent directory', async () => {
  assert.equal(await coordinateOf(join(await scratch(), 'absent')), undefined);
});
