# The Binding Layer

zingolib supports React Native mobile development. This file is the guide
to building and publishing the Binding Layer.

## Terminology

The glossary in [`zingolib/CONTEXT.md`](../zingolib/CONTEXT.md#consumers)
defines these terms:

- [Binding Layer](../zingolib/CONTEXT.md#consumers)
- [Prebuilt Bundle](../zingolib/CONTEXT.md#consumers)
- [Bundle Pin](../zingolib/CONTEXT.md#consumers)
- [Published Commit](../zingolib/CONTEXT.md#consumers)
- [Publication Record](../zingolib/CONTEXT.md#consumers)

## Build

The builder is `build-binding-layer` in `tools/workbench`, run from the
zingolib root. Android builds run in the tool image that
`bindings/android/docker/Dockerfile` describes, so a local build needs a
container engine; CI runs the same image. iOS builds need macOS and Xcode.
Every library builds under the `mobile` cargo profile.

```sh
# Android: builds the tool image, runs the plan in a container, writes every
# ABI's libraries under jniLibs and the Kotlin Binding Sets under kotlin.
cargo run --manifest-path tools/workbench/Cargo.toml --bin build-binding-layer -- \
  android --out bindings/android/build/binding-layer
# Kotlin Binding Sets alone, from host builds, with no container.
cargo run --manifest-path tools/workbench/Cargo.toml --bin build-binding-layer -- \
  kotlin --out <directory>
# iOS: one build per target (aarch64-apple-ios, aarch64-apple-ios-sim,
# x86_64-apple-ios), then one assembly into the directory Package.swift reads.
cargo run --manifest-path tools/workbench/Cargo.toml --bin build-binding-layer -- \
  ios --target aarch64-apple-ios
cargo run --manifest-path tools/workbench/Cargo.toml --bin build-binding-layer -- \
  ios --assemble
```

A change to an exported function, record or error must update
`zingo-ffi/lib/src/exported_surface.txt` in the same pull request, or the
test `zingo-ffi/lib/tests/exported_surface.rs` fails.

## Publish and consume

A publication is one dispatch of the workflow "Publish the Binding Layer" by
a developer with write access. It builds both bundles, pushes them to
`ghcr.io` tagged by the Published Commit, appends the entry to the
Publication Record, regenerates the changelog, pushes both files on
`record/<commit>`, and opens the pull request that merges them. Nothing
waits on that merge; the registry holds the bundles once the push ends.

```sh
# The head of dev, or one older commit of dev.
gh workflow run binding-layer-publish.yaml --ref dev
gh workflow run binding-layer-publish.yaml --ref dev --field commit=<sha>
```

An older commit is tagged `publish/<commit>` and built by a relayed run. The
plan refuses a commit older than the last entry, a pending record, and a
commit the record names. The record's pull request comes from the GitHub
App `zingolib-binding-layer-publisher`; while it lacks pull-request write,
open it from `record/<commit>` by hand. A consumer pins and fetches with
the package in [`pin/README.md`](pin/README.md):

```sh
yarn --silent pin <commit> --platform android --platform ios
yarn --silent fetch:binding-layer --platform <platform>
```
