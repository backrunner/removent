//! Per-data-directory named pipe with a protected owner-only DACL.
use std::{ffi::c_void, io};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
pub fn pipe_name(paths: &crate::DataPaths) -> String {
    use sha2::{Digest, Sha256};
    let root = paths
        .root
        .canonicalize()
        .unwrap_or_else(|_| paths.root.clone());
    format!(
        r"\\.\pipe\removent-{}",
        hex::encode(Sha256::digest(
            root.to_string_lossy().to_lowercase().as_bytes()
        ))
    )
}
pub fn server(paths: &crate::DataPaths, first: bool) -> io::Result<NamedPipeServer> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SECURITY_ATTRIBUTES,
        },
    };
    let sddl: Vec<u16> = "D:P(A;;GA;;;OW)\0".encode_utf16().collect();
    let mut descriptor = std::ptr::null_mut();
    // The owner-rights SID grants access only to the pipe's owner. Remote clients
    // are rejected independently of the ACL. The descriptor is copied by CreateNamedPipe.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut attrs = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let result = ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                pipe_name(paths),
                (&mut attrs as *mut SECURITY_ATTRIBUTES).cast::<c_void>(),
            );
        LocalFree(descriptor);
        result
    }
}
