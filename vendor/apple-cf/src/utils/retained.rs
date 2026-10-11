//! Declarative macro for retain/release wrapper boilerplate.
//!
//! Many wrapper types hold a single raw pointer to a retained Core Foundation
//! / Swift bridge object and hand-roll identical `Clone` (retain) and `Drop`
//! (release) implementations. `cf_retained!` consolidates that boilerplate
//! into a single audited place.
//!
//! The generated impls preserve the exact behavior of the previous
//! hand-written versions:
//! - `Clone` bumps the retain count by calling the supplied `retain` FFI fn.
//! - `Drop` calls the supplied `release` FFI fn. The crate has three historic
//!   drop shapes, selected per-invocation so behavior is byte-for-byte
//!   identical to the code it replaces:
//!     * default — null-check the pointer before releasing
//!       (`if !ptr.is_null() { release(ptr) }`).
//!     * `drop = unchecked` — release unconditionally (no null guard).
//!     * `drop = null_out` — null-check, release, then reset the field to a
//!       null pointer (matches the Core Graphics wrappers).
//!
//! Types whose `Clone`/`Drop` carry extra logic beyond retain/release (e.g.
//! `CMClock`'s null-returning `Clone`, `CMTimebase`'s locally-declared
//! `CFRetain`/`CFRelease`, or `AudioBufferList`'s custom-allocator teardown)
//! are intentionally left hand-written.

/// Generate `Clone` and `Drop` impls for a retain/release pointer wrapper.
///
/// Variants (tuple newtype `.0` unless `field = <name>` is given):
/// - Null-checked drop:
///   `cf_retained!(Ty, retain = path::retain, release = path::release);`
///   `cf_retained!(Ty, field = ptr, retain = path::retain, release = path::release);`
/// - Unchecked drop (no null guard):
///   `cf_retained!(Ty, retain = path::retain, release = path::release, drop = unchecked);`
///   `cf_retained!(Ty, field = ptr, retain = path::retain, release = path::release, drop = unchecked);`
/// - Null-checked drop that nulls the field afterwards:
///   `cf_retained!(Ty, field = ptr, retain = path::retain, release = path::release, drop = null_out);`
macro_rules! cf_retained {
    // ---- Named-field struct ----

    // Named field: Clone + null-checked Drop that nulls the field afterwards.
    ($ty:ty, field = $field:ident, retain = $retain:path, release = $release:path, drop = null_out $(,)?) => {
        impl Clone for $ty {
            fn clone(&self) -> Self {
                Self {
                    $field: unsafe { $retain(self.$field) },
                }
            }
        }

        impl Drop for $ty {
            fn drop(&mut self) {
                if !self.$field.is_null() {
                    unsafe {
                        $release(self.$field);
                    }
                    self.$field = ::core::ptr::null_mut();
                }
            }
        }
    };

    // Named field: Clone + unchecked Drop.
    ($ty:ty, field = $field:ident, retain = $retain:path, release = $release:path, drop = unchecked $(,)?) => {
        impl Clone for $ty {
            fn clone(&self) -> Self {
                Self {
                    $field: unsafe { $retain(self.$field) },
                }
            }
        }

        impl Drop for $ty {
            fn drop(&mut self) {
                unsafe {
                    $release(self.$field);
                }
            }
        }
    };

    // Named field: Clone + null-checked Drop.
    ($ty:ty, field = $field:ident, retain = $retain:path, release = $release:path $(,)?) => {
        impl Clone for $ty {
            fn clone(&self) -> Self {
                Self {
                    $field: unsafe { $retain(self.$field) },
                }
            }
        }

        impl Drop for $ty {
            fn drop(&mut self) {
                if !self.$field.is_null() {
                    unsafe {
                        $release(self.$field);
                    }
                }
            }
        }
    };

    // ---- Tuple newtype (`.0`) ----

    // Tuple: Clone + unchecked Drop.
    ($ty:ty, retain = $retain:path, release = $release:path, drop = unchecked $(,)?) => {
        impl Clone for $ty {
            fn clone(&self) -> Self {
                Self(unsafe { $retain(self.0) })
            }
        }

        impl Drop for $ty {
            fn drop(&mut self) {
                unsafe {
                    $release(self.0);
                }
            }
        }
    };

    // Tuple: Clone + null-checked Drop.
    ($ty:ty, retain = $retain:path, release = $release:path $(,)?) => {
        impl Clone for $ty {
            fn clone(&self) -> Self {
                Self(unsafe { $retain(self.0) })
            }
        }

        impl Drop for $ty {
            fn drop(&mut self) {
                if !self.0.is_null() {
                    unsafe {
                        $release(self.0);
                    }
                }
            }
        }
    };
}

pub(crate) use cf_retained;
