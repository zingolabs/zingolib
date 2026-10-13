import { rename, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { parsePin, serializePin } from './bundle_pin.mjs';
import { text } from './disk.mjs';

export function pinPath(root, pin) {
  return pin ?? join(root, 'binding-layer.json');
}

export async function readPin(path) {
  const content = await text(path);
  return content === undefined ? { kind: 'unpinned' } : parsePin(content);
}

export async function writePin(path, pin) {
  const partial = `${path}.partial`;
  await writeFile(partial, serializePin(pin));
  await rename(partial, path);
}
