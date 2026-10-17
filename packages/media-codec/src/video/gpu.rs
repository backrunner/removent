//! Native CVPixelBuffer -> Metal Lanczos scaling -> CVPixelBuffer. No CPU
//! mapping, readback, upload or pixel copy occurs on this path.
// objc 0.2's selector macros still emit their historical cargo-clippy cfg.
#![allow(unexpected_cfgs)]
use super::*;
use apple_cf::{cf::CFType, cv::CVPixelBuffer};
use metal::foreign_types::{ForeignType, ForeignTypeRef};
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};

#[link(name = "MetalPerformanceShaders", kind = "framework")]
unsafe extern "C" {}
#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVMetalTextureCacheCreate(
        allocator: *const c_void,
        attributes: *const c_void,
        device: *mut c_void,
        texture_attributes: *const c_void,
        cache: *mut *mut c_void,
    ) -> i32;
    fn CVMetalTextureCacheCreateTextureFromImage(
        allocator: *const c_void,
        cache: *mut c_void,
        source: *mut c_void,
        attributes: *const c_void,
        format: u64,
        width: usize,
        height: usize,
        plane: usize,
        texture: *mut *mut c_void,
    ) -> i32;
    fn CVMetalTextureGetTexture(texture: *mut c_void) -> *mut metal::MTLTexture;
}

pub(super) fn pixel_buffer(width: usize, height: usize) -> Result<CVPixelBuffer, VideoError> {
    let stride = width
        .checked_mul(4)
        .and_then(|n| n.checked_next_multiple_of(64))
        .ok_or_else(|| VideoError::Gpu("pixel stride overflow".into()))?;
    let size = stride
        .checked_mul(height)
        .ok_or_else(|| VideoError::Gpu("pixel allocation overflow".into()))?;
    let surface =
        IOSurface::create_with_properties(width, height, BGRA_FOURCC, 4, stride, size, None)
            .ok_or_else(|| VideoError::Gpu("IOSurface allocation failed".into()))?;
    from_surface(&surface)
}

pub(super) fn from_surface(surface: &IOSurface) -> Result<CVPixelBuffer, VideoError> {
    let mut buffer = std::ptr::null_mut();
    // SAFETY: the surface lives through creation; CoreVideo retains it.
    let status = unsafe {
        videotoolbox::ffi::CVPixelBufferCreateWithIOSurface(
            videotoolbox::ffi::kCFAllocatorDefault,
            surface.as_ptr().cast(),
            std::ptr::null(),
            &mut buffer,
        )
    };
    if status != 0 {
        return Err(VideoError::Gpu(format!("pixel buffer creation: {status}")));
    }
    unsafe { CVPixelBuffer::from_raw(buffer.cast()) }
        .ok_or_else(|| VideoError::Gpu("null pixel buffer".into()))
}

pub(super) struct MetalScaler {
    queue: metal::CommandQueue,
    cache: apple_cf::cv::CVMetalTextureCache,
    lanczos: objc::rc::StrongPtr,
    output: CVPixelBuffer,
}

// SAFETY: the owned Metal objects may move across threads. The scaler and its
// reusable destination are accessed exclusively through &mut self, and each
// command completes before the caller can submit the destination to VT.
unsafe impl Send for MetalScaler {}

impl MetalScaler {
    pub(super) fn new(width: usize, height: usize) -> Result<Self, VideoError> {
        let device = metal::Device::system_default()
            .ok_or_else(|| VideoError::Gpu("no Metal device".into()))?;
        let queue = device.new_command_queue();
        let mut cache = std::ptr::null_mut();
        let status = unsafe {
            CVMetalTextureCacheCreate(
                std::ptr::null(),
                std::ptr::null(),
                device.as_ptr().cast(),
                std::ptr::null(),
                &mut cache,
            )
        };
        if status != 0 {
            return Err(VideoError::Gpu(format!("Metal texture cache: {status}")));
        }
        let cache = unsafe { apple_cf::cv::CVMetalTextureCache::from_raw(cache) }
            .ok_or_else(|| VideoError::Gpu("null Metal cache".into()))?;
        let lanczos = unsafe {
            let allocated: *mut Object = msg_send![class!(MPSImageLanczosScale), alloc];
            let initialized: *mut Object = msg_send![allocated, initWithDevice: device.as_ptr()];
            if initialized.is_null() {
                return Err(VideoError::Gpu("Lanczos scaler unavailable".into()));
            }
            // Do not blend zero-valued pixels outside the screen into its
            // border. Defaults differ between MPS kernels/devices.
            let _: () = msg_send![initialized, setEdgeMode: 1_u64]; // MPSImageEdgeModeClamp
            objc::rc::StrongPtr::new(initialized)
        };
        Ok(Self {
            queue,
            cache,
            lanczos,
            output: pixel_buffer(width, height)?,
        })
    }

    fn texture(&self, buffer: &CVPixelBuffer) -> Result<(CFType, metal::Texture), VideoError> {
        let mut reference = std::ptr::null_mut();
        let status = unsafe {
            CVMetalTextureCacheCreateTextureFromImage(
                std::ptr::null(),
                self.cache.as_ptr(),
                buffer.as_ptr(),
                std::ptr::null(),
                metal::MTLPixelFormat::BGRA8Unorm as u64,
                buffer.width(),
                buffer.height(),
                0,
                &mut reference,
            )
        };
        if status != 0 {
            return Err(VideoError::Gpu(format!("Metal texture import: {status}")));
        }
        let owner = unsafe { CFType::from_raw(reference) }
            .ok_or_else(|| VideoError::Gpu("null Metal texture".into()))?;
        let texture = unsafe { CVMetalTextureGetTexture(reference) };
        if texture.is_null() {
            return Err(VideoError::Gpu("missing Metal texture".into()));
        }
        // The owned texture and its CoreVideo view live through GPU completion.
        Ok((
            owner,
            unsafe { metal::TextureRef::from_ptr(texture) }.to_owned(),
        ))
    }

    pub(super) fn scale(&mut self, input: &CVPixelBuffer) -> Result<CVPixelBuffer, VideoError> {
        objc::rc::autoreleasepool(|| {
            copy_color_attachments(input, &self.output);
            let (_source_owner, source) = self.texture(input)?;
            let (_destination_owner, destination) = self.texture(&self.output)?;
            let commands = self.queue.new_command_buffer();
            unsafe {
                let _: () = msg_send![*self.lanczos, encodeToCommandBuffer: commands.as_ptr()
                    sourceTexture: source.as_ptr() destinationTexture: destination.as_ptr()];
            }
            commands.commit();
            commands.wait_until_completed();
            if commands.status() != metal::MTLCommandBufferStatus::Completed {
                return Err(VideoError::Gpu(format!(
                    "Metal scaling failed: {:?}",
                    commands.status()
                )));
            }
            Ok(self.output.clone())
        })
    }
}

fn copy_color_attachments(source: &CVPixelBuffer, destination: &CVPixelBuffer) {
    use apple_cf::raw as cv;
    // Preserve the source color interpretation, but not its old-size geometry
    // attachments. Clear missing values because the destination is reused.
    unsafe {
        for key in [
            cv::kCVImageBufferCGColorSpaceKey,
            cv::kCVImageBufferYCbCrMatrixKey,
            cv::kCVImageBufferColorPrimariesKey,
            cv::kCVImageBufferTransferFunctionKey,
        ] {
            let value =
                cv::CVBufferGetAttachment(source.as_ptr().cast(), key, std::ptr::null_mut());
            if value.is_null() {
                cv::CVBufferRemoveAttachment(destination.as_ptr().cast(), key);
            } else {
                cv::CVBufferSetAttachment(destination.as_ptr().cast(), key, value, 1);
            }
        }
    }
}

#[cfg(test)]
mod tests;
