#!/usr/bin/env node
import { resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { failureMessage } from '../lib/bundle_messages.mjs';
import { movePin, repository } from '../lib/bundle_pin.mjs';
import { pinPath, readPin, writePin } from '../lib/pin_file.mjs';
import { resolveTag } from '../lib/registry.mjs';

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    platform: { type: 'string', multiple: true, default: [] },
    root: { type: 'string', default: process.cwd() },
    pin: { type: 'string' },
  },
});
const path = pinPath(resolve(values.root), values.pin);
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
if (outcome.kind !== 'moved') {
  process.stderr.write(`${failureMessage(outcome)}\n`);
  process.exitCode = 1;
} else {
  await writePin(path, outcome.pin);
  for (const [platform, digest] of Object.entries(outcome.pin.digests)) {
    process.stdout.write(
      `${platform} ${outcome.descriptors[platform]} ${digest}\n`,
    );
  }
  if (outcome.pin.jna !== undefined) {
    process.stdout.write(`jna ${outcome.pin.jna}\n`);
  }
}
