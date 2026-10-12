//! An RGBA bitmap that CoreGraphics draws into.

use std::ptr;

use image::RgbaImage;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextGetBytesPerRow, CGBitmapContextGetData, CGColorSpace,
    CGContext, CGImageAlphaInfo,
};

use crate::image::unpremultiply;

pub(crate) struct Bitmap {
    context: CFRetained<CGContext>,
    width: u32,
    height: u32,
}

impl Bitmap {
    /// A transparent bitmap, or `None` when CoreGraphics cannot allocate one of this size.
    pub(crate) fn new(width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let color_space = CGColorSpace::new_device_rgb()?;
        // SAFETY: a null data pointer makes CoreGraphics allocate and own the pixel memory, and
        // the other arguments describe a supported 8-bit RGBA layout.
        let context = unsafe {
            CGBitmapContextCreate(
                ptr::null_mut(),
                width as usize,
                height as usize,
                8,
                width as usize * 4,
                Some(&color_space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }?;
        Some(Self {
            context,
            width,
            height,
        })
    }

    pub(crate) fn context(&self) -> &CGContext {
        &self.context
    }

    /// Copies the pixels out, top row first, with straight alpha.
    pub(crate) fn into_image(self) -> Option<RgbaImage> {
        let data = CGBitmapContextGetData(Some(&self.context)) as *const u8;
        if data.is_null() {
            return None;
        }
        let bytes_per_row = CGBitmapContextGetBytesPerRow(Some(&self.context));
        let row_len = self.width as usize * 4;
        let mut pixels = Vec::with_capacity(row_len * self.height as usize);
        for row in 0..self.height as usize {
            // SAFETY: the context owns `bytes_per_row * height` bytes at `data` for as long as
            // it lives, and each row holds at least `row_len` bytes.
            let row = unsafe { std::slice::from_raw_parts(data.add(row * bytes_per_row), row_len) };
            pixels.extend_from_slice(row);
        }
        unpremultiply(&mut pixels);
        RgbaImage::from_raw(self.width, self.height, pixels)
    }
}
