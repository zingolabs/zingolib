#!/usr/bin/env node
import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { failureMessage } from '../lib/bundle_messages.mjs';
import { pinnedBundle } from '../lib/fetch_binding_layer.mjs';

const { values } = parseArgs({
  options: {
    platform: { type: 'string' },
    root: { type: 'string', default: process.cwd() },
    pin: { type: 'string' },
    store: { type: 'string' },
    'check-tag': { type: 'boolean', default: false },
  },
});
const root = resolve(values.root);
const outcome = await pinnedBundle(root, {
  ...values,
  pin: values.pin ?? join(root, 'binding-layer.json'),
});
if (outcome.kind === 'fetched') {
  process.stdout.write(`${outcome.path}\n`);
} else {
  process.stderr.write(`${failureMessage(outcome)}\n`);
  process.exitCode = 1;
}
