# Verifying the Binding Layer copy

This document tells a reviewer how to verify zingolib's copy of the Binding Layer. `zingolib/0054` sets the rules. The copy merges only after all four gates pass. zingo-mobile repoints to zingolib only after the copy merges.

## Reference points

| Name | Value | Meaning |
|---|---|---|
| Freeze Commit (TFC) | zingo-mobile `f3d1ac9d4` | The copy's source. zingo-mobile's `rust/` stays frozen at TFC until the repoint. |
| Aligned Rev (TAR) | zingolib `bd9e74ee0` | The zingolib rev that TFC pins. This branch starts at TAR. |
| Import commit | `999e935d1` | Imports the copied files with their history. |
| Placement commit | `1e9577b1a` | Places the copied crates as standalone workspaces. |

## Branch rules

- Do not merge `dev` into the copy's branch, and do not rebase it. Either action moves the branch off TAR, and gate 2 fails.
- The pull request's base is `binding_layer_tar`, a branch pinned at TAR. Do not push to `binding_layer_tar`.
- Do not change the copied crates. When a gate fails, report the failure. `zingolib/0054` gives the remedy for each kind of failure.
- After all four gates pass, retarget the pull request to `dev` and merge it. Then delete `binding_layer_tar`.

## The gates

The workbench tool `binding-copy-gate` runs gates 1 to 3. Every invocation takes the same flags:

```sh
cargo run \
  --manifest-path tools/workbench/Cargo.toml \
  --bin binding-copy-gate -- \
  <gate> \
  --mobile <zingo-mobile checkout at TFC> \
  --tfc f3d1ac9d4 \
  --tar bd9e74ee0 \
  --import 999e935d1 \
  --placement 1e9577b1a
```

| Gate | `<gate>` | Checks | Host |
|---|---|---|---|
| 1 | `import` | Every copied path has the same hash as at TFC. | Any |
| 2 | `graph` | The branch starts at TAR and changes only the copy's manifests, `bindings/`, and `tools/workbench/`. Each crate's dependency graph matches TFC's. | Any |
| 3 | `bindings` | The Kotlin and Swift bindings match TFC's byte for byte. | Any |
| 3 | `artifacts` | The AAR matches zingo-mobile's Android artifacts at TFC. | Linux |
| 3 | `ios-artifacts` | The two XCFrameworks and the Swift sources match zingo-mobile's at TFC. | macOS |
| 4 | none | zingo-mobile's test suites pass on a branch that consumes the copy, compared with a baseline at TFC. | Both |

`all` runs every gate that the host can run, and it reports the others as `SKIPPED`.

The `artifacts` gate needs podman or docker, `llvm-nm`, and `unzip`. It reads the AAR from the Gradle library's `bundleReleaseAar` task. Build that AAR with TFC's descriptor:

```sh
ZINGO_MOBILE_GIT_DESCRIBE=zingo-2.0.24-320-134-gf3d1ac9d4 \
  gradle --project-dir bindings/android bundleReleaseAar
```

The Gradle library has no Gradle wrapper, so use Gradle 8.14.3.

## Gate 4

Gate 4 runs zingo-mobile's suites. On Android, these are the `android_integration` suite, which runs `RustFFITest.kt` among the instrumented tests, and the `e2e` suite. On iOS, this is the `ZingoTests` XCTest suite. The baseline is three runs of each suite at TFC. Record the outcome of each test in each run. A test that passes in all three baseline runs and fails on the copy blocks the merge. A test that fails at TFC and fails on the copy counts as preserved function.

zingo-mobile branch `gate4_binding_layer_consumer` starts at TFC and consumes the copy. It exists only for gate 4, and it never merges.

- Android includes `bindings/android` as a Gradle included build. Set `ZINGOLIB_DIR` to a checkout of the copy's branch. Set `ZINGO_MOBILE_GIT_DESCRIBE` to the consumer branch's `git describe`. Do not run `rust/android/build_android.mjs`.
- iOS imports the Swift module `ZingoBindings`. The Xcode project needs the wiring in step 3 of the macOS checklist.
- Run `yarn` in the consumer branch before a build.

## Checklist for gate 4 on Android

### Environment

- Use a Linux host, as zingo-mobile's CI does.
- Install the Android SDK with NDK `28.2.13676358`, JDK 17, Node, yarn, rustup, and `cargo-nextest`.
- Create an x86_64 emulator with the API 34 `default` system image. zingo-mobile's CI uses the same emulator.
- Put `zebrad` and `zainod` on `PATH`, or name their directory in `TEST_BINARIES_DIR`.
- zingo-mobile's `build_android.mjs` calls `docker`. With podman, provide a `docker` command, for example through the `podman-docker` package. Also let podman resolve zingo-mobile's short image name:
  ```sh
  printf 'unqualified-search-registries = ["docker.io"]\n' > ~/registries.conf
  export CONTAINERS_REGISTRIES_CONF=~/registries.conf
  ```

### 1. Baseline at TFC

Check out zingo-mobile at TFC in a throwaway worktree, and build the app with zingo-mobile's own builder:

```sh
git -C <zingo-mobile checkout> worktree add --detach ../zingo-mobile-tfc f3d1ac9d4
cd ../zingo-mobile-tfc
yarn
node rust/android/build_android.mjs
cd android
./gradlew assembleProdRelease assembleProdReleaseAndroidTest \
  -DtestBuildType=release \
  -PsplitApk=true
```

With the emulator running, run each suite three times from `rust/`:

```sh
cargo nextest run android_integration::x86_64 --features ci --release
cargo nextest run e2e::x86_64 --release
```

The `e2e` suite needs Metro. Run `yarn start` in a separate terminal first.

**Deliverable:** the outcome of each test in each of the three runs of each suite.

### 2. Run on the copy

Check out zingo-mobile branch `gate4_binding_layer_consumer`. Point it at a checkout of the copy's branch, and give it the consumer branch's descriptor:

```sh
export ZINGOLIB_DIR=<copy checkout>
export ZINGO_MOBILE_GIT_DESCRIBE=$(git describe --dirty --always --long --match 'zingo-*')
yarn
cd android
./gradlew assembleProdRelease assembleProdReleaseAndroidTest \
  -DtestBuildType=release \
  -PsplitApk=true
```

Do not run `build_android.mjs` on this branch. Gradle builds the Binding Layer from the copy.

With the emulator running, run each suite one time from `rust/`, with the same commands as in step 1.

**Deliverable:** the outcome of each test in each suite. A test that passes in all three baseline runs and fails here blocks the merge.

## Checklist for a macOS reviewer

### Environment

- Use Xcode with the iPhone 16 simulator on iOS 18.5, as zingo-mobile's CI does.
- Install Node, yarn, CocoaPods, and rustup. Put `xcodebuild`, `lipo`, and `nm` on `PATH`.
- zingo-mobile's `build_ios.mjs` runs `rustup default stable`, which changes your global default toolchain. Run `rustup default` first, and record the toolchain it prints.
- Add the iOS targets:
  ```sh
  rustup target add --toolchain stable \
    aarch64-apple-ios \
    aarch64-apple-ios-sim \
    x86_64-apple-ios
  ```

### 1. Gate 3's iOS half

Check out zingo-mobile at TFC in a throwaway worktree, because zingo-mobile's iOS builder writes into it:

```sh
git -C <zingo-mobile checkout> worktree add --detach ../zingo-mobile-tfc f3d1ac9d4
```

From the root of the copy's branch, run the `ios-artifacts` gate with `--mobile ../zingo-mobile-tfc`.

**Deliverable:** the gate's full output.

### 2. Gate 4's iOS baseline at TFC

In `zingo-mobile-tfc`, prepare the build:

```sh
yarn
node rust/ios/build_ios.mjs
cd ios && pod install
```

From `ios/`, run the XCTest suite three times:

```sh
xcodebuild test \
  -workspace Zingo.xcworkspace \
  -scheme Zingo \
  -sdk iphonesimulator \
  -configuration Debug \
  -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' \
  -only-testing:ZingoTests
```

**Deliverable:** the outcome of each test in each run. If zingo-mobile runs Detox on iOS, with the `ios.sim.debug` configuration, record three Detox runs as well.

### 3. Xcode wiring on the consumer branch

Check out `gate4_binding_layer_consumer`. From the root of the copy's branch, build the SwiftPM package with the consumer branch's descriptor:

```sh
ZINGO_MOBILE_GIT_DESCRIBE=$(git -C <consumer checkout> describe --dirty --always --long --match 'zingo-*') \
  cargo run \
    --manifest-path tools/workbench/Cargo.toml \
    --bin build-binding-layer -- \
    ios --out bindings/swift/build
```

In Xcode, in the consumer branch's project:

1. Remove the references to `Zingolib.xcframework`, `ZingoNymProxyFFI.xcframework`, `zingo.swift`, and `zingo_nym_proxy_ffi.swift`.
2. Add `<copy checkout>/bindings/swift` as a local package.
3. Link the `ZingoBindings` product into the `Zingo` and `ZingoTests` targets.

**Deliverable:** the app builds, and the change to `project.pbxproj` is pushed to the consumer branch.

### 4. Gate 4's iOS run on the copy

On the consumer branch, run `yarn`, run `pod install` from `ios/`, and run the `xcodebuild test` command from step 2 one time.

**Deliverable:** the outcome of each test.

### Finish

Restore the toolchain that you recorded:

```sh
rustup default <recorded toolchain>
```

## Known differences from zingo-mobile

- The generated Swift compiles into the SwiftPM module `ZingoBindings`, not the app's module. `zingolib/0054` records this exception.
- The Gradle library has no Gradle wrapper, because a wrapper is a shell script.

## History

Gate 3's Android half first failed on the proxy's Kotlin. zingo-mobile generates that Kotlin from inside `nym-proxy-ffi`, where the crate's `uniffi.toml` sets `android = true`. The copy's builder first generated it from the wallet crate. The builder now uses zingo-mobile's directory for each binding set. The copied crates did not change.
