#![cfg(feature = "iosurface")]

use apple_cf::iosurface::{IOSurface, IOSurfaceLockOptions, PlaneProperties};

const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const YUV_420V: u32 = u32::from_be_bytes(*b"420v");

fn biplanar_surface(width: usize, height: usize) -> IOSurface {
    let luma_size = width * height;
    let chroma_size = width * (height / 2);
    let planes = [
        PlaneProperties {
            width,
            height,
            bytes_per_row: width,
            bytes_per_element: 1,
            offset: 0,
            size: luma_size,
        },
        PlaneProperties {
            width: width / 2,
            height: height / 2,
            bytes_per_row: width,
            bytes_per_element: 2,
            offset: luma_size,
            size: chroma_size,
        },
    ];
    IOSurface::create_with_properties(
        width,
        height,
        YUV_420V,
        1,
        width,
        luma_size + chroma_size,
        Some(&planes),
    )
    .expect("biplanar surface")
}

#[test]
fn create_rejects_overflowing_dimensions() {
    assert!(IOSurface::create(1 << 40, 1 << 40, BGRA, 4).is_none());
    assert!(IOSurface::create(usize::MAX, 1, BGRA, 4).is_none());
    assert!(IOSurface::create(1, usize::MAX, BGRA, 4).is_none());
    let surface = IOSurface::create(16, 8, BGRA, 4).expect("surface");
    assert_eq!((surface.width(), surface.height()), (16, 8));
}

#[test]
fn plane_views_stay_inside_the_surface_allocation() {
    let surface = biplanar_surface(64, 32);
    assert_eq!(surface.plane_count(), 2);
    let guard = surface.lock(IOSurfaceLockOptions::READ_ONLY).expect("lock");
    let base = guard.base_address() as usize;
    let alloc_size = guard.alloc_size();
    for plane in 0..2 {
        let data = unsafe { guard.plane_data(plane) }.expect("plane data");
        assert_eq!(
            data.len(),
            surface.height_of_plane(plane) * surface.bytes_per_row_of_plane(plane)
        );
        let offset = data.as_ptr() as usize - base;
        assert!(offset + data.len() <= alloc_size);
        let last_row = surface.height_of_plane(plane) - 1;
        let row = unsafe { guard.plane_row(plane, last_row) }.expect("last row");
        assert_eq!(row.len(), surface.bytes_per_row_of_plane(plane));
        assert!(row.as_ptr() as usize - base + row.len() <= alloc_size);
        assert!(unsafe { guard.plane_row(plane, last_row + 1) }.is_none());
    }
    assert!(unsafe { guard.plane_data(2) }.is_none());
    assert!(unsafe { guard.plane_row(2, 0) }.is_none());
}

#[test]
fn single_plane_surfaces_expose_no_plane_views() {
    let surface = IOSurface::create(8, 8, BGRA, 4).expect("surface");
    let guard = surface.lock(IOSurfaceLockOptions::READ_ONLY).expect("lock");
    assert!(unsafe { guard.plane_data(0) }.is_none());
    assert!(unsafe { guard.plane_row(0, 0) }.is_none());
    assert_eq!(
        unsafe { guard.as_slice() }.expect("bytes").len(),
        guard.alloc_size()
    );
}
