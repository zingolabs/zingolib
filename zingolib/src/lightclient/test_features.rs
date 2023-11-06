use super::*;

impl LightClient {
    pub fn is_mobile_target() -> bool {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            true
        }
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            false
        }
    }
}
