import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { mock, test } from 'node:test';
import {
  ARCHIVE_MEDIA_TYPE,
  movePin,
  parsePin,
  serializePin,
} from '../lib/bundle_pin.mjs';
import { fetchBundle, MARKER, pinnedBundle } from '../lib/fetch_binding_layer.mjs';
import { writePin } from '../lib/pin_file.mjs';
import { archive, COMMIT as MAVEN_COMMIT, COORDINATE } from './maven_fixture.mjs';

const COMMIT = 'a'.repeat(40);
const NEXT = 'b'.repeat(40);
const ANDROID = `sha256:${'1'.repeat(64)}`;
const IOS = `sha256:${'2'.repeat(64)}`;
const LAYER = `sha256:${'3'.repeat(64)}`;
const FILE = 'repository.tar.gz';
const OLD_FILE = 'binding-layer.aar';
const OLD_MEDIA_TYPE = 'application/zip';

const pin = {
  kind: 'pin',
  commit: COMMIT,
  digests: { android: ANDROID },
  coordinates: { android: COORDINATE },
};

function bundle(digest, revision, layer = { file: FILE, mediaType: ARCHIVE_MEDIA_TYPE }) {
  return {
    kind: 'bundle',
    digest,
    revision,
    descriptor: 'zl_3.0.0_aaaaa',
    coordinate: COORDINATE,
    layers: [{ digest: LAYER, ...layer }],
  };
}

function oldBundle(digest, revision) {
  return bundle(digest, revision, { file: OLD_FILE, mediaType: OLD_MEDIA_TYPE });
}

function registry(found) {
  return {
    access: mock.fn(async repository => ({
      kind: 'granted',
      repository,
      headers: {},
    })),
    resolveBundle: mock.fn(async () => found),
    download: mock.fn(async (_granted, _digest, path) => {
      await writeFile(path, 'bundle');
      return { kind: 'downloaded', path };
    }),
  };
}

async function store() {
  return mkdtemp(join(tmpdir(), 'bundle-pin-store-'));
}

async function storeWithBundle() {
  const root = await store();
  const home = join(root, ANDROID.slice('sha256:'.length));
  await mkdir(home);
  await writeFile(join(home, FILE), 'bundle');
  await writeFile(join(home, MARKER), `${FILE}\n`);
  return { root, path: join(home, FILE) };
}

test('parsePin reads the pin that serializePin wrote', () => {
  const written = {
    commit: COMMIT,
    digests: { ios: IOS, android: ANDROID },
    coordinates: { android: COORDINATE },
  };
  assert.deepEqual(parsePin(serializePin(written)), { kind: 'pin', ...written });
});

test('serializePin writes the commit, the digests in order, then the coordinates', () => {
  assert.equal(
    serializePin({
      commit: COMMIT,
      digests: { ios: IOS, android: ANDROID },
      coordinates: { android: COORDINATE },
    }),
    `{\n  "commit": "${COMMIT}",\n  "digests": {\n    "android": "${ANDROID}",\n    "ios": "${IOS}"\n  },\n  "coordinates": {\n    "android": "${COORDINATE}"\n  }\n}\n`,
  );
});

test('serializePin writes no coordinates for a pin of platforms without a Maven layout', () => {
  assert.equal(
    serializePin({ commit: COMMIT, digests: { ios: IOS }, coordinates: {} }),
    `{\n  "commit": "${COMMIT}",\n  "digests": {\n    "ios": "${IOS}"\n  }\n}\n`,
  );
});

test('parsePin reads a pin of platforms without a Maven layout and no coordinates', () => {
  const text = JSON.stringify({ commit: COMMIT, digests: { ios: IOS } });
  assert.deepEqual(parsePin(text), {
    kind: 'pin',
    commit: COMMIT,
    digests: { ios: IOS },
    coordinates: {},
  });
});

test('parsePin reads a pin of an earlier version, whose android digest has no coordinate, as old', () => {
  const text = JSON.stringify({ commit: COMMIT, digests: { android: ANDROID } });
  assert.deepEqual(parsePin(text), {
    kind: 'oldPin',
    commit: COMMIT,
    digests: { android: ANDROID },
    coordinates: {},
  });
});

test('movePin moves the platforms an old pin holds', async () => {
  const old = { kind: 'oldPin', commit: COMMIT, digests: { android: ANDROID }, coordinates: {} };
  const moved = await movePin(old, NEXT, [], async () => bundle(IOS, NEXT));
  assert.deepEqual(moved.pin, {
    commit: NEXT,
    digests: { android: IOS },
    coordinates: { android: COORDINATE },
  });
});

test('movePin checks the tag of an old pin at its own commit', async () => {
  const old = { kind: 'oldPin', commit: COMMIT, digests: { android: ANDROID }, coordinates: {} };
  const moved = await movePin(old, COMMIT, [], async () => bundle(IOS, COMMIT));
  assert.equal(moved.kind, 'movedTag');
});

test('pinnedBundle refuses an old pin and names its commit', async () => {
  const root = await store();
  const path = join(root, 'binding-layer.json');
  await writeFile(path, JSON.stringify({ commit: COMMIT, digests: { android: ANDROID } }));
  const outcome = await pinnedBundle(root, { platform: 'android', pin: path });
  assert.equal(outcome.kind, 'oldPin');
  assert.equal(outcome.commit, COMMIT);
});

test('parsePin names the android coordinate when it is not group, artifact and version', () => {
  const text = JSON.stringify({
    commit: COMMIT,
    digests: { android: ANDROID },
    coordinates: { android: 'org.zingolabs:zingo-binding-layer' },
  });
  assert.deepEqual(parsePin(text), { kind: 'malformedPin', field: 'coordinates.android' });
});

test('parsePin names a coordinate of a platform without a Maven layout', () => {
  const text = JSON.stringify({
    commit: COMMIT,
    digests: { android: ANDROID, ios: IOS },
    coordinates: { android: COORDINATE, ios: COORDINATE },
  });
  assert.deepEqual(parsePin(text), { kind: 'malformedPin', field: 'coordinates.ios' });
});

test('parsePin names the commit when the commit is abbreviated', () => {
  const text = JSON.stringify({ commit: 'aaaaa', digests: { android: ANDROID } });
  assert.deepEqual(parsePin(text), { kind: 'malformedPin', field: 'commit' });
});

test('parsePin names the JSON when the file does not parse', () => {
  assert.deepEqual(parsePin('{"commit": }'), { kind: 'malformedPin', field: 'JSON' });
});

test('parsePin names the commit when the file holds null', () => {
  assert.deepEqual(parsePin('null'), { kind: 'malformedPin', field: 'commit' });
});

test('parsePin names the platform when its digest is malformed', () => {
  const text = JSON.stringify({ commit: COMMIT, digests: { android: 'sha256:1' } });
  assert.deepEqual(parsePin(text), { kind: 'malformedPin', field: 'digests.android' });
});

test('movePin moves every listed platform when each has a bundle for the commit', async () => {
  const current = { ...pin, digests: { android: ANDROID, ios: IOS } };
  const moved = await movePin(current, NEXT, [], async platform =>
    bundle(platform === 'android' ? IOS : ANDROID, NEXT),
  );
  assert.deepEqual(moved.pin, {
    commit: NEXT,
    digests: { android: IOS, ios: ANDROID },
    coordinates: { android: COORDINATE },
  });
});

test('movePin records the coordinate the manifest of a Maven bundle names, and none for the other platforms', async () => {
  const resolve = mock.fn(async platform => bundle(platform === 'ios' ? IOS : ANDROID, NEXT));
  const moved = await movePin({ kind: 'unpinned' }, NEXT, ['ios'], resolve);
  assert.deepEqual(moved.pin, { commit: NEXT, digests: { ios: IOS }, coordinates: {} });
  assert.equal(resolve.mock.callCount(), 1);
});

test('movePin refuses a Maven bundle whose manifest names no coordinate', async () => {
  const moved = await movePin(pin, NEXT, [], async () => ({
    ...bundle(ANDROID, NEXT),
    coordinate: undefined,
  }));
  assert.deepEqual(moved, { kind: 'unannotated', platform: 'android', commit: NEXT });
});

test('movePin refuses a Maven bundle whose manifest coordinate is not group, artifact and version', async () => {
  const moved = await movePin(pin, NEXT, [], async () => ({
    ...bundle(ANDROID, NEXT),
    coordinate: 'org.zingolabs:zingo-binding-layer',
  }));
  assert.deepEqual(moved, {
    kind: 'badCoordinate',
    platform: 'android',
    commit: NEXT,
    coordinate: 'org.zingolabs:zingo-binding-layer',
  });
});

test('movePin returns no pin when one listed platform has no bundle for the commit', async () => {
  const current = { ...pin, digests: { android: ANDROID, ios: IOS } };
  const moved = await movePin(current, NEXT, [], async platform =>
    platform === 'ios' ? { kind: 'absent' } : bundle(ANDROID, NEXT),
  );
  assert.deepEqual(moved, { kind: 'absent', platform: 'ios', commit: NEXT });
});

test('movePin returns no pin when the registry refuses access to a platform', async () => {
  const moved = await movePin(pin, NEXT, [], async () => ({ kind: 'refused' }));
  assert.deepEqual(moved, { kind: 'refused', platform: 'android', commit: NEXT });
});

test('movePin reports a moved tag when the pinned commit resolves to another digest', async () => {
  const moved = await movePin(pin, COMMIT, [], async () => bundle(IOS, COMMIT));
  assert.deepEqual(moved, {
    kind: 'movedTag',
    platform: 'android',
    commit: COMMIT,
    pinned: ANDROID,
    found: IOS,
  });
});

test("movePin reports the bundle's own commit when it differs from the tag", async () => {
  const moved = await movePin(pin, NEXT, [], async () => bundle(ANDROID, COMMIT));
  assert.deepEqual(moved, {
    kind: 'wrongRevision',
    platform: 'android',
    commit: NEXT,
    revision: COMMIT,
  });
});

test('movePin adds a platform when the flag names it', async () => {
  const moved = await movePin(pin, COMMIT, ['ios'], async platform =>
    bundle(platform === 'ios' ? IOS : ANDROID, COMMIT),
  );
  assert.deepEqual(moved.pin.digests, { android: ANDROID, ios: IOS });
});

test('movePin asks the registry nothing when the commit is abbreviated', async () => {
  const resolve = mock.fn();
  const moved = await movePin(pin, 'aaaaa', [], resolve);
  assert.deepEqual(moved, { kind: 'badCommit', commit: 'aaaaa' });
  assert.equal(resolve.mock.callCount(), 0);
});

test('fetchBundle asks the registry nothing when the checkout holds the pinned digest', async () => {
  const { root, path } = await storeWithBundle();
  const requests = registry(bundle(ANDROID, COMMIT));
  const fetched = await fetchBundle(pin, 'android', root, false, requests);
  assert.deepEqual(fetched, { kind: 'fetched', path });
  assert.equal(requests.resolveBundle.mock.callCount(), 0);
});

test("fetchBundle downloads into the digest's directory when the checkout lacks the bundle", async () => {
  const root = await store();
  const requests = registry(bundle(ANDROID, COMMIT));
  const fetched = await fetchBundle(pin, 'android', root, false, requests);
  assert.deepEqual(fetched, {
    kind: 'fetched',
    path: join(root, ANDROID.slice('sha256:'.length), FILE),
  });
  assert.equal(requests.download.mock.calls[0].arguments[1], LAYER);
  assert.equal(requests.access.mock.callCount(), 1);
  assert.equal(await readFile(join(root, ANDROID.slice('sha256:'.length), MARKER), 'utf8'), `${FILE}\n`);
});

test('fetchBundle holds the bundle beside a dotfile in its directory', async () => {
  const { root, path } = await storeWithBundle();
  await writeFile(join(dirname(path), '.DS_Store'), '');
  const requests = registry(bundle(ANDROID, COMMIT));
  const fetched = await fetchBundle(pin, 'android', root, false, requests);
  assert.deepEqual(fetched, { kind: 'fetched', path });
  assert.equal(requests.download.mock.callCount(), 0);
});

test("fetchBundle replaces the digest's directory when no marker names its bundle", async () => {
  const root = await store();
  const home = join(root, ANDROID.slice('sha256:'.length));
  await mkdir(home);
  await writeFile(join(home, FILE), 'stale');
  const requests = registry(bundle(ANDROID, COMMIT));
  const fetched = await fetchBundle(pin, 'android', root, false, requests);
  assert.deepEqual(fetched, { kind: 'fetched', path: join(home, FILE) });
  assert.deepEqual((await readdir(home)).sort(), [FILE, MARKER].sort());
  assert.equal(await readFile(join(home, FILE), 'utf8'), 'bundle');
});

test('fetchBundle leaves no directory when the download is corrupt', async () => {
  const root = await store();
  const requests = registry(bundle(ANDROID, COMMIT));
  requests.download = mock.fn(async () => ({ kind: 'corrupt', expected: LAYER, found: IOS }));
  const fetched = await fetchBundle(pin, 'android', root, false, requests);
  assert.equal(fetched.kind, 'corrupt');
  assert.deepEqual(await readdir(root), []);
});

test('fetchBundle downloads nothing when the tag resolves to another digest', async () => {
  const requests = registry(bundle(IOS, COMMIT));
  const fetched = await fetchBundle(pin, 'android', await store(), false, requests);
  assert.equal(fetched.kind, 'movedTag');
  assert.equal(requests.download.mock.callCount(), 0);
});

test('fetchBundle reports a moved tag when asked to check, although the checkout holds the bundle', async () => {
  const { root } = await storeWithBundle();
  const requests = registry(bundle(IOS, COMMIT));
  const fetched = await fetchBundle(pin, 'android', root, true, requests);
  assert.equal(fetched.kind, 'movedTag');
});

test('pinnedBundle refuses before reading the pin when no platform is named', async () => {
  const outcome = await pinnedBundle(join(tmpdir(), 'absent'), {});
  assert.deepEqual(outcome, { kind: 'unnamedPlatform' });
});

test('movePin refuses a bundle that is not a gzip archive', async () => {
  const moved = await movePin(pin, NEXT, [], async () => oldBundle(ANDROID, NEXT));
  assert.deepEqual(moved, { kind: 'oldShape', platform: 'android', commit: NEXT });
});

test('fetchBundle downloads nothing from a bundle that is not a gzip archive', async () => {
  const requests = registry(oldBundle(ANDROID, COMMIT));
  const fetched = await fetchBundle(pin, 'android', await store(), false, requests);
  assert.deepEqual(fetched, { kind: 'oldShape', platform: 'android', commit: COMMIT });
  assert.equal(requests.download.mock.callCount(), 0);
});

test('fetchBundle asks the registry again when the checkout holds a bundle of the old shape', async () => {
  const root = await store();
  const home = join(root, ANDROID.slice('sha256:'.length));
  await mkdir(home);
  await writeFile(join(home, OLD_FILE), 'bundle');
  await writeFile(join(home, MARKER), `${OLD_FILE}\n`);
  const requests = registry(oldBundle(ANDROID, COMMIT));
  const fetched = await fetchBundle(pin, 'android', root, false, requests);
  assert.equal(fetched.kind, 'oldShape');
  assert.equal(requests.resolveBundle.mock.callCount(), 1);
});

const mavenPin = { commit: MAVEN_COMMIT, digests: { android: ANDROID } };

async function checkoutWithPin(coordinate) {
  const root = await store();
  const path = join(root, 'binding-layer.json');
  await writePin(path, { ...mavenPin, coordinates: { android: coordinate } });
  const home = join(root, 'android', 'build', 'binding-layer', ANDROID.slice('sha256:'.length));
  await mkdir(home, { recursive: true });
  await writeFile(join(home, FILE), await readFile(await archive([COORDINATE.split(':')[2]])));
  await writeFile(join(home, MARKER), `${FILE}\n`);
  return { root, path, home };
}

test('pinnedBundle prints the repository and the coordinate the pin holds', async () => {
  const { root, path, home } = await checkoutWithPin(COORDINATE);
  const outcome = await pinnedBundle(root, { platform: 'android', pin: path });
  assert.deepEqual(outcome, {
    kind: 'fetched',
    path: join(home, 'extracted', 'repository'),
    coordinate: COORDINATE,
  });
});

test('pinnedBundle refuses a bundle whose coordinate differs from the pin', async () => {
  const pinned = 'org.zingolabs:zingo-binding-layer:6.0.0-cafecafecafecafe';
  const { root, path } = await checkoutWithPin(pinned);
  const outcome = await pinnedBundle(root, { platform: 'android', pin: path });
  assert.deepEqual(outcome, {
    kind: 'wrongCoordinate',
    platform: 'android',
    commit: MAVEN_COMMIT,
    pinned,
    found: COORDINATE,
  });
});
