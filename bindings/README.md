# The Binding Layer: build and publish the React Native bundles

zingolib supports React Native mobile development. The Binding Layer is the
set of native libraries and generated Kotlin and Swift sources that a React
Native app links against: the wallet crate `zingo-ffi` and the proxy crate
`nym-proxy-ffi`, each exported through uniffi. A mobile app never builds
zingolib itself. It fetches a Prebuilt Bundle, one per platform, that a
publication of a commit of `dev` produced. This document explains how a
zingolib developer builds the Binding Layer locally and how a publication
runs.

## Vocabulary

- **Binding Layer.** The native libraries and the Kotlin and Swift Binding
  Sets built from one commit of zingolib.
- **Prebuilt Bundle.** One archive per platform, Android and iOS, that holds
  the Binding Layer for one commit. The Android bundle is a Maven repository
  layout that serves one AAR; the iOS bundle is one XCFramework beside the
  `ZingoBindings` Swift package.
- **Published Commit.** The commit of `dev` that a bundle was built from. It
  is the bundle's identity: the registry tag, the Maven version suffix, and
  the pin all carry it.
- **Publication Record.** `bindings/published.toml`, oldest entry first, one
  entry per Published Commit with the digest of each bundle.
  `bindings/CHANGELOG.md` is generated from it.
- **Bundle Pin.** The consumer's `binding-layer.json`, which names one
  Published Commit and its digests. The npm package in
  [`bindings/pin`](pin/README.md) moves the pin and fetches the bundles.

## Prerequisites

The builder is the workbench binary `build-binding-layer` in
`tools/workbench`. Every command below runs from the zingolib root.

- Android builds run inside the tool image that
  `bindings/android/docker/Dockerfile` describes, so a local build needs a
  container engine. CI runs the same image, `ghcr.io/zingolabs/android_builder`.
- iOS builds run on macOS with Xcode. The deployment target is iOS 16.0.
- The Kotlin Binding Sets are compiled by the Gradle library under
  `bindings/android`.

## Build locally

Build the Android libraries for every ABI, or one ABI with `--abi`:

```sh
cargo run --manifest-path tools/workbench/Cargo.toml --bin build-binding-layer -- \
  android --out bindings/android/build/binding-layer
```

The command builds the tool image, starts a container, and runs the Android
plan inside it. The output holds the libraries of every ABI under `jniLibs`
and the Kotlin Binding Sets under `kotlin`. Inside a job that already runs in
the image, pass `--in-image` to skip the container. To regenerate the Kotlin
Binding Sets alone, without a container, the `kotlin` command builds the two
crates for the host and runs bindgen against them:

```sh
cargo run --manifest-path tools/workbench/Cargo.toml --bin build-binding-layer -- \
  kotlin --out <directory>
```

On macOS, build the three iOS targets, `aarch64-apple-ios`,
`aarch64-apple-ios-sim` and `x86_64-apple-ios`, then assemble the XCFramework
and the Swift package into the fixed directory that
`bindings/swift/Package.swift` reads:

```sh
cargo run --manifest-path tools/workbench/Cargo.toml --bin build-binding-layer -- \
  ios --target aarch64-apple-ios
cargo run --manifest-path tools/workbench/Cargo.toml --bin build-binding-layer -- \
  ios --assemble
```

Every library builds under the `mobile` cargo profile. The artifact names and
the Maven version of a commit come from one more command, which the publish
workflow runs so that every job of one run agrees on the names:

```sh
cargo run --manifest-path tools/workbench/Cargo.toml --bin build-binding-layer -- \
  artifact --commit <commit>
```

The exported surface of the wallet crate is pinned by
`zingo-ffi/lib/src/exported_surface.txt`. A change to an exported function, a
record, or the error enum must update that file in the same pull request, or
the test `zingo-ffi/lib/tests/exported_surface.rs` fails.

## Publish

A publication is one dispatch of the workflow "Publish the Binding Layer",
`.github/workflows/binding-layer-publish.yaml`, by a developer with write
access to the repository:

```sh
gh workflow run binding-layer-publish.yaml --ref dev
```

The head of `dev` is published. `--field commit=<sha>` names an older commit
of `dev` instead; the plan then tags it `publish/<commit>` and relays a second
run at that tag, so the commit's own workflows and tool build it. The run:

1. builds the Android bundle through `binding-layer-android.yaml` and the iOS
   bundle through `binding-layer-ios.yaml`;
2. pushes each bundle to `ghcr.io` with the workbench tool `binding-publish`,
   tagged by the Published Commit;
3. appends the entry to `bindings/published.toml`, regenerates
   `bindings/CHANGELOG.md`, and pushes both on the branch `record/<commit>`;
4. opens the pull request that merges the record. The checks "Binding Layer
   manifest" and "Binding Layer changelog" verify the entry against the
   registry there.

The plan refuses a commit older than the last entry, refuses a publication
while another record's pull request is pending, and ends after the plan for a
commit the record already names. Nothing waits on the record's merge: the
registry holds the bundles as soon as step 2 ends.

The record's push and pull request use an installation token of the GitHub
App `zingolib-binding-layer-publisher`. If the App lacks the permission to
open pull requests, step 4 fails after the record branch is pushed, and the
developer opens the pull request from `record/<commit>` by hand.

## Consume

A consumer pins a Published Commit in its `binding-layer.json` and fetches
with the npm package `zingolib-bundle-pin`:

```sh
yarn --silent pin <commit> --platform android --platform ios
yarn --silent fetch:binding-layer --platform <platform>
```

[`bindings/pin/README.md`](pin/README.md) documents the package, the
permission bound its scripts run under, and the Gradle and Xcode wiring. The
reference consumer is [kitchen-sync](https://github.com/zingolabs/kitchen-sync),
and zingo-mobile consumes the same way.
