// CoreVideo Bridge - CVPixelBuffer, CVPixelBufferPool

import CoreMedia
import CoreVideo
import Foundation
import IOSurface

// MARK: - CVPixelBuffer Bridge

@_cdecl("cv_pixel_buffer_get_width")
public func cv_pixel_buffer_get_width(_ pixelBuffer: UnsafeMutableRawPointer) -> Int {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferGetWidth(buffer)
}

@_cdecl("cv_pixel_buffer_get_height")
public func cv_pixel_buffer_get_height(_ pixelBuffer: UnsafeMutableRawPointer) -> Int {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferGetHeight(buffer)
}

@_cdecl("cv_pixel_buffer_get_pixel_format_type")
public func cv_pixel_buffer_get_pixel_format_type(_ pixelBuffer: UnsafeMutableRawPointer) -> UInt32 {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferGetPixelFormatType(buffer)
}

@_cdecl("cv_pixel_buffer_get_bytes_per_row")
public func cv_pixel_buffer_get_bytes_per_row(_ pixelBuffer: UnsafeMutableRawPointer) -> Int {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferGetBytesPerRow(buffer)
}

@_cdecl("cv_pixel_buffer_lock_base_address")
public func cv_pixel_buffer_lock_base_address(_ pixelBuffer: UnsafeMutableRawPointer, _ flags: UInt64) -> Int32 {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags(rawValue: flags))
}

@_cdecl("cv_pixel_buffer_unlock_base_address")
public func cv_pixel_buffer_unlock_base_address(_ pixelBuffer: UnsafeMutableRawPointer, _ flags: UInt64) -> Int32 {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags(rawValue: flags))
}

@_cdecl("cv_pixel_buffer_get_base_address")
public func cv_pixel_buffer_get_base_address(_ pixelBuffer: UnsafeMutableRawPointer) -> UnsafeMutableRawPointer? {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferGetBaseAddress(buffer)
}

@_cdecl("cv_pixel_buffer_get_io_surface")
public func cv_pixel_buffer_get_io_surface(_ pixelBuffer: UnsafeMutableRawPointer) -> UnsafeMutableRawPointer? {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    guard let ioSurface = CVPixelBufferGetIOSurface(buffer) else {
        return nil
    }
    return Unmanaged.passRetained(ioSurface.takeUnretainedValue()).toOpaque()
}

// Compatibility alias (deprecated - use cv_pixel_buffer_get_io_surface)
@_cdecl("cv_pixel_buffer_get_iosurface")
public func cv_pixel_buffer_get_iosurface(_ pixelBuffer: UnsafeMutableRawPointer) -> UnsafeMutableRawPointer? {
    cv_pixel_buffer_get_io_surface(pixelBuffer)
}

@_cdecl("cv_pixel_buffer_is_backed_by_iosurface")
public func cv_pixel_buffer_is_backed_by_iosurface(_ pixelBuffer: UnsafeMutableRawPointer) -> Bool {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferGetIOSurface(buffer) != nil
}

@_cdecl("cv_pixel_buffer_release")
public func cv_pixel_buffer_release(_ pixelBuffer: UnsafeMutableRawPointer) {
    Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).release()
}

@_cdecl("cv_pixel_buffer_retain")
public func cv_pixel_buffer_retain(_ pixelBuffer: UnsafeMutableRawPointer) -> UnsafeMutableRawPointer {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return Unmanaged.passRetained(buffer).toOpaque()
}

// MARK: - CVPixelBuffer Creation

@_cdecl("cv_pixel_buffer_create")
public func cv_pixel_buffer_create(
    _ width: Int,
    _ height: Int,
    _ pixelFormatType: UInt32,
    _ pixelBufferOut: UnsafeMutablePointer<UnsafeMutableRawPointer?>
) -> Int32 {
    var pixelBuffer: CVPixelBuffer?
    let status = CVPixelBufferCreate(
        kCFAllocatorDefault,
        width,
        height,
        OSType(pixelFormatType),
        nil,
        &pixelBuffer
    )

    if status == kCVReturnSuccess, let buffer = pixelBuffer {
        pixelBufferOut.pointee = Unmanaged.passRetained(buffer).toOpaque()
    } else {
        pixelBufferOut.pointee = nil
    }

    return status
}

@_cdecl("cv_pixel_buffer_create_with_bytes")
public func cv_pixel_buffer_create_with_bytes(
    _ width: Int,
    _ height: Int,
    _ pixelFormatType: UInt32,
    _ baseAddress: UnsafeMutableRawPointer,
    _ bytesPerRow: Int,
    _ pixelBufferOut: UnsafeMutablePointer<UnsafeMutableRawPointer?>
) -> Int32 {
    var pixelBuffer: CVPixelBuffer?
    let status = CVPixelBufferCreateWithBytes(
        kCFAllocatorDefault,
        width,
        height,
        OSType(pixelFormatType),
        baseAddress,
        bytesPerRow,
        nil,
        nil,
        nil,
        &pixelBuffer
    )

    if status == kCVReturnSuccess, let buffer = pixelBuffer {
        pixelBufferOut.pointee = Unmanaged.passRetained(buffer).toOpaque()
    } else {
        pixelBufferOut.pointee = nil
    }

    return status
}

@_cdecl("cv_pixel_buffer_create_with_planar_bytes")
public func cv_pixel_buffer_create_with_planar_bytes(
    _ width: Int,
    _ height: Int,
    _ pixelFormatType: UInt32,
    _ numPlanes: Int,
    _ planeBaseAddresses: UnsafePointer<UnsafeMutableRawPointer?>,
    _ planeWidths: UnsafePointer<Int>,
    _ planeHeights: UnsafePointer<Int>,
    _ planeBytesPerRow: UnsafePointer<Int>,
    _ pixelBufferOut: UnsafeMutablePointer<UnsafeMutableRawPointer?>
) -> Int32 {
    var pixelBuffer: CVPixelBuffer?
    guard numPlanes >= 0 else {
        pixelBufferOut.pointee = nil
        return kCVReturnInvalidArgument
    }

    var planeBaseAddressesCopy = Array(UnsafeBufferPointer(start: planeBaseAddresses, count: numPlanes))
    var planeWidthsCopy = Array(UnsafeBufferPointer(start: planeWidths, count: numPlanes))
    var planeHeightsCopy = Array(UnsafeBufferPointer(start: planeHeights, count: numPlanes))
    var planeBytesPerRowCopy = Array(UnsafeBufferPointer(start: planeBytesPerRow, count: numPlanes))

    let status = CVPixelBufferCreateWithPlanarBytes(
        kCFAllocatorDefault,
        width,
        height,
        OSType(pixelFormatType),
        nil,
        0,
        numPlanes,
        &planeBaseAddressesCopy,
        &planeWidthsCopy,
        &planeHeightsCopy,
        &planeBytesPerRowCopy,
        nil,
        nil,
        nil,
        &pixelBuffer
    )

    if status == kCVReturnSuccess, let buffer = pixelBuffer {
        pixelBufferOut.pointee = Unmanaged.passRetained(buffer).toOpaque()
    } else {
        pixelBufferOut.pointee = nil
    }

    return status
}

@_cdecl("cv_pixel_buffer_create_with_io_surface")
public func cv_pixel_buffer_create_with_io_surface(
    _ ioSurface: UnsafeMutableRawPointer,
    _ pixelBufferOut: UnsafeMutablePointer<UnsafeMutableRawPointer?>
) -> Int32 {
    let surface = Unmanaged<IOSurface>.fromOpaque(ioSurface).takeUnretainedValue()
    var pixelBuffer: Unmanaged<CVPixelBuffer>?

    let status = CVPixelBufferCreateWithIOSurface(
        kCFAllocatorDefault,
        surface,
        nil,
        &pixelBuffer
    )

    if status == kCVReturnSuccess, let buffer = pixelBuffer {
        pixelBufferOut.pointee = buffer.toOpaque()
    } else {
        pixelBufferOut.pointee = nil
    }

    return status
}

@_cdecl("cv_pixel_buffer_get_type_id")
public func cv_pixel_buffer_get_type_id() -> Int {
    Int(CVPixelBufferGetTypeID())
}

@_cdecl("cv_pixel_buffer_fill_extended_pixels")
public func cv_pixel_buffer_fill_extended_pixels(_ pixelBuffer: UnsafeMutableRawPointer) -> Int32 {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferFillExtendedPixels(buffer)
}

@_cdecl("cv_pixel_buffer_get_data_size")
public func cv_pixel_buffer_get_data_size(_ pixelBuffer: UnsafeMutableRawPointer) -> Int {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferGetDataSize(buffer)
}

@_cdecl("cv_pixel_buffer_is_planar")
public func cv_pixel_buffer_is_planar(_ pixelBuffer: UnsafeMutableRawPointer) -> Bool {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferIsPlanar(buffer)
}

@_cdecl("cv_pixel_buffer_get_plane_count")
public func cv_pixel_buffer_get_plane_count(_ pixelBuffer: UnsafeMutableRawPointer) -> Int {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferGetPlaneCount(buffer)
}

@_cdecl("cv_pixel_buffer_get_width_of_plane")
public func cv_pixel_buffer_get_width_of_plane(_ pixelBuffer: UnsafeMutableRawPointer, _ planeIndex: Int) -> Int {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferGetWidthOfPlane(buffer, planeIndex)
}

@_cdecl("cv_pixel_buffer_get_height_of_plane")
public func cv_pixel_buffer_get_height_of_plane(_ pixelBuffer: UnsafeMutableRawPointer, _ planeIndex: Int) -> Int {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferGetHeightOfPlane(buffer, planeIndex)
}

@_cdecl("cv_pixel_buffer_get_base_address_of_plane")
public func cv_pixel_buffer_get_base_address_of_plane(_ pixelBuffer: UnsafeMutableRawPointer, _ planeIndex: Int) -> UnsafeMutableRawPointer? {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferGetBaseAddressOfPlane(buffer, planeIndex)
}

@_cdecl("cv_pixel_buffer_get_bytes_per_row_of_plane")
public func cv_pixel_buffer_get_bytes_per_row_of_plane(_ pixelBuffer: UnsafeMutableRawPointer, _ planeIndex: Int) -> Int {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return CVPixelBufferGetBytesPerRowOfPlane(buffer, planeIndex)
}

@_cdecl("cv_pixel_buffer_get_extended_pixels")
public func cv_pixel_buffer_get_extended_pixels(
    _ pixelBuffer: UnsafeMutableRawPointer,
    _ extraColumnsOnLeft: UnsafeMutablePointer<Int>,
    _ extraColumnsOnRight: UnsafeMutablePointer<Int>,
    _ extraRowsOnTop: UnsafeMutablePointer<Int>,
    _ extraRowsOnBottom: UnsafeMutablePointer<Int>
) {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    CVPixelBufferGetExtendedPixels(buffer,
                                   extraColumnsOnLeft,
                                   extraColumnsOnRight,
                                   extraRowsOnTop,
                                   extraRowsOnBottom)
}

// MARK: - Hash Functions

@_cdecl("cv_pixel_buffer_hash")
public func cv_pixel_buffer_hash(_ pixelBuffer: UnsafeMutableRawPointer) -> Int {
    let buffer = Unmanaged<CVPixelBuffer>.fromOpaque(pixelBuffer).takeUnretainedValue()
    return buffer.hashValue
}
