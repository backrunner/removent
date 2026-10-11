//! `CVPixelBuffer` - Video pixel buffer

use crate::cf::{AsCFType, CFDictionary, CFNumber, CFString};
use crate::iosurface::IOSurface;
use crate::{ffi, raw};
use std::collections::HashMap;
use std::fmt;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

/// Lock flags for `CVPixelBuffer`
///
/// This is a bitmask type matching Apple's `CVPixelBufferLockFlags`.
///
/// # Examples
///
/// ```
/// use apple_cf::cv::CVPixelBufferLockFlags;
///
/// // Read-only lock
/// let flags = CVPixelBufferLockFlags::READ_ONLY;
/// assert!(flags.is_read_only());
///
/// // Read-write lock (default)
/// let flags = CVPixelBufferLockFlags::NONE;
/// assert!(!flags.is_read_only());
/// ```
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CVPixelBufferLockFlags(u64);

impl CVPixelBufferLockFlags {
    /// No special options (read-write lock)
    pub const NONE: Self = Self(0);

    /// Read-only lock - use when you only need to read data.
    /// This allows Core Video to keep caches valid.
    pub const READ_ONLY: Self = Self(0x0000_0001);

    /// Create from a raw `CVOptionFlags` value.
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Return the raw `CVOptionFlags` bits.
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Check if this is a read-only lock
    #[must_use]
    pub const fn is_read_only(self) -> bool {
        (self.0 & Self::READ_ONLY.0) != 0
    }

    /// Check if no flags are set (read-write lock)
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl From<CVPixelBufferLockFlags> for u64 {
    fn from(flags: CVPixelBufferLockFlags) -> Self {
        flags.0
    }
}

#[derive(Debug)]
/// Owned wrapper around Apple's `CVPixelBufferRef`.
pub struct CVPixelBuffer(*mut std::ffi::c_void);

impl PartialEq for CVPixelBuffer {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for CVPixelBuffer {}

impl std::hash::Hash for CVPixelBuffer {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        unsafe {
            let hash_value = ffi::cv_pixel_buffer_hash(self.0);
            hash_value.hash(state);
        }
    }
}

impl CVPixelBuffer {
    /// Adopts a +1 retained `CVPixelBufferRef` and returns `None` for null.
    ///
    /// # Safety
    ///
    /// A non-null `ptr` must be a live `CVPixelBufferRef` of the exact type
    /// carrying one retain transferred to this wrapper. The caller must not
    /// release or separately adopt that transferred retain.
    pub unsafe fn from_raw(ptr: *mut std::ffi::c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    /// Retains a +0 borrowed `CVPixelBufferRef` and returns an owned wrapper.
    ///
    /// # Safety
    ///
    /// A non-null `ptr` must be a live `CVPixelBufferRef` of the exact type for
    /// the duration of the retain call.
    #[must_use]
    pub unsafe fn from_raw_borrowed(ptr: *mut std::ffi::c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            let retained = unsafe { ffi::cv_pixel_buffer_retain(ptr) };
            unsafe { Self::from_raw(retained) }
        }
    }

    /// Wraps a raw `CVPixelBufferRef` by taking ownership without retaining it.
    ///
    /// # Safety
    /// `ptr` must be a non-null, live `CVPixelBufferRef` of the exact type
    /// carrying one retain transferred to this wrapper.
    pub const unsafe fn from_ptr(ptr: *mut std::ffi::c_void) -> Self {
        Self(ptr)
    }

    /// Borrows the raw +0 `CVPixelBufferRef` while `self` remains alive.
    #[must_use]
    pub const fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.0
    }

    /// Create a new pixel buffer with the specified dimensions and pixel format
    ///
    /// # Arguments
    ///
    /// * `width` - Width of the pixel buffer in pixels
    /// * `height` - Height of the pixel buffer in pixels
    /// * `pixel_format` - Pixel format type (e.g., 0x42475241 for BGRA)
    ///
    /// # Errors
    ///
    /// Returns a Core Video error code if the pixel buffer creation fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_cf::cv::CVPixelBuffer;
    ///
    /// // Create a 1920x1080 BGRA pixel buffer
    /// let buffer = CVPixelBuffer::create(1920, 1080, 0x42475241)
    ///     .expect("Failed to create pixel buffer");
    ///
    /// assert_eq!(buffer.width(), 1920);
    /// assert_eq!(buffer.height(), 1080);
    /// assert_eq!(buffer.pixel_format(), 0x42475241);
    /// ```
    pub fn create(width: usize, height: usize, pixel_format: u32) -> Result<Self, i32> {
        unsafe {
            let mut pixel_buffer_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            let status =
                ffi::cv_pixel_buffer_create(width, height, pixel_format, &raw mut pixel_buffer_ptr);

            if status == 0 && !pixel_buffer_ptr.is_null() {
                Ok(Self(pixel_buffer_ptr))
            } else {
                Err(status)
            }
        }
    }

    /// Create a pixel buffer from existing memory
    ///
    /// # Arguments
    ///
    /// * `width` - Width of the pixel buffer in pixels
    /// * `height` - Height of the pixel buffer in pixels
    /// * `pixel_format` - Pixel format type (e.g., 0x42475241 for BGRA)
    /// * `base_address` - Pointer to pixel data
    /// * `bytes_per_row` - Number of bytes per row
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - `base_address` points to valid memory
    /// - Memory remains valid for the lifetime of the pixel buffer
    /// - `bytes_per_row` correctly represents the memory layout
    ///
    /// # Errors
    ///
    /// Returns a Core Video error code if the pixel buffer creation fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use apple_cf::cv::CVPixelBuffer;
    ///
    /// // Create pixel data (100x100 BGRA image)
    /// let width = 100;
    /// let height = 100;
    /// let bytes_per_pixel = 4; // BGRA
    /// let bytes_per_row = width * bytes_per_pixel;
    /// let mut pixel_data = vec![0u8; width * height * bytes_per_pixel];
    ///
    /// // Fill with blue color
    /// for y in 0..height {
    ///     for x in 0..width {
    ///         let offset = y * bytes_per_row + x * bytes_per_pixel;
    ///         pixel_data[offset] = 255;     // B
    ///         pixel_data[offset + 1] = 0;   // G
    ///         pixel_data[offset + 2] = 0;   // R
    ///         pixel_data[offset + 3] = 255; // A
    ///     }
    /// }
    ///
    /// // Create pixel buffer from the data
    /// let buffer = unsafe {
    ///     CVPixelBuffer::create_with_bytes(
    ///         width,
    ///         height,
    ///         0x42475241, // BGRA
    ///         pixel_data.as_mut_ptr() as *mut std::ffi::c_void,
    ///         bytes_per_row,
    ///     )
    /// }.expect("Failed to create pixel buffer");
    ///
    /// assert_eq!(buffer.width(), width);
    /// assert_eq!(buffer.height(), height);
    /// ```
    pub unsafe fn create_with_bytes(
        width: usize,
        height: usize,
        pixel_format: u32,
        base_address: *mut std::ffi::c_void,
        bytes_per_row: usize,
    ) -> Result<Self, i32> {
        let mut pixel_buffer_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let status = ffi::cv_pixel_buffer_create_with_bytes(
            width,
            height,
            pixel_format,
            base_address,
            bytes_per_row,
            &raw mut pixel_buffer_ptr,
        );

        if status == 0 && !pixel_buffer_ptr.is_null() {
            Ok(Self(pixel_buffer_ptr))
        } else {
            Err(status)
        }
    }

    /// Fill the extended pixels of a pixel buffer
    ///
    /// This is useful for pixel buffers that have been created with extended pixels
    /// enabled, to ensure proper edge handling for effects and filters.
    ///
    /// # Errors
    ///
    /// Returns a Core Video error code if the operation fails.
    pub fn fill_extended_pixels(&self) -> Result<(), i32> {
        unsafe {
            let status = ffi::cv_pixel_buffer_fill_extended_pixels(self.0);
            if status == 0 {
                Ok(())
            } else {
                Err(status)
            }
        }
    }

    /// Create a pixel buffer with planar bytes
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - `plane_base_addresses` points to valid memory for each plane
    /// - Memory remains valid for the lifetime of the pixel buffer
    /// - All plane parameters correctly represent the memory layout
    ///
    /// # Errors
    ///
    /// Returns a Core Video error code if the pixel buffer creation fails.
    pub unsafe fn create_with_planar_bytes(
        width: usize,
        height: usize,
        pixel_format: u32,
        plane_base_addresses: &[*mut std::ffi::c_void],
        plane_widths: &[usize],
        plane_heights: &[usize],
        plane_bytes_per_row: &[usize],
    ) -> Result<Self, i32> {
        if plane_base_addresses.len() != plane_widths.len()
            || plane_widths.len() != plane_heights.len()
            || plane_heights.len() != plane_bytes_per_row.len()
        {
            return Err(-50); // paramErr
        }

        let mut pixel_buffer_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let status = ffi::cv_pixel_buffer_create_with_planar_bytes(
            width,
            height,
            pixel_format,
            plane_base_addresses.len(),
            plane_base_addresses.as_ptr(),
            plane_widths.as_ptr(),
            plane_heights.as_ptr(),
            plane_bytes_per_row.as_ptr(),
            &raw mut pixel_buffer_ptr,
        );

        if status == 0 && !pixel_buffer_ptr.is_null() {
            Ok(Self(pixel_buffer_ptr))
        } else {
            Err(status)
        }
    }

    /// Create a pixel buffer from an `IOSurface`
    ///
    /// # Errors
    ///
    /// Returns a Core Video error code if the pixel buffer creation fails.
    pub fn create_with_io_surface(surface: &IOSurface) -> Result<Self, i32> {
        unsafe {
            let mut pixel_buffer_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            let status = ffi::cv_pixel_buffer_create_with_io_surface(
                surface.as_ptr(),
                &raw mut pixel_buffer_ptr,
            );

            if status == 0 && !pixel_buffer_ptr.is_null() {
                Ok(Self(pixel_buffer_ptr))
            } else {
                Err(status)
            }
        }
    }

    /// Get the Core Foundation type ID for `CVPixelBuffer`
    #[must_use]
    pub fn type_id() -> usize {
        unsafe { ffi::cv_pixel_buffer_get_type_id() }
    }

    /// Get the data size of the pixel buffer
    #[must_use]
    pub fn data_size(&self) -> usize {
        unsafe { ffi::cv_pixel_buffer_get_data_size(self.0) }
    }

    /// Check if the pixel buffer is planar
    #[must_use]
    pub fn is_planar(&self) -> bool {
        unsafe { ffi::cv_pixel_buffer_is_planar(self.0) }
    }

    /// Get the number of planes in the pixel buffer
    #[must_use]
    pub fn plane_count(&self) -> usize {
        unsafe { ffi::cv_pixel_buffer_get_plane_count(self.0) }
    }

    /// Get the width of a specific plane
    #[must_use]
    pub fn width_of_plane(&self, plane_index: usize) -> usize {
        unsafe { ffi::cv_pixel_buffer_get_width_of_plane(self.0, plane_index) }
    }

    /// Get the height of a specific plane
    #[must_use]
    pub fn height_of_plane(&self, plane_index: usize) -> usize {
        unsafe { ffi::cv_pixel_buffer_get_height_of_plane(self.0, plane_index) }
    }

    /// Get the base address of a specific plane (internal use only)
    ///
    /// # Safety
    /// Caller must ensure the buffer is locked before accessing the returned pointer.
    fn base_address_of_plane_raw(&self, plane_index: usize) -> Option<*mut u8> {
        unsafe {
            let ptr = ffi::cv_pixel_buffer_get_base_address_of_plane(self.0, plane_index);
            if ptr.is_null() {
                None
            } else {
                Some(ptr.cast::<u8>())
            }
        }
    }

    /// Get the bytes per row of a specific plane
    #[must_use]
    pub fn bytes_per_row_of_plane(&self, plane_index: usize) -> usize {
        unsafe { ffi::cv_pixel_buffer_get_bytes_per_row_of_plane(self.0, plane_index) }
    }

    /// Get the extended pixel information (left, right, top, bottom)
    #[must_use]
    pub fn extended_pixels(&self) -> (usize, usize, usize, usize) {
        unsafe {
            let mut left: usize = 0;
            let mut right: usize = 0;
            let mut top: usize = 0;
            let mut bottom: usize = 0;
            ffi::cv_pixel_buffer_get_extended_pixels(
                self.0,
                &raw mut left,
                &raw mut right,
                &raw mut top,
                &raw mut bottom,
            );
            (left, right, top, bottom)
        }
    }

    /// Check if the pixel buffer is backed by an `IOSurface`
    #[must_use]
    pub fn is_backed_by_io_surface(&self) -> bool {
        self.io_surface().is_some()
    }

    /// Get the width of the pixel buffer in pixels
    #[must_use]
    pub fn width(&self) -> usize {
        unsafe { ffi::cv_pixel_buffer_get_width(self.0) }
    }

    /// Returns the height of the pixel buffer in pixels.
    #[must_use]
    pub fn height(&self) -> usize {
        unsafe { ffi::cv_pixel_buffer_get_height(self.0) }
    }

    /// Returns the Core Video pixel format type.
    #[must_use]
    pub fn pixel_format(&self) -> u32 {
        unsafe { ffi::cv_pixel_buffer_get_pixel_format_type(self.0) }
    }

    /// Returns the number of bytes in each row.
    #[must_use]
    pub fn bytes_per_row(&self) -> usize {
        unsafe { ffi::cv_pixel_buffer_get_bytes_per_row(self.0) }
    }

    /// Lock the base address for raw access.
    ///
    /// This is a native synchronization/mapping operation and does not grant
    /// Rust-exclusive access to the backing bytes.
    ///
    /// # Safety
    ///
    /// Every successful call must be paired exactly once with
    /// [`Self::unlock_raw`] using identical flags. The caller must not mix this
    /// protocol with a live RAII guard or allow any derived access to outlive
    /// the matching unlock.
    ///
    /// # Errors
    ///
    /// Returns a Core Video error code if the lock operation fails.
    pub unsafe fn lock_raw(&self, flags: CVPixelBufferLockFlags) -> Result<(), i32> {
        let result = unsafe { raw::CVPixelBufferLockBaseAddress(self.0.cast(), flags.bits()) };
        if result == 0 {
            Ok(())
        } else {
            Err(result)
        }
    }

    /// Unlock the base address after raw access.
    ///
    /// # Safety
    ///
    /// This call must balance exactly one successful [`Self::lock_raw`] call
    /// with identical flags. No pointer or reference derived from that mapping
    /// may be accessed after this call, and the mapping must not belong to an
    /// RAII guard.
    ///
    /// # Errors
    ///
    /// Returns a Core Video error code if the unlock operation fails.
    pub unsafe fn unlock_raw(&self, flags: CVPixelBufferLockFlags) -> Result<(), i32> {
        let result = unsafe { raw::CVPixelBufferUnlockBaseAddress(self.0.cast(), flags.bits()) };
        if result == 0 {
            Ok(())
        } else {
            Err(result)
        }
    }

    /// Get the base address (internal use only)
    ///
    /// # Safety
    /// Caller must ensure the buffer is locked before accessing the returned pointer.
    fn base_address_raw(&self) -> Option<*mut u8> {
        unsafe {
            let ptr = ffi::cv_pixel_buffer_get_base_address(self.0);
            if ptr.is_null() {
                None
            } else {
                Some(ptr.cast::<u8>())
            }
        }
    }

    /// Get the `IOSurface` backing this pixel buffer
    #[must_use]
    pub fn io_surface(&self) -> Option<IOSurface> {
        unsafe {
            let ptr = ffi::cv_pixel_buffer_get_io_surface(self.0);
            IOSurface::from_raw(ptr)
        }
    }

    /// Lock the base address and return a guard for RAII-style access
    ///
    /// # Arguments
    ///
    /// * `flags` - Lock flags (use `CVPixelBufferLockFlags::READ_ONLY` for read-only access)
    ///
    /// # Errors
    ///
    /// Returns a Core Video error code if the lock operation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use apple_cf::cv::{CVPixelBuffer, CVPixelBufferLockFlags};
    ///
    /// fn read_buffer(buffer: &CVPixelBuffer) {
    ///     let guard = buffer.lock(CVPixelBufferLockFlags::READ_ONLY).unwrap();
    ///     // SAFETY: this scope excludes every native and retained alias that
    ///     // could mutate or remap the pixel bytes.
    ///     let data = unsafe { guard.as_slice() }.unwrap();
    ///     println!("Buffer has {} bytes", data.len());
    ///     // Buffer automatically unlocked when guard drops
    /// }
    /// ```
    pub fn lock(&self, flags: CVPixelBufferLockFlags) -> Result<CVPixelBufferLockGuard<'_>, i32> {
        unsafe { self.lock_raw(flags)? };
        Ok(CVPixelBufferLockGuard {
            buffer: self,
            flags,
        })
    }

    /// Lock the base address for read-only access
    ///
    /// This is a convenience method equivalent to `lock(CVPixelBufferLockFlags::READ_ONLY)`.
    ///
    /// # Errors
    ///
    /// Returns a Core Video error code if the lock operation fails.
    pub fn lock_read_only(&self) -> Result<CVPixelBufferLockGuard<'_>, i32> {
        self.lock(CVPixelBufferLockFlags::READ_ONLY)
    }

    /// Lock the base address for read-write access
    ///
    /// This is a convenience method equivalent to `lock(CVPixelBufferLockFlags::NONE)`.
    ///
    /// # Errors
    ///
    /// Returns a Core Video error code if the lock operation fails.
    pub fn lock_read_write(&self) -> Result<CVPixelBufferLockGuard<'_>, i32> {
        self.lock(CVPixelBufferLockFlags::NONE)
    }
}

/// RAII guard for locked `CVPixelBuffer` base address
pub struct CVPixelBufferLockGuard<'a> {
    buffer: &'a CVPixelBuffer,
    flags: CVPixelBufferLockFlags,
}

impl CVPixelBufferLockGuard<'_> {
    fn non_planar_data_len(&self) -> Option<usize> {
        if self.buffer.is_planar() {
            return None;
        }
        let len = self.height().checked_mul(self.bytes_per_row())?;
        (len <= self.data_size() && isize::try_from(len).is_ok()).then_some(len)
    }

    /// Get the base address of the locked buffer.
    ///
    /// Dereferencing the returned pointer is unsafe. The native lock keeps the
    /// mapping synchronized but does not guarantee Rust aliasing or immutability.
    #[must_use]
    pub fn base_address(&self) -> *const u8 {
        self.buffer
            .base_address_raw()
            .unwrap_or(std::ptr::null_mut())
            .cast_const()
    }

    /// Get mutable base address (only valid for read-write locks).
    ///
    /// Returns `None` if this is a read-only lock.
    /// Dereferencing the returned pointer requires unique access to the bytes
    /// across all Rust, native, retained, GPU, and cross-process aliases.
    pub fn base_address_mut(&mut self) -> Option<*mut u8> {
        if self.flags.is_read_only() {
            None
        } else {
            self.buffer.base_address_raw()
        }
    }

    /// Get the base address of a specific plane.
    ///
    /// For multi-planar formats like YCbCr 4:2:0:
    /// - Plane 0: Y (luminance) data
    /// - Plane 1: `CbCr` (chrominance) data
    ///
    /// Returns `None` if the plane index is out of bounds. Dereferencing the
    /// returned pointer is unsafe because the lock does not establish Rust
    /// immutability.
    pub fn base_address_of_plane(&self, plane_index: usize) -> Option<*const u8> {
        self.buffer
            .base_address_of_plane_raw(plane_index)
            .map(<*mut u8>::cast_const)
    }

    /// Get the mutable base address of a specific plane.
    ///
    /// Returns `None` if this is a read-only lock or the plane index is out of
    /// bounds. Dereferencing requires unique access across every alias.
    pub fn base_address_of_plane_mut(&mut self, plane_index: usize) -> Option<*mut u8> {
        if self.flags.is_read_only() {
            return None;
        }
        self.buffer.base_address_of_plane_raw(plane_index)
    }

    /// Get the width of the buffer
    #[must_use]
    pub fn width(&self) -> usize {
        self.buffer.width()
    }

    /// Get the height of the buffer
    #[must_use]
    pub fn height(&self) -> usize {
        self.buffer.height()
    }

    /// Get bytes per row
    #[must_use]
    pub fn bytes_per_row(&self) -> usize {
        self.buffer.bytes_per_row()
    }

    /// Get the data size in bytes
    ///
    /// This provides API parity with `IOSurfaceLockGuard::data_size()`.
    #[must_use]
    pub fn data_size(&self) -> usize {
        self.buffer.data_size()
    }

    /// Get the number of planes
    #[must_use]
    pub fn plane_count(&self) -> usize {
        self.buffer.plane_count()
    }

    /// Get the width of a specific plane
    #[must_use]
    pub fn width_of_plane(&self, plane_index: usize) -> usize {
        self.buffer.width_of_plane(plane_index)
    }

    /// Get the height of a specific plane
    #[must_use]
    pub fn height_of_plane(&self, plane_index: usize) -> usize {
        self.buffer.height_of_plane(plane_index)
    }

    /// Get the bytes per row of a specific plane
    #[must_use]
    pub fn bytes_per_row_of_plane(&self, plane_index: usize) -> usize {
        self.buffer.bytes_per_row_of_plane(plane_index)
    }

    /// Get non-planar data as a byte slice.
    ///
    /// Returns `None` for planar buffers, missing base addresses, or lengths
    /// that cannot be represented by a Rust slice.
    ///
    /// # Safety
    ///
    /// For the returned reference's lifetime, the mapped range must remain
    /// allocated, initialized, and immovable, and no Rust or native alias may
    /// mutate or remap any byte in it. The caller must also prevent any manual
    /// unlock of this mapping.
    #[must_use]
    pub unsafe fn as_slice(&self) -> Option<&[u8]> {
        let ptr = self.base_address();
        let len = self.non_planar_data_len()?;
        if len == 0 {
            return Some(&[]);
        }
        if ptr.is_null() {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    /// Get non-planar data as a mutable byte slice.
    ///
    /// Returns `None` for read-only locks, planar buffers, missing base
    /// addresses, or lengths that cannot be represented by a Rust slice.
    ///
    /// # Safety
    ///
    /// For the returned reference's lifetime, this caller must have unique
    /// access to the full mapped range across every Rust, native, retained,
    /// GPU, and cross-process alias. The mapping must remain allocated,
    /// initialized, and locked.
    pub unsafe fn as_slice_mut(&mut self) -> Option<&mut [u8]> {
        let len = self.non_planar_data_len()?;
        if len == 0 {
            return Some(&mut []);
        }
        let ptr = self.base_address_mut()?;
        Some(unsafe { std::slice::from_raw_parts_mut(ptr, len) })
    }

    /// Get a slice of plane data.
    ///
    /// Returns the data for a specific plane as a byte slice.
    ///
    /// Returns `None` if the plane index is out of bounds or its byte length
    /// cannot be represented.
    ///
    /// # Safety
    ///
    /// For the returned reference's lifetime, the plane must remain allocated,
    /// initialized, locked, and immutable through every Rust and native alias.
    #[must_use]
    pub unsafe fn plane_data(&self, plane_index: usize) -> Option<&[u8]> {
        if !self.buffer.is_planar() || plane_index >= self.buffer.plane_count() {
            return None;
        }
        let base = self.base_address_of_plane(plane_index)?;
        let height = self.buffer.height_of_plane(plane_index);
        let bytes_per_row = self.buffer.bytes_per_row_of_plane(plane_index);
        let len = height.checked_mul(bytes_per_row)?;
        if isize::try_from(len).is_err() {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(base, len) })
    }

    /// Get a specific row from a plane as a slice.
    ///
    /// Returns `None` if the plane or row index is out of bounds.
    ///
    /// # Safety
    ///
    /// For the returned reference's lifetime, the row must remain allocated,
    /// initialized, locked, and immutable through every Rust and native alias.
    #[must_use]
    pub unsafe fn plane_row(&self, plane_index: usize, row_index: usize) -> Option<&[u8]> {
        if !self.buffer.is_planar() || plane_index >= self.buffer.plane_count() {
            return None;
        }
        let height = self.buffer.height_of_plane(plane_index);
        if row_index >= height {
            return None;
        }
        let base = self.base_address_of_plane(plane_index)?;
        let bytes_per_row = self.buffer.bytes_per_row_of_plane(plane_index);
        let plane_len = height.checked_mul(bytes_per_row)?;
        let offset = row_index.checked_mul(bytes_per_row)?;
        let end = offset.checked_add(bytes_per_row)?;
        if end > plane_len || isize::try_from(bytes_per_row).is_err() {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(base.add(offset), bytes_per_row) })
    }

    /// Get a specific non-planar row as a slice.
    ///
    /// Returns `None` if the row index is out of bounds.
    ///
    /// # Safety
    ///
    /// For the returned reference's lifetime, the row must remain allocated,
    /// initialized, locked, and immutable through every Rust and native alias.
    #[must_use]
    pub unsafe fn row(&self, row_index: usize) -> Option<&[u8]> {
        if row_index >= self.height() {
            return None;
        }
        let len = self.non_planar_data_len()?;
        let ptr = self.base_address();
        if ptr.is_null() {
            return None;
        }
        let bytes_per_row = self.bytes_per_row();
        let offset = row_index.checked_mul(bytes_per_row)?;
        let end = offset.checked_add(bytes_per_row)?;
        if end > len || isize::try_from(bytes_per_row).is_err() {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(ptr.add(offset), bytes_per_row) })
    }

    /// Access buffer with a standard `std::io::Cursor`
    ///
    /// Returns a cursor over the buffer data that implements `Read` and `Seek`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use apple_cf::cv::{CVPixelBuffer, CVPixelBufferLockFlags};
    /// use std::io::{Read, Seek, SeekFrom};
    ///
    /// fn read_buffer(buffer: &CVPixelBuffer) {
    ///     let guard = buffer.lock(CVPixelBufferLockFlags::READ_ONLY).unwrap();
    ///     // SAFETY: no alias can mutate or remap the bytes while the cursor lives.
    ///     let mut cursor = unsafe { guard.cursor() }.unwrap();
    ///
    ///     // Read first 4 bytes
    ///     let mut pixel = [0u8; 4];
    ///     cursor.read_exact(&mut pixel).unwrap();
    ///
    ///     // Seek to row 10
    ///     let offset = 10 * guard.bytes_per_row();
    ///     cursor.seek(SeekFrom::Start(offset as u64)).unwrap();
    /// }
    /// ```
    ///
    /// # Safety
    ///
    /// The same immutability and mapping guarantees as [`Self::as_slice`] must
    /// hold for the cursor's lifetime.
    #[must_use]
    pub unsafe fn cursor(&self) -> Option<io::Cursor<&[u8]>> {
        unsafe { self.as_slice() }.map(io::Cursor::new)
    }

    /// Get raw pointer to buffer data
    #[must_use]
    pub fn as_ptr(&self) -> *const u8 {
        self.base_address()
    }

    /// Get mutable raw pointer to buffer data (only valid for read-write locks)
    ///
    /// Returns `None` if this is a read-only lock.
    pub fn as_mut_ptr(&mut self) -> Option<*mut u8> {
        self.base_address_mut()
    }

    /// Check if this is a read-only lock
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        self.flags.is_read_only()
    }

    /// Get the lock options
    #[must_use]
    pub const fn options(&self) -> CVPixelBufferLockFlags {
        self.flags
    }

    /// Get the pixel format
    #[must_use]
    pub fn pixel_format(&self) -> u32 {
        self.buffer.pixel_format()
    }
}

impl Drop for CVPixelBufferLockGuard<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.buffer.unlock_raw(self.flags) };
    }
}

impl std::fmt::Debug for CVPixelBufferLockGuard<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CVPixelBufferLockGuard")
            .field("flags", &self.flags)
            .field("buffer_size", &(self.buffer.width(), self.buffer.height()))
            .finish()
    }
}

crate::utils::retained::cf_retained!(
    CVPixelBuffer,
    retain = ffi::cv_pixel_buffer_retain,
    release = ffi::cv_pixel_buffer_release,
);

// SAFETY: `CVPixelBufferRef` is a Core Foundation type whose retain/release
// operations are thread-safe. Our wrapper only holds the opaque pointer, and
// native mapping operations are thread-safe. Byte dereferencing has a separate
// unsafe contract because a lock does not establish Rust aliasing guarantees.
unsafe impl Send for CVPixelBuffer {}
unsafe impl Sync for CVPixelBuffer {}

impl fmt::Display for CVPixelBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CVPixelBuffer({}x{}, format: 0x{:08X})",
            self.width(),
            self.height(),
            self.pixel_format()
        )
    }
}

/// Flags controlling which unused buffers a pool flushes.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CVPixelBufferPoolFlushFlags(u64);

impl CVPixelBufferPoolFlushFlags {
    /// Flush only buffers that have aged out.
    pub const NONE: Self = Self(0);

    /// Flush every unused buffer regardless of age.
    pub const EXCESS_BUFFERS: Self = Self(1);

    /// Create flags from raw `CVOptionFlags` bits.
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Return the raw `CVOptionFlags` bits.
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }
}

impl std::ops::BitOr for CVPixelBufferPoolFlushFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for CVPixelBufferPoolFlushFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl From<CVPixelBufferPoolFlushFlags> for u64 {
    fn from(flags: CVPixelBufferPoolFlushFlags) -> Self {
        flags.bits()
    }
}

const CV_RETURN_WOULD_EXCEED_ALLOCATION_THRESHOLD: i32 = -6689;
const PARAM_ERR: i32 = -50;

#[derive(Debug)]
struct CVPixelBufferPoolPolicy {
    max_buffers: Option<usize>,
    allocation_attributes: Option<CFDictionary>,
}

// SAFETY: The policy is immutable after construction. Its cached dictionary is
// immutable and only shared for Core Foundation reads and retain/release.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for CVPixelBufferPoolPolicy {}
unsafe impl Sync for CVPixelBufferPoolPolicy {}

impl CVPixelBufferPoolPolicy {
    const fn unlimited() -> Self {
        Self {
            max_buffers: None,
            allocation_attributes: None,
        }
    }

    fn new(max_buffers: usize) -> Result<Self, i32> {
        if max_buffers == 0 {
            return Ok(Self::unlimited());
        }

        let threshold = i64::try_from(max_buffers).map_err(|_| PARAM_ERR)?;
        let key = retained_cf_string(
            unsafe { raw::kCVPixelBufferPoolAllocationThresholdKey },
            "kCVPixelBufferPoolAllocationThresholdKey",
        );
        let value = CFNumber::from_i64(threshold);
        let attributes = CFDictionary::from_pairs(&[(&key, &value)]);

        Ok(Self {
            max_buffers: Some(max_buffers),
            allocation_attributes: Some(attributes),
        })
    }

    const fn max_buffers(&self) -> usize {
        match self.max_buffers {
            Some(max_buffers) => max_buffers,
            None => 0,
        }
    }
}

fn retained_cf_string(ptr: raw::CFStringRef, symbol: &'static str) -> CFString {
    unsafe { CFString::from_raw_borrowed(ptr.cast_mut().cast()) }
        .unwrap_or_else(|| panic!("{symbol} was NULL"))
}

fn pool_pixel_buffer_attributes(
    width: usize,
    height: usize,
    pixel_format: u32,
) -> Result<CFDictionary, i32> {
    let width = u64::try_from(width).map_err(|_| PARAM_ERR)?;
    let height = u64::try_from(height).map_err(|_| PARAM_ERR)?;
    let width_key = retained_cf_string(
        unsafe { raw::kCVPixelBufferWidthKey },
        "kCVPixelBufferWidthKey",
    );
    let height_key = retained_cf_string(
        unsafe { raw::kCVPixelBufferHeightKey },
        "kCVPixelBufferHeightKey",
    );
    let pixel_format_key = retained_cf_string(
        unsafe { raw::kCVPixelBufferPixelFormatTypeKey },
        "kCVPixelBufferPixelFormatTypeKey",
    );
    let io_surface_key = retained_cf_string(
        unsafe { raw::kCVPixelBufferIOSurfacePropertiesKey },
        "kCVPixelBufferIOSurfacePropertiesKey",
    );
    let width_value = CFNumber::from_u64(width);
    let height_value = CFNumber::from_u64(height);
    let pixel_format_value = CFNumber::from_u64(u64::from(pixel_format));
    let io_surface_properties = CFDictionary::from_pairs(&[]);
    let pairs: [(&dyn AsCFType, &dyn AsCFType); 4] = [
        (&width_key, &width_value),
        (&height_key, &height_value),
        (&pixel_format_key, &pixel_format_value),
        (&io_surface_key, &io_surface_properties),
    ];
    Ok(CFDictionary::from_pairs(&pairs))
}

/// Opaque handle to a native `CVPixelBufferPoolRef`.
pub struct CVPixelBufferPool {
    ptr: *mut std::ffi::c_void,
    policy: Arc<CVPixelBufferPoolPolicy>,
}

// SAFETY: Core Video pool retain/release, allocation, and flush operations are
// thread-safe. The wrapper's shared allocation policy is immutable and
// thread-safe.
unsafe impl Send for CVPixelBufferPool {}
unsafe impl Sync for CVPixelBufferPool {}

impl PartialEq for CVPixelBufferPool {
    fn eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }
}

impl Eq for CVPixelBufferPool {}

impl std::hash::Hash for CVPixelBufferPool {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        unsafe { ffi::cf_type_hash(self.ptr) }.hash(state);
    }
}

impl CVPixelBufferPool {
    /// Adopts a +1 retained native pool with no wrapper allocation threshold.
    ///
    /// # Safety
    ///
    /// A non-null `ptr` must be a live `CVPixelBufferPoolRef` carrying one
    /// retain transferred to this wrapper. The caller must not release or
    /// separately adopt that transferred retain.
    pub unsafe fn from_raw(ptr: *mut std::ffi::c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self {
                ptr,
                policy: Arc::new(CVPixelBufferPoolPolicy::unlimited()),
            })
        }
    }

    /// Adopts a +1 retained native pool and applies `max_buffers` to wrapper allocations.
    ///
    /// A zero threshold means unlimited. Native callers using [`Self::as_ptr`]
    /// can bypass this wrapper policy.
    ///
    /// # Safety
    ///
    /// A non-null `ptr` must be a live `CVPixelBufferPoolRef` carrying one
    /// retain transferred to this wrapper. On an error, ownership remains with
    /// the caller.
    ///
    /// # Errors
    ///
    /// Returns `paramErr` if `max_buffers` cannot be represented by Core Video.
    pub unsafe fn from_raw_with_max_buffers(
        ptr: *mut std::ffi::c_void,
        max_buffers: usize,
    ) -> Result<Option<Self>, i32> {
        if ptr.is_null() {
            return Ok(None);
        }
        let policy = Arc::new(CVPixelBufferPoolPolicy::new(max_buffers)?);
        Ok(Some(Self { ptr, policy }))
    }

    /// Retains a +0 borrowed native pool with no wrapper allocation threshold.
    ///
    /// # Safety
    ///
    /// A non-null `ptr` must be a live `CVPixelBufferPoolRef` for the duration
    /// of the retain call.
    #[must_use]
    pub unsafe fn from_raw_borrowed(ptr: *mut std::ffi::c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            let retained = unsafe { raw::CVPixelBufferPoolRetain(ptr.cast()) };
            unsafe { Self::from_raw(retained.cast()) }
        }
    }

    /// Retains a +0 borrowed native pool and applies `max_buffers` to wrapper allocations.
    ///
    /// # Safety
    ///
    /// A non-null `ptr` must be a live `CVPixelBufferPoolRef` for the duration
    /// of the retain call.
    ///
    /// # Errors
    ///
    /// Returns `paramErr` if `max_buffers` cannot be represented by Core Video.
    pub unsafe fn from_raw_borrowed_with_max_buffers(
        ptr: *mut std::ffi::c_void,
        max_buffers: usize,
    ) -> Result<Option<Self>, i32> {
        if ptr.is_null() {
            return Ok(None);
        }
        let policy = Arc::new(CVPixelBufferPoolPolicy::new(max_buffers)?);
        let retained = unsafe { raw::CVPixelBufferPoolRetain(ptr.cast()) };
        Ok(Some(Self {
            ptr: retained.cast(),
            policy,
        }))
    }

    /// Wraps a raw `CVPixelBufferPoolRef` by taking ownership without retaining it.
    ///
    /// # Safety
    /// `ptr` must be a non-null, live `CVPixelBufferPoolRef` carrying one retain
    /// transferred to this wrapper.
    pub unsafe fn from_ptr(ptr: *mut std::ffi::c_void) -> Self {
        Self {
            ptr,
            policy: Arc::new(CVPixelBufferPoolPolicy::unlimited()),
        }
    }

    /// Borrows the raw +0 native pool pointer while `self` remains alive.
    ///
    /// Allocations performed directly through this pointer bypass the wrapper's
    /// configured [`Self::max_buffers`] threshold.
    #[must_use]
    pub const fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.ptr
    }

    /// Wrapper-enforced allocation threshold, or zero when unlimited.
    #[must_use]
    pub fn max_buffers(&self) -> usize {
        self.policy.max_buffers()
    }

    /// Create a new pixel buffer pool
    ///
    /// # Arguments
    ///
    /// * `width` - Width of pixel buffers in the pool
    /// * `height` - Height of pixel buffers in the pool
    /// * `pixel_format` - Pixel format type
    /// * `max_buffers` - Maximum number of buffers in the pool (0 for unlimited)
    ///
    /// # Errors
    ///
    /// Returns a Core Video error code if the pool creation fails.
    pub fn create(
        width: usize,
        height: usize,
        pixel_format: u32,
        max_buffers: usize,
    ) -> Result<Self, i32> {
        let policy = Arc::new(CVPixelBufferPoolPolicy::new(max_buffers)?);
        let pool_attributes = CFDictionary::from_pairs(&[]);
        let pixel_buffer_attributes = pool_pixel_buffer_attributes(width, height, pixel_format)?;
        let mut pool_ptr: raw::CVPixelBufferPoolRef = std::ptr::null_mut();
        unsafe {
            let status = raw::CVPixelBufferPoolCreate(
                std::ptr::null(),
                pool_attributes.as_ptr().cast(),
                pixel_buffer_attributes.as_ptr().cast(),
                &raw mut pool_ptr,
            );

            if status == 0 && !pool_ptr.is_null() {
                Ok(Self {
                    ptr: pool_ptr.cast(),
                    policy,
                })
            } else {
                Err(status)
            }
        }
    }

    fn create_pixel_buffer_with_dictionary(
        &self,
        auxiliary_attributes: Option<&CFDictionary>,
    ) -> Result<CVPixelBuffer, i32> {
        let mut pixel_buffer_ptr: raw::CVPixelBufferRef = std::ptr::null_mut();
        let status = unsafe {
            if let Some(attributes) = auxiliary_attributes {
                raw::CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(
                    std::ptr::null(),
                    self.ptr.cast(),
                    attributes.as_ptr().cast(),
                    &raw mut pixel_buffer_ptr,
                )
            } else {
                raw::CVPixelBufferPoolCreatePixelBuffer(
                    std::ptr::null(),
                    self.ptr.cast(),
                    &raw mut pixel_buffer_ptr,
                )
            }
        };

        if status == 0 && !pixel_buffer_ptr.is_null() {
            unsafe { CVPixelBuffer::from_raw(pixel_buffer_ptr.cast()) }.ok_or(status)
        } else {
            Err(status)
        }
    }

    /// Create a pixel buffer while enforcing the configured allocation threshold.
    ///
    /// # Errors
    ///
    /// Returns a Core Video error code if the buffer creation fails.
    pub fn create_pixel_buffer(&self) -> Result<CVPixelBuffer, i32> {
        self.create_pixel_buffer_with_dictionary(self.policy.allocation_attributes.as_ref())
    }

    /// Flush buffers that have aged out of the pool.
    pub fn flush(&self) {
        self.flush_with_flags(CVPixelBufferPoolFlushFlags::NONE);
    }

    /// Flush unused buffers according to native Core Video flags.
    pub fn flush_with_flags(&self, flags: CVPixelBufferPoolFlushFlags) {
        unsafe { raw::CVPixelBufferPoolFlush(self.ptr.cast(), flags.bits()) };
    }

    /// Flush every unused buffer regardless of age.
    pub fn flush_excess_buffers(&self) {
        self.flush_with_flags(CVPixelBufferPoolFlushFlags::EXCESS_BUFFERS);
    }

    /// Get the Core Foundation type ID for `CVPixelBufferPool`
    #[must_use]
    pub fn type_id() -> usize {
        #[allow(clippy::cast_possible_truncation)]
        {
            unsafe { raw::CVPixelBufferPoolGetTypeID() as usize }
        }
    }

    /// Create a pixel buffer from the pool with per-call auxiliary attributes.
    ///
    /// String keys become `CFString` keys and values become `CFNumber` values.
    /// A per-call allocation threshold can tighten but not loosen the
    /// threshold configured when the wrapper was created.
    ///
    /// # Errors
    ///
    /// Returns `paramErr` for an attribute key containing a NUL byte or an
    /// unrepresentable threshold, otherwise returns the Core Video allocation
    /// status.
    pub fn create_pixel_buffer_with_aux_attributes(
        &self,
        aux_attributes: Option<&HashMap<String, u32>>,
    ) -> Result<CVPixelBuffer, i32> {
        let Some(aux_attributes) = aux_attributes.filter(|attributes| !attributes.is_empty())
        else {
            return self.create_pixel_buffer();
        };

        let threshold_key = retained_cf_string(
            unsafe { raw::kCVPixelBufferPoolAllocationThresholdKey },
            "kCVPixelBufferPoolAllocationThresholdKey",
        );
        let mut keys = Vec::with_capacity(aux_attributes.len() + 1);
        let mut values = Vec::with_capacity(aux_attributes.len() + 1);
        let mut requested_threshold = None;

        for (key, value) in aux_attributes {
            if key.as_bytes().contains(&0) {
                return Err(PARAM_ERR);
            }
            let key = CFString::new(key);
            if key == threshold_key {
                requested_threshold = Some(usize::try_from(*value).map_err(|_| PARAM_ERR)?);
            } else {
                keys.push(key);
                values.push(CFNumber::from_u64(u64::from(*value)));
            }
        }

        let effective_threshold = match (self.policy.max_buffers, requested_threshold) {
            (Some(configured), Some(requested)) => Some(configured.min(requested)),
            (Some(configured), None) => Some(configured),
            (None, requested) => requested,
        };

        if let Some(threshold) = effective_threshold {
            let threshold = i64::try_from(threshold).map_err(|_| PARAM_ERR)?;
            keys.push(threshold_key);
            values.push(CFNumber::from_i64(threshold));
        }

        let pairs: Vec<(&dyn AsCFType, &dyn AsCFType)> = keys
            .iter()
            .zip(&values)
            .map(|(key, value)| (key as &dyn AsCFType, value as &dyn AsCFType))
            .collect();
        let attributes = CFDictionary::from_pairs(&pairs);
        self.create_pixel_buffer_with_dictionary(Some(&attributes))
    }

    /// Try to create a pixel buffer without exceeding the allocation threshold.
    ///
    /// Only `kCVReturnWouldExceedAllocationThreshold` maps to `Ok(None)`;
    /// every other Core Video error is preserved.
    ///
    /// # Errors
    ///
    /// Returns any Core Video allocation error other than threshold exhaustion.
    pub fn try_create_pixel_buffer(&self) -> Result<Option<CVPixelBuffer>, i32> {
        match self.create_pixel_buffer() {
            Ok(buffer) => Ok(Some(buffer)),
            Err(CV_RETURN_WOULD_EXCEED_ALLOCATION_THRESHOLD) => Ok(None),
            Err(status) => Err(status),
        }
    }

    /// Copy the pool attributes into an independently owned dictionary.
    #[must_use]
    pub fn attributes(&self) -> Option<CFDictionary> {
        let ptr = unsafe { raw::CVPixelBufferPoolGetAttributes(self.ptr.cast()) };
        unsafe { CFDictionary::from_raw_borrowed(ptr.cast_mut().cast()) }
    }

    /// Copy the pixel-buffer attributes into an independently owned dictionary.
    #[must_use]
    pub fn pixel_buffer_attributes(&self) -> Option<CFDictionary> {
        let ptr = unsafe { raw::CVPixelBufferPoolGetPixelBufferAttributes(self.ptr.cast()) };
        unsafe { CFDictionary::from_raw_borrowed(ptr.cast_mut().cast()) }
    }
}

impl Clone for CVPixelBufferPool {
    fn clone(&self) -> Self {
        let ptr = unsafe { raw::CVPixelBufferPoolRetain(self.ptr.cast()) };
        Self {
            ptr: ptr.cast(),
            policy: Arc::clone(&self.policy),
        }
    }
}

impl Drop for CVPixelBufferPool {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { raw::CVPixelBufferPoolRelease(self.ptr.cast()) };
        }
    }
}

impl fmt::Debug for CVPixelBufferPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CVPixelBufferPool")
            .field("ptr", &self.ptr)
            .field("max_buffers", &self.max_buffers())
            .finish_non_exhaustive()
    }
}

impl fmt::Display for CVPixelBufferPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CVPixelBufferPool")
    }
}

/// Extension trait for `io::Cursor` to add pixel buffer specific operations
pub trait PixelBufferCursorExt {
    /// Seek to a specific pixel coordinate (x, y)
    ///
    /// Assumes 4 bytes per pixel (BGRA format).
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the seek operation fails.
    fn seek_to_pixel(&mut self, x: usize, y: usize, bytes_per_row: usize) -> io::Result<u64>;

    /// Read a single pixel (4 bytes: BGRA)
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the read operation fails.
    fn read_pixel(&mut self) -> io::Result<[u8; 4]>;
}

impl<T: AsRef<[u8]>> PixelBufferCursorExt for io::Cursor<T> {
    fn seek_to_pixel(&mut self, x: usize, y: usize, bytes_per_row: usize) -> io::Result<u64> {
        let pos = y * bytes_per_row + x * 4; // 4 bytes per pixel (BGRA)
        self.seek(SeekFrom::Start(pos as u64))
    }

    fn read_pixel(&mut self) -> io::Result<[u8; 4]> {
        let mut pixel = [0u8; 4];
        self.read_exact(&mut pixel)?;
        Ok(pixel)
    }
}
