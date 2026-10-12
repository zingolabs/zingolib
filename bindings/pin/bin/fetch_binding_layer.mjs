#!/usr/bin/env node
// Prints the path of the pinned Prebuilt Bundle for one platform, downloading
// it first when the checkout does not hold it. For iOS it prints the path of
// the ZingoBindings package. With --check-tag it also verifies that the
// registry's tag for the pinned commit resolves to the pinned digest.
//
// Usage, from the consumer's root: fetch-binding-layer --platform android [--check-tag]

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
