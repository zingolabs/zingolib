import {
  MAVEN_PLATFORMS,
  PLATFORMS,
  reference,
  REGISTRY,
  repository,
} from './bundle_pin.mjs';
import { COMMIT_CHARACTERS } from './maven_repository.mjs';

const REPORT = 'Report this to zingolib.';

export function failureMessage(failure) {
  switch (failure.kind) {
    case 'badCommit':
      return `"${failure.commit}" is not a full zingolib commit. Give all 40 hexadecimal characters.`;
    case 'noPlatform':
      return 'The Bundle Pin lists no platform. Add one with --platform android.';
    case 'unnamedPlatform':
      return `Name the platform with --platform ${PLATFORMS.join(' or --platform ')}.`;
    case 'unknownPlatform':
      return `"${
        failure.platform
      }" is not a platform. The platforms are ${PLATFORMS.join(' and ')}.`;
    case 'absent':
      return (
        `zingolib has published no ${failure.platform} Prebuilt Bundle for commit ${failure.commit}. ` +
        `The registry holds no ${reference(
          failure.platform,
          failure.commit,
        )}. ` +
        'Request the publish in zingolib, or pin a published commit with "pin-binding-layer <commit>".'
      );
    case 'refused':
      return (
        `The registry refuses anonymous access to ${REGISTRY}/${repository(
          failure.platform,
        )}. ` +
        `zingolib has published no ${failure.platform} Prebuilt Bundle, or its package is private.`
      );
    case 'wrongRevision':
      return (
        `The ${failure.platform} bundle tagged ${failure.commit} reports the commit ${failure.revision}. ` +
        REPORT
      );
    case 'movedTag':
      return (
        `The tag ${reference(
          failure.platform,
          failure.commit,
        )} now resolves to ${failure.found}, ` +
        `and the Bundle Pin holds ${failure.pinned}. A published tag never moves. ${REPORT}`
      );
    case 'malformedPin':
      return `binding-layer.json holds no valid ${failure.field}. Write the file with "pin-binding-layer <commit>".`;
    case 'oldPin':
      return (
        `binding-layer.json holds no coordinate for ${MAVEN_PLATFORMS.filter(
          platform => platform in failure.digests && !(platform in failure.coordinates),
        ).join(' and ')}, which an earlier zingolib-bundle-pin did not write. ` +
        `Rewrite the file with "pin-binding-layer ${failure.commit}".`
      );
    case 'unpinned':
      return 'This checkout has no binding-layer.json. Create it with "pin-binding-layer <commit> --platform android".';
    case 'unpinnedPlatform':
      return (
        `The Bundle Pin holds no ${failure.platform} digest. ` +
        `Add it with "pin-binding-layer <commit> --platform ${failure.platform}".`
      );
    case 'badLayout':
      return `The ${failure.platform} bundle is not one named file in one layer. ${REPORT}`;
    case 'corrupt':
      return `The downloaded content hashes to ${failure.found}, and the bundle's layer is ${failure.expected}.`;
    case 'badPackage':
      return `The ${failure.platform} bundle holds no ZingoBindings/Package.swift. ${REPORT}`;
    case 'oldShape':
      return (
        `The ${failure.platform} bundle tagged ${failure.commit} is not a gzip archive. ` +
        'zingolib published it before the Maven repository layout. Pin a commit published later.'
      );
    case 'unannotated':
      return (
        `The ${failure.platform} bundle tagged ${failure.commit} names no Maven coordinate in its manifest. ` +
        'zingolib published it before it annotated the coordinate. Pin a commit published later.'
      );
    case 'badCoordinate':
      return (
        `The ${failure.platform} bundle tagged ${failure.commit} names the coordinate "${failure.coordinate}" ` +
        `in its manifest, which is not group, artifact and version. ${REPORT}`
      );
    case 'badRepository':
      return `The ${failure.platform} bundle holds no Maven repository with one artifact at one version. ${REPORT}`;
    case 'wrongCoordinate':
      return (
        `The ${failure.platform} bundle tagged ${failure.commit} holds the coordinate ${failure.found}, ` +
        `and the Bundle Pin holds ${failure.pinned}. A pinned bundle never changes. ` +
        `Rewrite the pin with "pin-binding-layer ${failure.commit}".`
      );
    case 'wrongVersion':
      return (
        `The ${failure.platform} bundle tagged ${failure.commit} is published as version ${failure.version}, ` +
        `which does not end with the first ${COMMIT_CHARACTERS} characters of the commit. ${REPORT}`
      );
  }
}
