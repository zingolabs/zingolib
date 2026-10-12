import { execFile } from 'node:child_process';
import { mkdir, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { promisify } from 'node:util';

export const PACKAGE = 'ZingoBindings';
export const MARKER = 'digest';
const MANIFEST = 'Package.swift';

export function placement(marker, digest, manifestPresent) {
  return marker === digest && manifestPresent ? 'held' : 'extract';
}

async function text(path) {
  return readFile(path, 'utf8')
    .then(content => content.trim())
    .catch(failure => {
      if (failure.code === 'ENOENT') {
        return undefined;
      }
      throw failure;
    });
}

async function exists(path) {
  return stat(path)
    .then(() => true)
    .catch(failure => {
      if (failure.code === 'ENOENT') {
        return false;
      }
      throw failure;
    });
}

export async function untar(archive, into) {
  await promisify(execFile)('tar', ['-xzf', archive, '-C', into]);
}

export async function placeSwiftPackage(archive, digest, home, extract = untar) {
  const manifest = join(home, PACKAGE, MANIFEST);
  const marker = await text(join(home, MARKER));
  if (placement(marker, digest, await exists(manifest)) === 'held') {
    return { kind: 'placed', path: join(home, PACKAGE) };
  }
  const partial = `${home}.partial`;
  await rm(partial, { recursive: true, force: true });
  await mkdir(partial, { recursive: true });
  await extract(archive, partial);
  if (!(await exists(join(partial, PACKAGE, MANIFEST)))) {
    await rm(partial, { recursive: true, force: true });
    return { kind: 'badPackage', platform: 'ios' };
  }
  await writeFile(join(partial, MARKER), `${digest}\n`);
  await rm(home, { recursive: true, force: true });
  await rename(partial, home);
  return { kind: 'placed', path: join(home, PACKAGE) };
}
