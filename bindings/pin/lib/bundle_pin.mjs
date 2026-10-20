export const REGISTRY = 'ghcr.io';
export const PLATFORMS = ['android', 'ios'];
// The platforms whose bundle is a Maven repository, so whose pin carries the
// coordinate Gradle declares.
export const MAVEN_PLATFORMS = ['android'];

const COMMIT = /^[0-9a-f]{40}$/;
const DIGEST = /^sha256:[0-9a-f]{64}$/;
const COORDINATE = /^[^:\s]+:[^:\s]+:[^:\s]+$/;

function sorted(entries) {
  return Object.fromEntries(
    Object.entries(entries).sort(([left], [right]) => left.localeCompare(right)),
  );
}

export function repository(platform) {
  return `zingolabs/zingolib/binding-layer-${platform}`;
}

export function reference(platform, commit) {
  return `${REGISTRY}/${repository(platform)}:${commit}`;
}

export function parsePin(text) {
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { kind: 'malformedPin', field: 'JSON' };
  }
  const { commit, digests, coordinates = {} } = parsed ?? {};
  if (!COMMIT.test(commit)) {
    return { kind: 'malformedPin', field: 'commit' };
  }
  const platforms = Object.keys(digests ?? {});
  const bad = platforms.find(
    platform =>
      !PLATFORMS.includes(platform) || !DIGEST.test(digests[platform]),
  );
  if (platforms.length === 0 || bad) {
    return { kind: 'malformedPin', field: bad ? `digests.${bad}` : 'digests' };
  }
  const unpaired = Object.keys(coordinates).find(
    platform =>
      !(MAVEN_PLATFORMS.includes(platform) && platform in digests) ||
      !COORDINATE.test(coordinates[platform]),
  );
  if (unpaired) {
    return { kind: 'malformedPin', field: `coordinates.${unpaired}` };
  }
  // A pin that an earlier version wrote names no coordinate; the pin command
  // moves it, and the fetch refuses it until the move.
  const old = MAVEN_PLATFORMS.some(
    platform => platform in digests && !(platform in coordinates),
  );
  return { kind: old ? 'oldPin' : 'pin', commit, digests, coordinates };
}

export function holdsPlatforms(current) {
  return current.kind === 'pin' || current.kind === 'oldPin';
}

export function serializePin({ commit, digests, coordinates }) {
  const pin = { commit, digests: sorted(digests) };
  if (Object.keys(coordinates).length > 0) {
    pin.coordinates = sorted(coordinates);
  }
  return `${JSON.stringify(pin, null, 2)}\n`;
}

export const ARCHIVE_MEDIA_TYPE = 'application/gzip';

export function oldShape(found) {
  return !found.layers.every(layer => layer.mediaType === ARCHIVE_MEDIA_TYPE);
}

export function checkTag(platform, commit, pinned, found) {
  if (found.kind !== 'bundle') {
    return { kind: found.kind, platform, commit };
  }
  if (found.digest !== pinned) {
    return { kind: 'movedTag', platform, commit, pinned, found: found.digest };
  }
  if (oldShape(found)) {
    return { kind: 'oldShape', platform, commit };
  }
  return { kind: 'agrees' };
}

function judge(current, platform, commit, found) {
  if (found.kind !== 'bundle') {
    return { kind: found.kind, platform, commit };
  }
  if (found.revision !== commit) {
    return {
      kind: 'wrongRevision',
      platform,
      commit,
      revision: found.revision,
    };
  }
  if (oldShape(found)) {
    return { kind: 'oldShape', platform, commit };
  }
  const pinned = holdsPlatforms(current) && current.commit === commit;
  if (pinned && current.digests[platform]) {
    return checkTag(platform, commit, current.digests[platform], found);
  }
  return { kind: 'agrees' };
}

export async function movePin(current, commit, added, resolve) {
  if (!COMMIT.test(commit)) {
    return { kind: 'badCommit', commit };
  }
  const held = holdsPlatforms(current) ? Object.keys(current.digests) : [];
  const platforms = [...new Set([...held, ...added])];
  if (platforms.length === 0) {
    return { kind: 'noPlatform' };
  }
  const unknown = platforms.find(platform => !PLATFORMS.includes(platform));
  if (unknown) {
    return { kind: 'unknownPlatform', platform: unknown };
  }
  const bundles = await Promise.all(
    platforms.map(async platform => ({
      platform,
      found: await resolve(platform, commit),
    })),
  );
  const failure = bundles
    .map(({ platform, found }) => judge(current, platform, commit, found))
    .find(judged => judged.kind !== 'agrees');
  if (failure) {
    return failure;
  }
  return {
    kind: 'moved',
    pin: {
      commit,
      digests: Object.fromEntries(
        bundles.map(({ platform, found }) => [platform, found.digest]),
      ),
    },
    descriptors: Object.fromEntries(
      bundles.map(({ platform, found }) => [platform, found.descriptor]),
    ),
  };
}
