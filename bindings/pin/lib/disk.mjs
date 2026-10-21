import { mkdir, readFile, rename, rm, stat } from 'node:fs/promises';
import { join } from 'node:path';

function orAbsent(promise, absent) {
  return promise.catch(failure => {
    if (failure.code === 'ENOENT') {
      return absent;
    }
    throw failure;
  });
}

export function text(path) {
  return orAbsent(
    readFile(path, 'utf8').then(content => content.trim()),
    undefined,
  );
}

export function exists(path) {
  return orAbsent(
    stat(path).then(() => true),
    false,
  );
}

export async function replaceDirectory(home, fill, inner) {
  const partial = `${home}.partial`;
  await rm(partial, { recursive: true, force: true });
  await mkdir(partial, { recursive: true });
  const outcome = await fill(partial);
  if (outcome.kind !== 'filled') {
    await rm(partial, { recursive: true, force: true });
    return outcome;
  }
  await rm(home, { recursive: true, force: true });
  await rename(inner === undefined ? partial : join(partial, inner), home);
  if (inner !== undefined) {
    await rm(partial, { recursive: true, force: true });
  }
  return { kind: 'replaced' };
}
