#[cfg(target_os = "android")]
mod android {
    use jni::errors::ThrowRuntimeExAndDefault;
    use jni::objects::{JClass, JObject};
    use jni::EnvUnowned;

    /// Captures the JVM and the application context so the shim library's `rustls-platform-verifier` can reach Android's certificate verifier.
    #[no_mangle]
    pub extern "system" fn Java_org_ZingoLabs_Zingo_NymTlsInit_initPlatformVerifier<'local>(
        mut env: EnvUnowned<'local>,
        _class: JClass<'local>,
        context: JObject<'local>,
    ) {
        env.with_env(|env| rustls_platform_verifier::android::init_with_env(env, context))
            .resolve::<ThrowRuntimeExAndDefault>()
    }
}
