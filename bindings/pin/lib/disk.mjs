import { mkdir, readFile, rename, rm, stat } from 'node:fs/promises';

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

export async function replaceDirectory(home, fill) {
  const partial = `${home}.partial`;
  await rm(partial, { recursive: true, force: true });
  await mkdir(partial, { recursive: true });
  const outcome = await fill(partial);
  if (outcome.kind !== 'filled') {
    await rm(partial, { recursive: true, force: true });
    return outcome;
  }
  await rm(home, { recursive: true, force: true });
  await rename(partial, home);
  return { kind: 'replaced' };
}
