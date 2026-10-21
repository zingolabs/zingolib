import { rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { exists, replaceDirectory, text } from './disk.mjs';
import { untar } from './tar_archive.mjs';

export const PACKAGE = 'ZingoBindings';
export const MARKER = 'digest';
const MANIFEST = 'Package.swift';

export function placement(marker, digest, manifestPresent) {
  return marker === digest && manifestPresent ? 'held' : 'extract';
}

export async function placeSwiftPackage(archive, digest, home, extract = untar) {
  const marker = join(home, MARKER);
  const manifestPresent = await exists(join(home, PACKAGE, MANIFEST));
  if (placement(await text(marker), digest, manifestPresent) === 'held') {
    return { kind: 'placed', path: join(home, PACKAGE) };
  }
  await rm(marker, { force: true });
  const replaced = await replaceDirectory(
    join(home, PACKAGE),
    async partial => {
      const extracted = await extract(archive, partial);
      if (extracted.kind !== 'extracted') {
        return { ...extracted, platform: 'ios' };
      }
      if (!(await exists(join(partial, PACKAGE, MANIFEST)))) {
        return { kind: 'badPackage', platform: 'ios' };
      }
      return { kind: 'filled' };
    },
    PACKAGE,
  );
  if (replaced.kind !== 'replaced') {
    return replaced;
  }
  await writeFile(marker, `${digest}\n`);
  return { kind: 'placed', path: join(home, PACKAGE) };
}
