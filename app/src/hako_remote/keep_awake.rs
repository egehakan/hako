//! Keeps the computer from going to sleep on its own while remote access is on, so that it stays
//! reachable. The display may still sleep, and closing a laptop's lid still puts it to sleep.

#[derive(Default)]
pub(super) struct KeepAwake {
    #[cfg(target_os = "macos")]
    assertion: Option<mac::Assertion>,
}

impl KeepAwake {
    pub(super) fn set(&mut self, keep_awake: bool) {
        cfg_if::cfg_if! {
            if #[cfg(target_os = "macos")] {
                match (keep_awake, self.assertion.is_some()) {
                    (true, false) => self.assertion = mac::Assertion::create(),
                    (false, true) => self.assertion = None,
                    (true, true) | (false, false) => {}
                }
            } else {
                let _ = keep_awake;
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};

    const PREVENT_USER_IDLE_SYSTEM_SLEEP: &str = "PreventUserIdleSystemSleep";
    const REASON: &str = "Hako remote access";
    const ASSERTION_LEVEL_ON: u32 = 255;
    const IO_RETURN_SUCCESS: i32 = 0;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            level: u32,
            name: CFStringRef,
            assertion_id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(assertion_id: u32) -> i32;
    }

    /// A power assertion, released when dropped. The system also releases it when the app exits.
    pub(super) struct Assertion(u32);

    impl Assertion {
        pub(super) fn create() -> Option<Self> {
            let assertion_type = CFString::new(PREVENT_USER_IDLE_SYSTEM_SLEEP);
            let reason = CFString::new(REASON);
            let mut assertion_id = 0;
            // SAFETY: both strings outlive the call, and `assertion_id` is a valid place to write.
            let result = unsafe {
                IOPMAssertionCreateWithName(
                    assertion_type.as_concrete_TypeRef(),
                    ASSERTION_LEVEL_ON,
                    reason.as_concrete_TypeRef(),
                    &mut assertion_id,
                )
            };
            if result != IO_RETURN_SUCCESS {
                log::warn!("[Remote access] could not keep the Mac awake: IOReturn {result:#x}");
                return None;
            }
            Some(Self(assertion_id))
        }
    }

    impl Drop for Assertion {
        fn drop(&mut self) {
            // SAFETY: the id came from a successful `IOPMAssertionCreateWithName` and is released
            // only here.
            let result = unsafe { IOPMAssertionRelease(self.0) };
            if result != IO_RETURN_SUCCESS {
                log::warn!("[Remote access] could not release the power assertion: {result:#x}");
            }
        }
    }
}
