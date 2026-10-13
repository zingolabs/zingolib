import { execFile } from 'node:child_process';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { exists, replaceDirectory, text } from './disk.mjs';

export const PACKAGE = 'ZingoBindings';
export const MARKER = 'digest';
const MANIFEST = 'Package.swift';

export function placement(marker, digest, manifestPresent) {
  return marker === digest && manifestPresent ? 'held' : 'extract';
}

export async function untar(archive, into) {
  await promisify(execFile)('tar', ['-xzf', archive, '-C', into]);
}

export async function placeSwiftPackage(archive, digest, home, extract = untar) {
  const marker = await text(join(home, MARKER));
  const manifestPresent = await exists(join(home, PACKAGE, MANIFEST));
  if (placement(marker, digest, manifestPresent) === 'held') {
    return { kind: 'placed', path: join(home, PACKAGE) };
  }
  const replaced = await replaceDirectory(home, async partial => {
    await extract(archive, partial);
    if (!(await exists(join(partial, PACKAGE, MANIFEST)))) {
      return { kind: 'badPackage', platform: 'ios' };
    }
    await writeFile(join(partial, MARKER), `${digest}\n`);
    return { kind: 'filled' };
  });
  return replaced.kind === 'replaced'
    ? { kind: 'placed', path: join(home, PACKAGE) }
    : replaced;
}
