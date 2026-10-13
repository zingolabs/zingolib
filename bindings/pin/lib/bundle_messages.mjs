import { PLATFORMS, reference, REGISTRY, repository } from './bundle_pin.mjs';

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
  }
}
