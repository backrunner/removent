use crate::ffi;
use std::ffi::c_void;
use std::fmt;

/// Trait for Core Foundation values that can be inserted into CF collections.
///
/// # Safety
///
/// Implementors must return a non-null, live Core Foundation object pointer
/// of the represented type. The pointer must remain valid for the duration of
/// the borrow and support generic Core Foundation retain/release operations.
pub unsafe trait AsCFType {
    /// Borrow the underlying Core Foundation object pointer.
    fn as_ptr(&self) -> *mut c_void;

    /// Clone this value as an erased [`CFType`].
    #[must_use]
    fn to_cf_type(&self) -> CFType {
        let retained = unsafe { ffi::cf_type_retain(self.as_ptr()) };
        unsafe { CFType::from_raw(retained) }.expect("retained CFType pointer must be non-null")
    }
}

/// Owned, type-erased `CFTypeRef`.
pub struct CFType(*mut c_void);

impl CFType {
    /// Adopts a +1 retained `CFTypeRef` and returns `None` for null.
    ///
    /// # Safety
    ///
    /// A non-null `ptr` must be a live Core Foundation object pointer carrying
    /// one retain that is transferred to the returned wrapper. The caller must
    /// not release or separately adopt that transferred retain.
    #[must_use]
    pub unsafe fn from_raw(ptr: *mut c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    /// Retains a +0 borrowed `CFTypeRef` and returns an owned wrapper.
    ///
    /// # Safety
    ///
    /// A non-null `ptr` must be a live Core Foundation object pointer for the
    /// duration of the retain call.
    #[must_use]
    pub unsafe fn from_raw_borrowed(ptr: *mut c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            let retained = unsafe { ffi::cf_type_retain(ptr) };
            unsafe { Self::from_raw(retained) }
        }
    }

    /// Borrow the raw +0 `CFTypeRef` pointer while `self` remains alive.
    #[must_use]
    pub const fn as_ptr(&self) -> *mut c_void {
        self.0
    }

    /// Runtime type identifier of the wrapped object.
    #[must_use]
    pub fn type_id(&self) -> usize {
        unsafe { ffi::cf_type_get_type_id(self.0) }
    }

    /// `CFHash` of the wrapped object.
    #[must_use]
    pub fn hash_code(&self) -> usize {
        unsafe { ffi::cf_type_hash(self.0) }
    }

    /// Human-readable Core Foundation description.
    #[must_use]
    pub fn description(&self) -> String {
        if self.type_id() == unsafe { ffi::cf_run_loop_get_type_id() } {
            return format!("<CFRunLoop {:p}>", self.0);
        }
        let ptr = unsafe { ffi::acf_cf_type_copy_description_string(self.0) };
        unsafe { crate::cf::CFString::from_raw(ptr) }
            .map_or_else(String::new, |description| description.to_string_lossy())
    }
}

crate::utils::retained::cf_retained!(
    CFType,
    retain = ffi::cf_type_retain,
    release = ffi::cf_type_release,
);

unsafe impl AsCFType for CFType {
    fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}

impl PartialEq for CFType {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ffi::cf_type_equal(self.0, other.0) }
    }
}

impl Eq for CFType {}

impl std::hash::Hash for CFType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.hash_code().hash(state);
    }
}

impl fmt::Debug for CFType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CFType")
            .field("ptr", &self.0)
            .field("type_id", &self.type_id())
            .field("description", &self.description())
            .finish()
    }
}

impl fmt::Display for CFType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.description())
    }
}

/// Owned Swift bridge holder object used for callback-heavy wrappers.
pub struct SwiftObject(*mut c_void);

impl SwiftObject {
    /// Wraps a +1 retained bridge object pointer and returns `None` for null.
    #[must_use]
    pub(crate) fn from_raw_owned(ptr: *mut c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    /// Returns the wrapped raw bridge object pointer.
    #[must_use]
    pub(crate) const fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}

crate::utils::retained::cf_retained!(
    SwiftObject,
    retain = ffi::acf_object_retain,
    release = ffi::acf_object_release,
);

impl PartialEq for SwiftObject {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for SwiftObject {}

impl std::hash::Hash for SwiftObject {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        unsafe { ffi::acf_object_hash(self.0) }.hash(state);
    }
}

impl fmt::Debug for SwiftObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SwiftObject").field("ptr", &self.0).finish()
    }
}

macro_rules! impl_cf_type_wrapper {
    ($name:ident, $type_id_fn:ident) => {
        #[derive(Clone, PartialEq, Eq, Hash)]
        #[doc = concat!("Safe wrapper around a retained Core Foundation `", stringify!($name), "` reference.")]
        pub struct $name(pub(crate) crate::cf::base::CFType);

        impl $name {
            #[doc = concat!("Adopts a +1 retained `", stringify!($name), "` pointer and returns `None` for null.")]
            ///
            /// # Safety
            ///
            #[doc = concat!("A non-null `ptr` must be a live `", stringify!($name), "` pointer of the exact dynamic type carrying one retain transferred to this wrapper. The caller must not release or separately adopt that transferred retain.")]
            #[must_use]
            pub unsafe fn from_raw(ptr: *mut std::ffi::c_void) -> Option<Self> {
                unsafe { crate::cf::base::CFType::from_raw(ptr) }.map(Self)
            }

            #[doc = concat!("Retains a +0 borrowed `", stringify!($name), "` pointer and returns an owned wrapper.")]
            ///
            /// # Safety
            ///
            #[doc = concat!("A non-null `ptr` must be a live `", stringify!($name), "` pointer of the exact dynamic type for the duration of the retain call.")]
            #[must_use]
            pub unsafe fn from_raw_borrowed(ptr: *mut std::ffi::c_void) -> Option<Self> {
                unsafe { crate::cf::base::CFType::from_raw_borrowed(ptr) }.map(Self)
            }

            /// Borrows the raw +0 Core Foundation pointer while `self` remains alive.
            #[must_use]
            pub const fn as_ptr(&self) -> *mut std::ffi::c_void {
                self.0.as_ptr()
            }

            #[doc = concat!("Returns the Core Foundation type ID for `", stringify!($name), "`.")]
            #[must_use]
            pub fn type_id() -> usize {
                unsafe { crate::ffi::$type_id_fn() }
            }

            /// Consumes this wrapper and returns the erased `CFType`.
            #[must_use]
            pub fn into_cf_type(self) -> crate::cf::base::CFType {
                self.0
            }
        }

        unsafe impl crate::cf::base::AsCFType for $name {
            fn as_ptr(&self) -> *mut std::ffi::c_void {
                self.as_ptr()
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name))
                    .field("ptr", &self.as_ptr())
                    .field("description", &self.0.description())
                    .finish()
            }
        }
    };
}

/// Re-exports the wrapper-generation macro within this crate.
pub(crate) use impl_cf_type_wrapper;
