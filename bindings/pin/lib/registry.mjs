// The registry requests of the Bundle Pin scripts: a token, a manifest, and a
// layer's content.

import { createHash } from 'node:crypto';
import { createWriteStream } from 'node:fs';
import { rm } from 'node:fs/promises';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { REGISTRY } from './bundle_pin.mjs';

const MANIFEST = 'application/vnd.oci.image.manifest.v1+json';
const REVISION = 'org.opencontainers.image.revision';
const DESCRIPTOR = 'org.zingolabs.zingolib.descriptor';
const TITLE = 'org.opencontainers.image.title';
const REFUSALS = [401, 403];
const NOT_FOUND = [404];

function sha256(hash) {
  return `sha256:${hash.digest('hex')}`;
}

async function request(url, headers, tolerated) {
  const reply = await fetch(url, { headers });
  if (!reply.ok && !tolerated.includes(reply.status)) {
    throw new Error(`${url} answered ${reply.status}`);
  }
  return reply;
}

async function access(repository) {
  const reply = await request(
    `https://${REGISTRY}/token?service=${REGISTRY}&scope=repository:${repository}:pull`,
    {},
    REFUSALS,
  );
  if (!reply.ok) {
    return { kind: 'refused' };
  }
  const { token } = await reply.json();
  return { kind: 'granted', headers: { Authorization: `Bearer ${token}` } };
}

export async function resolveBundle(repository, tag) {
  const granted = await access(repository);
  if (granted.kind === 'refused') {
    return granted;
  }
  const reply = await request(
    `https://${REGISTRY}/v2/${repository}/manifests/${tag}`,
    { ...granted.headers, Accept: MANIFEST },
    NOT_FOUND,
  );
  if (!reply.ok) {
    return { kind: 'absent' };
  }
  const body = Buffer.from(await reply.arrayBuffer());
  const { annotations = {}, layers } = JSON.parse(body);
  return {
    kind: 'bundle',
    digest: sha256(createHash('sha256').update(body)),
    revision: annotations[REVISION],
    descriptor: annotations[DESCRIPTOR],
    layers: layers.map(layer => ({
      digest: layer.digest,
      file: layer.annotations?.[TITLE],
    })),
  };
}

export async function download(repository, digest, path) {
  const granted = await access(repository);
  if (granted.kind === 'refused') {
    throw new Error(`${REGISTRY} refused access to ${repository}`);
  }
  const reply = await request(
    `https://${REGISTRY}/v2/${repository}/blobs/${digest}`,
    granted.headers,
    [],
  );
  const hash = createHash('sha256');
  await pipeline(
    Readable.fromWeb(reply.body),
    async function* (chunks) {
      for await (const chunk of chunks) {
        hash.update(chunk);
        yield chunk;
      }
    },
    createWriteStream(path),
  );
  const found = sha256(hash);
  if (found !== digest) {
    await rm(path);
    return { kind: 'corrupt', expected: digest, found };
  }
  return { kind: 'downloaded', path };
}
