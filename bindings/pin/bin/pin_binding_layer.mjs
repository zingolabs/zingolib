#!/usr/bin/env node
import { resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { failureMessage } from '../lib/bundle_messages.mjs';
import { movePin, repository } from '../lib/bundle_pin.mjs';
import { coordinates, storePath } from '../lib/fetch_binding_layer.mjs';
import { pinPath, readPin, writePin } from '../lib/pin_file.mjs';
import { access, download, resolveBundle, resolveTag } from '../lib/registry.mjs';

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    platform: { type: 'string', multiple: true, default: [] },
    root: { type: 'string', default: process.cwd() },
    pin: { type: 'string' },
    store: { type: 'string' },
  },
});
const root = resolve(values.root);
const path = pinPath(root, values.pin);
const current = await readPin(path);
const outcome =
  current.kind === 'malformedPin'
    ? current
    : await movePin(
        current,
        positionals[0] ?? '',
        values.platform,
        (platform, commit) => resolveTag(repository(platform), commit),
      );
const described =
  outcome.kind === 'moved'
    ? await coordinates(
        outcome.pin,
        platform => storePath(root, platform, values.store),
        { access, resolveBundle, download },
      )
    : outcome;
if (described.kind !== 'described') {
  process.stderr.write(`${failureMessage(described)}\n`);
  process.exitCode = 1;
} else {
  const pin = { ...outcome.pin, coordinates: described.coordinates };
  await writePin(path, pin);
  for (const [platform, digest] of Object.entries(pin.digests)) {
    const coordinate = pin.coordinates[platform];
    process.stdout.write(
      `${platform} ${outcome.descriptors[platform]} ${digest}${coordinate ? ` ${coordinate}` : ''}\n`,
    );
  }
}
