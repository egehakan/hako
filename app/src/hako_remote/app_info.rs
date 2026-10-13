//! What the app says about itself and the computer it runs on.

use ::hako_remote::protocol::{AppInfo, PROTOCOL_VERSION};
use warp_core::channel::ChannelState;

/// The version reported for a build that was not given one.
const UNVERSIONED_BUILD: &str = "dev";

pub(super) fn app_info() -> AppInfo {
    let hostname = hostname();
    AppInfo {
        name: computer_name().unwrap_or_else(|| hostname.clone()),
        version: ChannelState::app_version()
            .unwrap_or(UNVERSIONED_BUILD)
            .to_owned(),
        hostname,
        protocol: PROTOCOL_VERSION,
    }
}

/// The computer's network name.
fn hostname() -> String {
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            // The kernel's host name follows the network the Mac is on and can be a bare address,
            // so the name the Mac announces on the local network is used when it has one.
            if let Some(name) = mac::local_host_name() {
                return format!("{name}.local");
            }
        }
    }
    cfg_if::cfg_if! {
        if #[cfg(unix)] {
            nix::unistd::gethostname()
                .ok()
                .and_then(|name| name.into_string().ok())
                .unwrap_or_default()
        } else {
            std::env::var("COMPUTERNAME").unwrap_or_default()
        }
    }
}

/// The computer's name as its owner set it, where the system has such a name.
fn computer_name() -> Option<String> {
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            mac::computer_name()
        } else {
            None
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::ptr;

    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCDynamicStoreCopyComputerName(
            store: *const c_void,
            name_encoding: *mut u32,
        ) -> CFStringRef;
        fn SCDynamicStoreCopyLocalHostName(store: *const c_void) -> CFStringRef;
    }

    fn owned_string(string: CFStringRef) -> Option<String> {
        if string.is_null() {
            return None;
        }
        // SAFETY: `string` is non-null and came from a Copy function, so it is ours to release.
        Some(unsafe { CFString::wrap_under_create_rule(string) }.to_string())
    }

    /// The name shown in System Settings, such as "Ege's MacBook Pro".
    pub(super) fn computer_name() -> Option<String> {
        // SAFETY: both arguments are optional.
        owned_string(unsafe { SCDynamicStoreCopyComputerName(ptr::null(), ptr::null_mut()) })
    }

    /// The name the Mac goes by on the local network, without the `.local` suffix.
    pub(super) fn local_host_name() -> Option<String> {
        // SAFETY: the argument is optional.
        owned_string(unsafe { SCDynamicStoreCopyLocalHostName(ptr::null()) })
    }
}
