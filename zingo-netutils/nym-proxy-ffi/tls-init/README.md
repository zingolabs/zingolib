# zingo-nym-tls-init

Android JNI initializer for the Nym proxy shim's TLS verifier.

One exported function,
`Java_org_ZingoLabs_Zingo_NymTlsInit_initPlatformVerifier`, hands the app
`Context` to `rustls-platform-verifier`. The Kotlin side
(`android/app/src/main/java/org/ZingoLabs/Zingo/NymTlsInit.kt`) calls it
once before the shim opens a mixnet connection.

The shim crate (`..`) links this crate on `target_os = "android"` only. On
every other target the crate is empty.
