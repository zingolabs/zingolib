import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { REPOSITORY } from '../lib/maven_repository.mjs';

export const COMMIT = 'ba7336710419c9ac1e9362ac1b4e5b0073ff477e';
export const VERSION = '6.0.0-ba7336710419c9ac';
export const COORDINATE = `org.zingolabs:zingo-binding-layer:${VERSION}`;

export function metadata(versions) {
  return [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<metadata>',
    '  <groupId>org.zingolabs</groupId>',
    '  <artifactId>zingo-binding-layer</artifactId>',
    '  <versioning>',
    `    <latest>${versions[0] ?? ''}</latest>`,
    '    <versions>',
    ...versions.map(version => `      <version>${version}</version>`),
    '    </versions>',
    '  </versioning>',
    '</metadata>',
  ].join('\n');
}

export async function scratch() {
  return mkdtemp(join(tmpdir(), 'bundle-pin-maven-'));
}

export async function archive(versions) {
  const root = await scratch();
  const artifact = join(root, REPOSITORY, 'org', 'zingolabs', 'zingo-binding-layer');
  await mkdir(join(artifact, VERSION), { recursive: true });
  if (versions !== undefined) {
    await writeFile(join(artifact, 'maven-metadata.xml'), metadata(versions));
  }
  await writeFile(join(artifact, VERSION, `zingo-binding-layer-${VERSION}.aar`), 'aar');
  const tarball = join(root, 'repository.tar.gz');
  await promisify(execFile)('tar', ['--create', '--gzip', '--format=pax', '--file', tarball, '-C', root, REPOSITORY]);
  return tarball;
}
