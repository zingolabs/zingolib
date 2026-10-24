# The Binding Layer

- [Binding Layer](../zingolib/CONTEXT.md#consumers)
- [Prebuilt Bundle](../zingolib/CONTEXT.md#consumers)
- [Bundle Pin](../zingolib/CONTEXT.md#consumers)
- [Published Commit](../zingolib/CONTEXT.md#consumers)
- [Publication Record](../zingolib/CONTEXT.md#consumers)

zingolib supports React Native mobile development. This file is the guide
to building and publishing the Binding Layer.

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

`zingo-ffi/lib/src/exported_surface.snapshot.txt` is a snapshot of the
exported surface. The test `the_library_exports_the_committed_surface` in
`zingo-ffi/lib/tests/exported_surface.rs` diffs the sources against it on
every pull request and prints the lines to add or remove. A change to an
exported function, record or error updates the snapshot in the same pull
request.

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
commit the record names. The record's push and pull request use the GitHub
App `zingolib-binding-layer-publisher`. A consumer pins and fetches with
the package in [`pin/README.md`](pin/README.md):

```sh
yarn --silent pin <commit> --platform android --platform ios
yarn --silent fetch:binding-layer --platform <platform>
```
