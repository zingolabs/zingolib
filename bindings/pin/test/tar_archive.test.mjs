import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, readdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { promisify } from 'node:util';
import { gzipSync } from 'node:zlib';
import { BLOCK, untar } from '../lib/tar_archive.mjs';

const LONG_DIR = 'repository/org/zingolabs/zingo-binding-layer/6.0.0-ba7336710419c9ac';
const LONG_FILE = `${LONG_DIR}/zingo-binding-layer-6.0.0-ba7336710419c9ac.aar`;
const CONTENT = 'aar bytes';
const POSIX_MAGIC = 'ustar\0';
const GNU_MAGIC = 'ustar ';
const LONG_LINK = '././@LongLink';

async function scratch() {
  return mkdtemp(join(tmpdir(), 'bundle-pin-tar-'));
}

async function packed(format) {
  const root = await scratch();
  await mkdir(join(root, LONG_DIR), { recursive: true });
  await writeFile(join(root, LONG_FILE), CONTENT);
  const tarball = join(root, 'packed.tar.gz');
  await promisify(execFile)('tar', [
    '--create',
    '--gzip',
    `--format=${format}`,
    '--file',
    tarball,
    '-C',
    root,
    'repository',
  ]);
  return tarball;
}

function header(path, type, size, { prefix = '', magic = POSIX_MAGIC } = {}) {
  const block = Buffer.alloc(BLOCK);
  block.write(path, 0, 100, 'latin1');
  block.write('0000644\0', 100, 'latin1');
  block.write('0000000\0', 108, 'latin1');
  block.write('0000000\0', 116, 'latin1');
  block.write(`${size.toString(8).padStart(11, '0')}\0`, 124, 'latin1');
  block.write('00000000000\0', 136, 'latin1');
  block.write('        ', 148, 'latin1');
  block.write(type, 156, 'latin1');
  block.write(magic, 257, 'latin1');
  block.write('00', 263, 'latin1');
  block.write(prefix, 345, 155, 'latin1');
  const sum = block.reduce((total, byte) => total + byte, 0);
  block.write(`${sum.toString(8).padStart(6, '0')}\0 `, 148, 'latin1');
  return block;
}

function entry(path, type, content = '', options) {
  const body = Buffer.from(content);
  const pad = Buffer.alloc((BLOCK - (body.length % BLOCK)) % BLOCK);
  return Buffer.concat([header(path, type, body.length, options), body, pad]);
}

function record(key, value) {
  const bare = ` ${key}=${value}\n`;
  let length = bare.length + 1;
  while (`${length}`.length + bare.length !== length) {
    length += 1;
  }
  return `${length}${bare}`;
}

async function handmade(...entries) {
  const root = await scratch();
  const file = join(root, 'hand.tar.gz');
  await writeFile(file, gzipSync(Buffer.concat([...entries, Buffer.alloc(BLOCK * 2)])));
  return file;
}

async function extracted(archive) {
  const into = join(await scratch(), 'into');
  await mkdir(into);
  return { into, outcome: await untar(archive, into) };
}

test('untar extracts a pax archive whose paths exceed the ustar name field', async () => {
  const { into, outcome } = await extracted(await packed('pax'));
  assert.deepEqual(outcome, { kind: 'extracted' });
  assert.equal(await readFile(join(into, LONG_FILE), 'utf8'), CONTENT);
});

test('untar extracts a ustar archive through its prefix split', async () => {
  const { into, outcome } = await extracted(await packed('ustar'));
  assert.deepEqual(outcome, { kind: 'extracted' });
  assert.equal(await readFile(join(into, LONG_FILE), 'utf8'), CONTENT);
});

test('untar takes the path from an extended header', async () => {
  const records = record('path', LONG_FILE);
  const { into, outcome } = await extracted(
    await handmade(
      entry('PaxHeaders/short', 'x', records),
      entry('short', '0', CONTENT),
      entry('ignored', 'g', record('comment', 'global')),
    ),
  );
  assert.deepEqual(outcome, { kind: 'extracted' });
  assert.equal(await readFile(join(into, LONG_FILE), 'utf8'), CONTENT);
});

test('untar refuses the long-name entry of the GNU format', async () => {
  const { into, outcome } = await extracted(
    await handmade(
      entry(LONG_LINK, 'L', `${LONG_FILE}\0`, { magic: GNU_MAGIC }),
      entry(LONG_FILE.slice(0, 100), '0', CONTENT, { magic: GNU_MAGIC }),
    ),
  );
  assert.deepEqual(outcome, { kind: 'refusedEntry', path: LONG_LINK, type: 'L' });
  assert.deepEqual(await readdir(into), []);
});

test('untar refuses symbolic and hard links', async () => {
  for (const type of ['2', '1']) {
    const { outcome } = await extracted(await handmade(entry('link', type)));
    assert.deepEqual(outcome, { kind: 'refusedEntry', path: 'link', type });
  }
});

test('untar refuses a path that leaves the root', async () => {
  for (const path of ['../escape', '/absolute', 'inside/../../escape']) {
    const { outcome } = await extracted(await handmade(entry(path, '0', CONTENT)));
    assert.deepEqual(outcome, { kind: 'escapedPath', path });
  }
});

test('untar refuses a header without the ustar magic', async () => {
  const { outcome } = await extracted(
    await handmade(entry('file', '0', CONTENT, { magic: '      ' })),
  );
  assert.equal(outcome.kind, 'badArchive');
});

test('untar refuses an archive that ends inside an entry', async () => {
  const root = await scratch();
  const file = join(root, 'short.tar.gz');
  await writeFile(file, gzipSync(Buffer.concat([header('file', '0', BLOCK * 4), Buffer.alloc(10)])));
  const { outcome } = await extracted(file);
  assert.equal(outcome.kind, 'badArchive');
});

test('untar refuses a file that is not gzip', async () => {
  const root = await scratch();
  const file = join(root, 'plain.tar.gz');
  await writeFile(file, 'not gzip');
  const { outcome } = await extracted(file);
  assert.equal(outcome.kind, 'badArchive');
});
