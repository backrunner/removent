//! Consult local OS routing state, never an external Internet/ping service.
//! Unknown status permits connection attempts rather than trapping the UI offline.

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(super) fn online() -> bool {
    use std::{ffi::c_void, ptr};
    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCNetworkReachabilityCreateWithAddress(
            allocator: *const c_void,
            address: *const libc::sockaddr,
        ) -> *const c_void;
        fn SCNetworkReachabilityGetFlags(target: *const c_void, flags: *mut u32) -> u8;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
    }
    // IPv4 AND IPv6: an IPv6-only network is still online. These are local
    // default-route queries; no DNS, sockets or packets are created.
    unsafe {
        let mut v4: libc::sockaddr_in = std::mem::zeroed();
        v4.sin_len = std::mem::size_of_val(&v4) as u8;
        v4.sin_family = libc::AF_INET as u8;
        let mut v6: libc::sockaddr_in6 = std::mem::zeroed();
        v6.sin6_len = std::mem::size_of_val(&v6) as u8;
        v6.sin6_family = libc::AF_INET6 as u8;
        [
            &v4 as *const _ as *const libc::sockaddr,
            &v6 as *const _ as *const libc::sockaddr,
        ]
        .into_iter()
        .any(|address| {
            let target = SCNetworkReachabilityCreateWithAddress(ptr::null(), address);
            if target.is_null() {
                return true;
            }
            let mut flags = 0;
            let valid = SCNetworkReachabilityGetFlags(target, &mut flags) != 0;
            CFRelease(target);
            !valid || flags_online(flags)
        })
    }
}

#[cfg(any(target_os = "macos", target_os = "ios", test))]
fn flags_online(flags: u32) -> bool {
    const REACHABLE: u32 = 1 << 1;
    const REQUIRED: u32 = 1 << 2;
    const INTERVENTION: u32 = 1 << 4;
    const AUTOMATIC: u32 = (1 << 3) | (1 << 5);
    flags & REACHABLE != 0
        && (flags & REQUIRED == 0 || (flags & AUTOMATIC != 0 && flags & INTERVENTION == 0))
}

// The desktop and mobile applications currently target Apple platforms.
// Other library consumers retain bounded transport recovery without OS gating.
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub(super) fn online() -> bool {
    true
}

#[cfg(test)]
#[test]
fn route_flags_do_not_confuse_a_manual_connection_with_online() {
    assert!(!flags_online(0));
    assert!(flags_online(2));
    assert!(!flags_online(2 | 4));
    assert!(flags_online(2 | 4 | 32));
    assert!(!flags_online(2 | 4 | 32 | 16));
}
