# 53. zingolib admits Swift, Kotlin, and TypeScript with their native tooling

Date: 2026-09-23

Status: draft — ruled in a grilling session, pending review

## Context

This repository has been Rust only. Committed tooling was Rust in the
workbench crate, and a new language needed explicit consent: ADR 0028
sought consent before TypeScript entered, and a Python binding test was
removed in favour of a Rust round trip. ADR 0052 moves the Binding Layer
here, and that layer is Swift and Kotlin by nature, with packaging that
only Gradle and SwiftPM produce idiomatically. zingo-mobile's Node build
scripts cannot move unchanged under the old rule.

## Decision

zingolib admits Swift, Kotlin, and TypeScript. Rust code and Rust builds
stay in the Rust workbench. Swift, Kotlin, and TypeScript use their own
modern tooling: SwiftPM for Swift, Gradle with the Kotlin DSL for Kotlin
and Android packaging, and the ecosystem's standard toolchain for
TypeScript. zingo-mobile's `.mjs` build scripts do not move; the workbench
and the native tools replace them. Python and Bash remain barred as
committed files, apart from small task glue.

## Consequences

Contributors to the Binding Layer need the Android SDK and NDK, and a
macOS host for the XCFramework. The Rust-only workspace remains buildable
without either.
