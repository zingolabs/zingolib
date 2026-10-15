# zingolib-bundle-pin

The package that a consumer of the Binding Layer installs to hold its Bundle
Pin and to fetch the pinned Prebuilt Bundles. zingolib publishes it to npm
from this directory, so every consumer runs one definition of the scripts, and
the scripts and the bundle layout they read change in one pull request.

## Install

```sh
yarn add --dev zingolib-bundle-pin
```

The package depends on nothing beyond Node 20 and `tar`.

## The Bundle Pin

`binding-layer.json` at the consumer's root names one zingolib commit and the
digest of that commit's Prebuilt Bundle for each platform the consumer uses:

```json
{
  "commit": "<40 hexadecimal characters>",
  "digests": {
    "android": "sha256:<64 hexadecimal characters>",
    "ios": "sha256:<64 hexadecimal characters>"
  },
  "jna": "net.java.dev.jna:jna:<version>@aar"
}
```

`jna` is the Gradle coordinate of the JNA (Java Native Access) library that
the Android bundle's own build declares. The AAR carries no POM, so nothing
else tells a consumer which JNA the Kotlin Binding Set loads its native
libraries through. The publish writes the coordinate into the Android
bundle's manifest annotation `org.zingolabs.zingolib.jna`, `pin-binding-layer`
copies it into the pin whenever `android` is pinned, and the consumer's Gradle
build declares the dependency from the pin. A pin written before the
annotation existed has no `jna` key, and an Android bundle published before it
cannot be pinned.

## Commands

Both commands run from the consumer's root. `--root <dir>` names another root,
and `--pin <file>` another pin file.

`pin-binding-layer <commit> [--platform <platform>]` moves the pin. It asks
the registry for the digest of each pinned platform's bundle at that commit
and rewrites the pin for every platform or for none. The first move names the
platforms with `--platform`. It prints one line per platform with the bundle's
`zl_` descriptor and digest, and a `jna` line when `android` is pinned.

`fetch-binding-layer --platform <platform> [--check-tag]` prints the path of
the pinned bundle, downloading it first when the checkout does not hold it.
Android bundles land under `android/build/binding-layer/<digest>/`, beside a
marker file that names the bundle's file. The iOS bundle is extracted to
`ios/binding-layer/ZingoBindings`, beside a marker file that names its digest,
and the command prints that package path. With
`--check-tag` the command also verifies that the registry's tag for the pinned
commit still resolves to the pinned digest.

## Consumers

A Gradle build runs the fetch and adds the printed AAR as a file dependency,
and it declares JNA from the pin's `jna` key, so a pin move carries the JNA
version with it:

```groovy
def bindingLayerPin = new groovy.json.JsonSlurper().parse(rootProject.file("../binding-layer.json"))
dependencies {
    implementation files(bindingLayerAar)
    implementation(bindingLayerPin.jna)
}
```

An Xcode project references `ios/binding-layer/ZingoBindings` as a local
package. The consumer's CI fetches each platform in its own job with
`--check-tag`.

## Tests

```sh
node --test test/*.test.mjs
```

## Publish

The workflow "Bundle Pin package" publishes this directory to npm on a push
to `dev` whenever npm lacks the version that `package.json` names, so every
published version is a merged commit and its provenance names that commit. A
pull request that changes `bin`, `lib`, or `package.json` must raise the
version, and the same workflow fails the pull request when it does not.
