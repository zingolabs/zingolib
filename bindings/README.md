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

Each term below is defined here and nowhere else. The glossary in
`zingolib/CONTEXT.md` and every other document point at these headings.

### Binding Layer

The foreign-language projection of zingolib's consumer surface: the two UniFFI crates (the wallet crate and the Nym proxy crate), the Swift and Kotlin Binding Sets generated from them, thin idiomatic wrappers over those Binding Sets, and the platform packaging that ships them. It knows no UI framework, so any mobile consumer, zingo-mobile today or a future Edge integration, can link it unchanged. Ruled 2026-09-23, pending review. *Avoid*: FFI (ambiguous between one Rust crate and the whole layer), the mobile Rust. See `zingo-adrs zingolib/0054`.

### Prebuilt Bundle

The compiled and packaged Binding Layer for one platform, which zingolib builds and publishes and a consumer fetches without rebuilding it. The Android Prebuilt Bundle is a Maven repository directory holding one AAR, which carries the four ABIs' native libraries and the compiled Kotlin Binding Sets, and the POM that declares the AAR's dependencies, so a consumer adds the directory as a repository and declares the AAR by its coordinate; the iOS Prebuilt Bundle is the `ZingoBindings` Swift package holding one XCFramework, `Zingolib.xcframework`, whose one archive carries both crates, and the Swift Binding Sets. Ruled 2026-09-23 as a loose `.so` set or XCFramework; restated 2026-10-12 to the packaged form that #2833 publishes; restated 2026-10-15 to the Maven layout, so the POM carries the JNA dependency; restated 2026-10-22 to one XCFramework, so the app links one Rust std. *Avoid*: bundle alone, and never "bundle" for grouping changes into one pull request. See `zingo-adrs zingolib/0054` and `zingolib/0057`.

### Bundle Pin

A consumer's fixed choice of zingolib's Prebuilt Bundles: one zingolib commit, and the digest of that commit's bundle for each platform the consumer uses, kept in one lock file in the consumer's repository. The commit's tag in the registry never moves, and the pin holds the commit to its recorded digests. Ruled 2026-10-12. *Avoid*: the submodule pin (the pin zingo-mobile held before #2930), the lock alone.

### Published Commit

The zingolib commit a Prebuilt Bundle was built from, and the one identity of a published build: every other reference to the build derives from it or is recorded against it, and no record, pin, check or message names a build by anything else. Ruled 2026-10-20. *Avoid*: the version, the build, the tag (the registry's spelling of the commit), the descriptor.

### Publication Record

zingolib's own record of the Prebuilt Bundles it has published, one entry per Published Commit with the digest of each platform's bundle and the crates the build audited, kept in the repository and reconciled against the registry by a required check. A consumer never reads it; the registry is the one source a consumer reads. Ruled 2026-10-20. *Avoid*: the manifest (which names the registry's OCI image manifest, where a bundle's digest and annotations live), the ledger, `published.toml` in prose.

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
