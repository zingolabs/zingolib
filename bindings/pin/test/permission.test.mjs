import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { cp, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { promisify } from 'node:util';
import { MARKER as BUNDLE } from '../lib/fetch_binding_layer.mjs';
import { serializePin } from '../lib/bundle_pin.mjs';
import { MARKER as DIGEST, PACKAGE } from '../lib/swift_package.mjs';
import { archive, COMMIT, COORDINATE } from './maven_fixture.mjs';

const PACKAGE_DIR = dirname(dirname(fileURLToPath(import.meta.url)));
const README = join(PACKAGE_DIR, 'README.md');
const INSTALLED = join('node_modules', 'zingolib-bundle-pin');
const ANDROID = `sha256:${'1'.repeat(64)}`;
const IOS = `sha256:${'2'.repeat(64)}`;
const SHIPPED = ['bin', 'lib', 'package.json'];
const SCRIPTS_BLOCK = /```json\n(\{[^`]*"fetch:binding-layer"[^`]*\})\n```/;
const DENIED = 'ERR_ACCESS_DENIED';
const IOS_ROOT_WRITE = '--allow-fs-write=ios/binding-layer';
const PIN_COMMAND = 'node_modules/zingolib-bundle-pin/bin/pin_binding_layer.mjs';

async function scripts() {
  const found = (await readFile(README, 'utf8')).match(SCRIPTS_BLOCK);
  assert.ok(found, 'README.md shows the consumer scripts in one json block');
  return JSON.parse(found[1]).scripts;
}

async function run(line, extra, cwd) {
  try {
    const { stdout, stderr } = await promisify(execFile)(
      'sh',
      ['-c', [line, ...extra].join(' ')],
      { cwd, env: { ...process.env, PATH: `${dirname(process.execPath)}:${process.env.PATH}` } },
    );
    return { code: 0, stdout, stderr };
  } catch (failure) {
    return { code: failure.code, stdout: failure.stdout, stderr: failure.stderr };
  }
}

async function swiftArchive(root) {
  await mkdir(join(root, PACKAGE), { recursive: true });
  await writeFile(join(root, PACKAGE, 'Package.swift'), '// swift-tools-version:5.9\n');
  const tarball = join(root, 'ZingoBindings.tar.gz');
  await promisify(execFile)('tar', [
    '--create',
    '--gzip',
    '--format=pax',
    '--file',
    tarball,
    '-C',
    root,
    PACKAGE,
  ]);
  return tarball;
}

async function store(root, platform, digest, source) {
  const home = join(root, platform, 'build', 'binding-layer', digest.slice('sha256:'.length));
  await mkdir(home, { recursive: true });
  const file = source.split('/').at(-1);
  await cp(source, join(home, file));
  await writeFile(join(home, BUNDLE), `${file}\n`);
}

async function consumer() {
  const root = await mkdtemp(join(tmpdir(), 'bundle-pin-consumer-'));
  for (const name of SHIPPED) {
    await cp(join(PACKAGE_DIR, name), join(root, INSTALLED, name), { recursive: true });
  }
  await writeFile(
    join(root, 'binding-layer.json'),
    serializePin({
      commit: COMMIT,
      digests: { android: ANDROID, ios: IOS },
      coordinates: { android: COORDINATE },
    }),
  );
  await store(root, 'android', ANDROID, await archive(['6.0.0-ba7336710419c9ac']));
  await store(root, 'ios', IOS, await swiftArchive(await mkdtemp(join(tmpdir(), 'bundle-pin-swift-'))));
  return root;
}

test('the README fetch script suffices for an Android fetch that extracts', async () => {
  const root = await consumer();
  const { code, stdout, stderr } = await run(
    (await scripts())['fetch:binding-layer'],
    ['--platform', 'android'],
    root,
  );
  assert.equal(code, 0, stderr);
  const [path, coordinate] = stdout.trim().split('\n');
  assert.equal(coordinate, COORDINATE);
  assert.match(await readFile(join(path, 'org/zingolabs/zingo-binding-layer/maven-metadata.xml'), 'utf8'), /<groupId>/);
});

test('the README fetch script suffices for an iOS fetch that extracts through the partial path', async () => {
  const root = await consumer();
  const { code, stdout, stderr } = await run(
    (await scripts())['fetch:binding-layer'],
    ['--platform', 'ios'],
    root,
  );
  assert.equal(code, 0, stderr);
  assert.equal(stdout, `${join(root, 'ios', 'binding-layer', PACKAGE)}\n`);
  assert.equal(await readFile(join(root, 'ios', 'binding-layer', DIGEST), 'utf8'), `${IOS}\n`);
});

test('the bound refuses the iOS fetch when the extraction root is not opened for writing', async () => {
  const root = await consumer();
  const line = (await scripts())['fetch:binding-layer'];
  assert.ok(line.includes(` ${IOS_ROOT_WRITE} `), 'the README opens the iOS root');
  const { code, stderr } = await run(
    line.replace(` ${IOS_ROOT_WRITE} `, ' '),
    ['--platform', 'ios'],
    root,
  );
  assert.notEqual(code, 0);
  assert.ok(stderr.includes(DENIED), stderr);
});

test('the README pin script loads the command under the bound', async () => {
  const root = await consumer();
  const { code, stderr } = await run((await scripts()).pin, ['not-a-commit'], root);
  assert.equal(code, 1);
  assert.ok(stderr.includes('not a full zingolib commit'), stderr);
  assert.ok(!stderr.includes(DENIED), stderr);
});

test('the README pin script opens what writePin needs', async () => {
  const root = await consumer();
  const line = (await scripts()).pin;
  assert.ok(line.endsWith(` ${PIN_COMMAND}`), 'the pin line ends with the command');
  const probe = [
    "import { writePin } from './node_modules/zingolib-bundle-pin/lib/pin_file.mjs';",
    `await writePin('binding-layer.json', { commit: '${'c'.repeat(40)}', digests: { ios: '${IOS}' }, coordinates: {} });`,
  ].join(' ');
  const { code, stderr } = await run(
    line.slice(0, -PIN_COMMAND.length),
    ['--input-type=module', '--eval', `"${probe}"`],
    root,
  );
  assert.equal(code, 0, stderr);
  assert.match(await readFile(join(root, 'binding-layer.json'), 'utf8'), /"ios"/);
});
