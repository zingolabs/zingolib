# zingolib-bundle-pin

The package that a consumer of the Binding Layer installs to hold its Bundle
Pin and to fetch the pinned Prebuilt Bundles. zingolib publishes it to npm
from [`bindings/pin`][pin], so every consumer runs one definition of the
scripts, and the scripts and the bundle layout they read change in one pull
request.

## Install

```sh
yarn add --dev zingolib-bundle-pin
```

The package depends on nothing beyond Node 22.13, the first release whose
permission model is stable. It spawns no process: both bundles are
extracted in process, and the extractor writes regular files and
directories only.

## The Bundle Pin

The Bundle Pin is defined in [the zingolib glossary][vocabulary]. The file is
`binding-layer.json` at the consumer's root. It names the Published Commit,
the digest of that commit's Prebuilt Bundle for each platform the consumer
uses, and, for each platform whose bundle is a Maven repository, the
coordinate of the artifact in it:

```json
{
  "commit": "<40 hexadecimal characters>",
  "digests": {
    "android": "sha256:<64 hexadecimal characters>",
    "ios": "sha256:<64 hexadecimal characters>"
  },
  "coordinates": {
    "android": "org.zingolabs:zingo-binding-layer:<version>"
  }
}
```

The pin is the one source of the coordinate. A Gradle build reads the group
and the dependency from the pin without running the fetch, and the fetch
refuses a bundle whose coordinate differs from the pin.

## Commands

Both commands run from the consumer's root. `--root <dir>` names another root,
and `--pin <file>` another pin file.

`pin-binding-layer <commit> [--platform <platform>]` moves the pin. It asks
the registry for the digest of each pinned platform's bundle at that commit
and rewrites the pin for every platform or for none. The first move names the
platforms with `--platform`. For each platform whose bundle is a Maven
repository the command takes the coordinate from the bundle's manifest, where
zingolib's publish annotates it from the Maven metadata in the bundle, and
writes it into the pin. The command downloads nothing, and it refuses a Maven
bundle published before zingolib annotated the coordinate. It prints one line
per platform: the platform, the digest, and the coordinate where there is
one.

`fetch-binding-layer --platform <platform> [--check-tag]` prints the path of
the pinned bundle, downloading it first when the checkout does not hold it.
Every bundle is one gzip archive in the pax tar format, and the command
refuses a bundle of any other shape: one zingolib published before the
Maven repository layout, or one packed before zingolib fixed the format,
whose long paths sit in GNU long-name entries. The extractor also refuses
links and any path that leaves the archive root.
With `--check-tag` the command also verifies that the registry's tag for the
pinned commit still resolves to the pinned digest.

The Android bundle lands under `android/build/binding-layer/<digest>/`, beside
a marker file that names the archive, and is extracted there as a Maven
repository in canonical layout: the AAR, the Gradle Module Metadata and the
POM that declare the AAR's dependencies, and `maven-metadata.xml`. The command
prints two lines, the repository root and then the coordinate it reads from
the metadata, such as `org.zingolabs:zingo-binding-layer:6.0.0-ba7336710419c9ac`.
The version is the zingolib crate version and the first sixteen characters of
the pinned commit. The command refuses a layout whose version names another
commit, and a layout whose coordinate differs from the pin.

The iOS bundle is extracted to `ios/binding-layer/ZingoBindings`, beside a
marker file that names its digest, and the command prints that package path.

## Consumers

A consumer runs both commands only through two scripts in its `package.json`,
copied from here verbatim. Each script runs Node with `--permission` and
opens exactly the paths the command touches: the installed package, the pin
file, the fetch store of each platform and the iOS extraction root, and for
the pin command the pin file and its `.partial` sibling, the latter for
reading as well, since a rename reads its source. No wildcard and no
child process is opened, so a compromised release of this package can reach
nothing else on the build machine. The bound is the consumer's, never the
package's. The scripts name the command's file rather than the
`node_modules/.bin` link, since the permission model follows symbolic links;
the package must be installed as a real directory.

Two rules of the model shape the lines. A directory opened by the model
covers its children and never the directory itself, so everything the
package removes, renames or writes sits one level under an opened
directory: the stores hold one directory per digest, and the iOS root holds
the package, its `.partial` sibling during a swap, and the digest marker.
The model also resolves every opened path when Node starts, and a path that
does not exist yet opens as one file, so the fetch script creates the three
directories it opens before Node starts; the pin script opens files only.

```json
{
  "scripts": {
    "fetch:binding-layer": "mkdir -p android/build/binding-layer ios/build/binding-layer ios/binding-layer && node --permission --allow-fs-read=node_modules/zingolib-bundle-pin --allow-fs-read=binding-layer.json --allow-fs-read=android/build/binding-layer --allow-fs-read=ios/build/binding-layer --allow-fs-read=ios/binding-layer --allow-fs-write=android/build/binding-layer --allow-fs-write=ios/build/binding-layer --allow-fs-write=ios/binding-layer node_modules/zingolib-bundle-pin/bin/fetch_binding_layer.mjs",
    "pin": "node --permission --allow-fs-read=node_modules/zingolib-bundle-pin --allow-fs-read=binding-layer.json --allow-fs-read=binding-layer.json.partial --allow-fs-write=binding-layer.json --allow-fs-write=binding-layer.json.partial node_modules/zingolib-bundle-pin/bin/pin_binding_layer.mjs"
  }
}
```

`yarn pin <commit>` moves the pin, and `yarn --silent fetch:binding-layer
--platform <platform>` fetches. The test `test/permission.test.mjs` runs both
commands under these lines, so a release that needs one path more fails
here before it publishes. The bound does not cover the network.

A Gradle build reads the coordinate from the pin, declares it as a
dependency, and adds the repository root the fetch prints as a Maven
repository that serves the coordinate's group alone. The fetch runs when
Gradle resolves the repository, not at configuration, and Gradle resolves
what the module metadata declares, JNA among it, from Maven Central:

```groovy
def bindingLayerPin = new groovy.json.JsonSlurper().parse(rootProject.file("../binding-layer.json"))
def bindingLayerCoordinate = bindingLayerPin.coordinates.android
def bindingLayerRepository = providers.exec {
    workingDir rootProject.file("..")
    commandLine "yarn", "--silent", "fetch:binding-layer", "--platform", "android"
}.standardOutput.asText.map { it.trim().split("\n")[0] }
repositories {
    exclusiveContent {
        forRepository {
            maven { url = bindingLayerRepository.map { uri(it) } }
        }
        filter { includeGroup(bindingLayerCoordinate.split(":")[0]) }
    }
}
dependencies {
    implementation(bindingLayerCoordinate)
}
```

An Xcode project references `ios/binding-layer/ZingoBindings` as a local
package, and its build phase runs `yarn --silent fetch:binding-layer
--platform ios`. The consumer's CI fetches each platform in its own job
with `yarn --silent fetch:binding-layer --platform <platform> --check-tag`.

## Tests

```sh
node --test test/*.test.mjs
```

## Publish

The workflow "Bundle Pin package" publishes [`bindings/pin`][pin] to npm on
a push to `dev` whenever npm lacks the version that `package.json` names, so
every published version is a merged commit and its provenance names that
commit. A pull request that changes `bin`, `lib`, or `package.json` must
raise the version, and the same workflow fails the pull request when it does
not.

[pin]: https://github.com/zingolabs/zingolib/tree/14405155a52f47a9ab690ba99197a3422e5445c9/bindings/pin#zingolib-bundle-pin
[vocabulary]: https://github.com/zingolabs/zingolib/blob/755c3adaea5906cf55357e69ad1b0a27d746cdb8/zingolib/CONTEXT.md#consumers
