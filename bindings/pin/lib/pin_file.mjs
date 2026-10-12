// The Bundle Pin's file: binding-layer.json.

import { readFile, rename, writeFile } from 'node:fs/promises';
import { parsePin, serializePin } from './bundle_pin.mjs';

export async function readPin(path) {
  const text = await readFile(path, 'utf8').catch(failure => {
    if (failure.code === 'ENOENT') {
      return undefined;
    }
    throw failure;
  });
  return text === undefined ? { kind: 'unpinned' } : parsePin(text);
}

export async function writePin(path, pin) {
  const partial = `${path}.partial`;
  await writeFile(partial, serializePin(pin));
  await rename(partial, path);
}
