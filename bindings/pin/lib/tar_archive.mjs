import { createReadStream, createWriteStream } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import { dirname, isAbsolute, join, normalize, relative } from 'node:path';
import { pipeline } from 'node:stream/promises';
import { createGunzip } from 'node:zlib';

export const BLOCK = 512;
const NAME = [0, 100];
const MODE = [100, 8];
const SIZE = [124, 12];
const TYPE = [156, 1];
const MAGIC = [257, 6];
const PREFIX = [345, 155];
const USTAR = 'ustar';
const REGULAR = ['0', '\0'];
const DIRECTORY = '5';
const EXTENDED = 'x';
const GLOBAL = 'g';
const PATH_RECORD = 'path';
const SIZE_RECORD = 'size';
const MODE_MASK = 0o777;
const SPACE = 0x20;
const EQUALS = '=';

class Truncated extends Error {
  constructor() {
    super('the archive ends inside an entry');
  }
}

function field(header, [start, length]) {
  const raw = header.subarray(start, start + length).toString('latin1');
  const end = raw.indexOf('\0');
  return end === -1 ? raw : raw.slice(0, end);
}

function octal(header, where) {
  return parseInt(field(header, where).trim() || '0', 8);
}

function parseRecords(body) {
  const records = {};
  let at = 0;
  while (at < body.length) {
    const space = body.indexOf(SPACE, at);
    const length = space === -1 ? NaN : parseInt(body.subarray(at, space).toString('latin1'), 10);
    if (!(length > 0) || at + length > body.length) {
      return undefined;
    }
    const record = body.subarray(space + 1, at + length - 1).toString('utf8');
    const equals = record.indexOf(EQUALS);
    records[record.slice(0, equals)] = record.slice(equals + 1);
    at += length;
  }
  return records;
}

function entryPath(header, records) {
  if (records[PATH_RECORD] !== undefined) {
    return records[PATH_RECORD];
  }
  const name = field(header, NAME);
  const prefix = field(header, PREFIX);
  return prefix ? `${prefix}/${name}` : name;
}

function placeOf(into, path) {
  const trimmed = path.replace(/\/+$/, '');
  if (trimmed === '' || isAbsolute(trimmed) || trimmed.startsWith('/')) {
    return undefined;
  }
  const target = join(into, normalize(trimmed));
  const inside = relative(into, target);
  if (inside.startsWith('..') || isAbsolute(inside)) {
    return undefined;
  }
  return { target, root: inside === '' };
}

function chunks(stream) {
  const iterator = stream[Symbol.asyncIterator]();
  let head = Buffer.alloc(0);
  async function more() {
    const { value, done } = await iterator.next();
    if (done) {
      return false;
    }
    head = head.length === 0 ? value : Buffer.concat([head, value]);
    return true;
  }
  return {
    async take(length) {
      while (head.length < length) {
        if (!(await more())) {
          return undefined;
        }
      }
      const taken = head.subarray(0, length);
      head = head.subarray(length);
      return taken;
    },
    async *pieces(length) {
      let left = length;
      while (left > 0) {
        if (head.length === 0 && !(await more())) {
          throw new Truncated();
        }
        const piece = head.subarray(0, Math.min(left, head.length));
        head = head.subarray(piece.length);
        left -= piece.length;
        yield piece;
      }
    },
  };
}

function padded(size) {
  return Math.ceil(size / BLOCK) * BLOCK;
}

async function extractEntries(blocks, into) {
  let records = {};
  for (;;) {
    const header = await blocks.take(BLOCK);
    if (header === undefined || header.every(byte => byte === 0)) {
      return { kind: 'extracted' };
    }
    if (field(header, MAGIC).trim() !== USTAR) {
      return { kind: 'badArchive', reason: 'a header carries no ustar magic' };
    }
    const type = field(header, TYPE) || '\0';
    const size =
      records[SIZE_RECORD] !== undefined ? Number(records[SIZE_RECORD]) : octal(header, SIZE);
    if (type === EXTENDED || type === GLOBAL) {
      const body = await blocks.take(padded(size));
      if (body === undefined) {
        throw new Truncated();
      }
      if (type === EXTENDED) {
        records = parseRecords(body.subarray(0, size));
        if (records === undefined) {
          return { kind: 'badArchive', reason: 'an extended header is malformed' };
        }
      }
      continue;
    }
    const path = entryPath(header, records);
    records = {};
    const place = placeOf(into, path);
    if (place === undefined || (place.root && type !== DIRECTORY)) {
      return { kind: 'escapedPath', path };
    }
    if (type === DIRECTORY) {
      await mkdir(place.target, { recursive: true });
    } else if (REGULAR.includes(type)) {
      await mkdir(dirname(place.target), { recursive: true });
      await pipeline(
        blocks.pieces(size),
        createWriteStream(place.target, { mode: octal(header, MODE) & MODE_MASK }),
      );
    } else {
      return { kind: 'refusedEntry', path, type };
    }
    const written = REGULAR.includes(type) ? size : 0;
    if (padded(size) > written && (await blocks.take(padded(size) - written)) === undefined) {
      throw new Truncated();
    }
  }
}

export async function untar(archive, into) {
  const source = createReadStream(archive).pipe(createGunzip());
  try {
    return await extractEntries(chunks(source), into);
  } catch (failure) {
    if (failure instanceof Truncated || String(failure.code).startsWith('Z_')) {
      return { kind: 'badArchive', reason: failure.message };
    }
    throw failure;
  } finally {
    source.destroy();
  }
}
