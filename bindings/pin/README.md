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
  }
}
```

## Commands

Both commands run from the consumer's root. `--root <dir>` names another root,
and `--pin <file>` another pin file.

`pin-binding-layer <commit> [--platform <platform>]` moves the pin. It asks
the registry for the digest of each pinned platform's bundle at that commit
and rewrites the pin for every platform or for none. The first move names the
platforms with `--platform`.

`fetch-binding-layer --platform <platform> [--check-tag]` prints the path of
the pinned bundle, downloading it first when the checkout does not hold it.
Every bundle is one gzip archive, and the command refuses a bundle of any
other shape, which zingolib published before the Maven repository layout.
With `--check-tag` the command also verifies that the registry's tag for the
pinned commit still resolves to the pinned digest.

The Android bundle lands under `android/build/binding-layer/<digest>/`, beside
a marker file that names the archive, and is extracted there as a Maven
repository in canonical layout: the AAR, the POM that declares the AAR's
dependencies, and `maven-metadata.xml`. The command prints two lines, the
repository root and then the coordinate it reads from the metadata, such as
`org.zingolabs:zingo-binding-layer:6.0.0-ba7336710419c9ac`. The version is
the zingolib crate version and the first sixteen characters of the pinned
commit, and the command refuses a layout whose version names another commit.

The iOS bundle is extracted to `ios/binding-layer/ZingoBindings`, beside a
marker file that names its digest, and the command prints that package path.

## Consumers

A Gradle build runs the fetch at configuration time, adds the first printed
line as a Maven repository and the second as a dependency, and Gradle resolves
what the POM declares, JNA among it, from Maven Central:

```groovy
def bindingLayer = providers.exec {
    workingDir rootProject.file("..")
    commandLine "node_modules/.bin/fetch-binding-layer", "--platform", "android"
}.standardOutput.asText.map { it.trim().split("\n") }
repositories {
    maven { url = uri(bindingLayer.get()[0]) }
}
dependencies {
    implementation(bindingLayer.get()[1])
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
