import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { mock, test } from 'node:test';
import { movePin, parsePin, serializePin } from '../lib/bundle_pin.mjs';
import { fetchBundle, MARKER, pinnedBundle } from '../lib/fetch_binding_layer.mjs';

const COMMIT = 'a'.repeat(40);
const NEXT = 'b'.repeat(40);
const ANDROID = `sha256:${'1'.repeat(64)}`;
const IOS = `sha256:${'2'.repeat(64)}`;
const LAYER = `sha256:${'3'.repeat(64)}`;
const FILE = 'binding-layer.aar';

const pin = { kind: 'pin', commit: COMMIT, digests: { android: ANDROID } };

function bundle(digest, revision) {
  return {
    kind: 'bundle',
    digest,
    revision,
    descriptor: 'zl_3.0.0_aaaaa',
    layers: [{ digest: LAYER, file: FILE }],
  };
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
  const written = { commit: COMMIT, digests: { ios: IOS, android: ANDROID } };
  assert.deepEqual(parsePin(serializePin(written)), { kind: 'pin', ...written });
});

test('serializePin writes the commit first and the platforms in order', () => {
  assert.equal(
    serializePin({ commit: COMMIT, digests: { ios: IOS, android: ANDROID } }),
    `{\n  "commit": "${COMMIT}",\n  "digests": {\n    "android": "${ANDROID}",\n    "ios": "${IOS}"\n  }\n}\n`,
  );
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
  assert.deepEqual(moved.pin, { commit: NEXT, digests: { android: IOS, ios: ANDROID } });
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
