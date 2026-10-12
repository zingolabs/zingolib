#!/usr/bin/env node
// Moves the Bundle Pin to one zingolib commit. It asks the registry for the
// digest of each pinned platform's bundle and rewrites binding-layer.json,
// for every platform or for none.
//
// Usage, from the consumer's root: pin-binding-layer <commit> [--platform <platform>]

import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { failureMessage } from '../lib/bundle_messages.mjs';
import { movePin, repository } from '../lib/bundle_pin.mjs';
import { readPin, writePin } from '../lib/pin_file.mjs';
import { resolveBundle } from '../lib/registry.mjs';

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    platform: { type: 'string', multiple: true, default: [] },
    root: { type: 'string', default: process.cwd() },
    pin: { type: 'string' },
  },
});
const pinPath = values.pin ?? join(resolve(values.root), 'binding-layer.json');
const current = await readPin(pinPath);
const outcome =
  current.kind === 'malformedPin'
    ? current
    : await movePin(
        current,
        positionals[0] ?? '',
        values.platform,
        (platform, commit) => resolveBundle(repository(platform), commit),
      );
if (outcome.kind !== 'moved') {
  process.stderr.write(`${failureMessage(outcome)}\n`);
  process.exitCode = 1;
} else {
  await writePin(pinPath, outcome.pin);
  for (const [platform, digest] of Object.entries(outcome.pin.digests)) {
    process.stdout.write(
      `${platform} ${outcome.descriptors[platform]} ${digest}\n`,
    );
  }
}
