import { readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { exists, replaceDirectory, text } from './disk.mjs';
import { untar } from './swift_package.mjs';

export const REPOSITORY = 'repository';
export const METADATA = 'maven-metadata.xml';
export const COMMIT_CHARACTERS = 16;
const VERSION_SEPARATOR = '-';
const COORDINATE_SEPARATOR = ':';

function element(xml, name) {
  const found = xml.match(new RegExp(`<${name}>([^<]*)</${name}>`));
  return found?.[1].trim();
}

export function coordinateIn(xml) {
  const groupId = element(xml, 'groupId');
  const artifactId = element(xml, 'artifactId');
  const versions = [...xml.matchAll(/<version>([^<]*)<\/version>/g)].map(
    found => found[1].trim(),
  );
  if (!groupId || !artifactId || versions.length !== 1 || !versions[0]) {
    return undefined;
  }
  const [version] = versions;
  return {
    version,
    coordinate: [groupId, artifactId, version].join(COORDINATE_SEPARATOR),
  };
}

export function versionNamesCommit(version, commit) {
  return version.endsWith(
    `${VERSION_SEPARATOR}${commit.slice(0, COMMIT_CHARACTERS)}`,
  );
}

async function metadataFiles(dir) {
  const entries = await readdir(dir, { withFileTypes: true, recursive: true });
  return entries
    .filter(entry => entry.isFile() && entry.name === METADATA)
    .map(entry => join(entry.parentPath, entry.name));
}

export async function coordinateOf(repository) {
  const files = (await exists(repository)) ? await metadataFiles(repository) : [];
  const found = (
    await Promise.all(files.map(async file => coordinateIn((await text(file)) ?? '')))
  ).filter(Boolean);
  return found.length === 1 ? found[0] : undefined;
}

export async function placeMavenRepository(
  archive,
  home,
  refuse = () => undefined,
  extract = untar,
) {
  const repository = join(home, REPOSITORY);
  const held = await coordinateOf(repository);
  if (held) {
    return refuse(held) ?? { kind: 'placed', path: repository, ...held };
  }
  let found;
  const replaced = await replaceDirectory(home, async partial => {
    await extract(archive, partial);
    found = await coordinateOf(join(partial, REPOSITORY));
    if (!found) {
      return { kind: 'badRepository', platform: 'android' };
    }
    return refuse(found) ?? { kind: 'filled' };
  });
  return replaced.kind === 'replaced'
    ? { kind: 'placed', path: repository, ...found }
    : replaced;
}
