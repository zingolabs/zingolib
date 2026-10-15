import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { mock, test } from 'node:test';
import { promisify } from 'node:util';
import {
  coordinateIn,
  coordinateOf,
  placeMavenRepository,
  REPOSITORY,
  versionNamesCommit,
} from '../lib/maven_repository.mjs';

const COMMIT = 'ba7336710419c9ac1e9362ac1b4e5b0073ff477e';
const VERSION = '6.0.0-ba7336710419c9ac';
const COORDINATE = `org.zingolabs:zingo-binding-layer:${VERSION}`;

function metadata(versions) {
  return [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<metadata>',
    '  <groupId>org.zingolabs</groupId>',
    '  <artifactId>zingo-binding-layer</artifactId>',
    '  <versioning>',
    `    <latest>${versions[0] ?? ''}</latest>`,
    '    <versions>',
    ...versions.map(version => `      <version>${version}</version>`),
    '    </versions>',
    '  </versioning>',
    '</metadata>',
  ].join('\n');
}

async function scratch() {
  return mkdtemp(join(tmpdir(), 'bundle-pin-maven-'));
}

async function archive(versions) {
  const root = await scratch();
  const artifact = join(root, REPOSITORY, 'org', 'zingolabs', 'zingo-binding-layer');
  await mkdir(join(artifact, VERSION), { recursive: true });
  if (versions !== undefined) {
    await writeFile(join(artifact, 'maven-metadata.xml'), metadata(versions));
  }
  await writeFile(join(artifact, VERSION, `zingo-binding-layer-${VERSION}.aar`), 'aar');
  const tarball = join(root, 'repository.tar.gz');
  await promisify(execFile)('tar', ['-czf', tarball, '-C', root, REPOSITORY]);
  return tarball;
}

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
  const placed = await placeMavenRepository(tarball, home, extract);
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

test('coordinateOf reads nothing from an absent directory', async () => {
  assert.equal(await coordinateOf(join(await scratch(), 'absent')), undefined);
});
