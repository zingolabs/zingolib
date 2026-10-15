export const REGISTRY = 'ghcr.io';
export const PLATFORMS = ['android', 'ios'];

const COMMIT = /^[0-9a-f]{40}$/;
const DIGEST = /^sha256:[0-9a-f]{64}$/;

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
  const { commit, digests } = parsed ?? {};
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
  return { kind: 'pin', commit, digests };
}

export function serializePin({ commit, digests }) {
  const sorted = Object.fromEntries(
    Object.entries(digests).sort(([left], [right]) =>
      left.localeCompare(right),
    ),
  );
  return `${JSON.stringify({ commit, digests: sorted }, null, 2)}\n`;
}

export function checkTag(platform, commit, pinned, found) {
  if (found.kind !== 'bundle') {
    return { kind: found.kind, platform, commit };
  }
  if (found.digest !== pinned) {
    return { kind: 'movedTag', platform, commit, pinned, found: found.digest };
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
  const pinned = current.kind === 'pin' && current.commit === commit;
  if (pinned && current.digests[platform]) {
    return checkTag(platform, commit, current.digests[platform], found);
  }
  return { kind: 'agrees' };
}

export async function movePin(current, commit, added, resolve) {
  if (!COMMIT.test(commit)) {
    return { kind: 'badCommit', commit };
  }
  const held = current.kind === 'pin' ? Object.keys(current.digests) : [];
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
