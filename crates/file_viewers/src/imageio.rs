//! Image decoding through ImageIO, the decoder of macOS.

use std::path::Path;
use std::ptr::NonNull;

use image::RgbaImage;
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CFURL, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{CGContext, CGImage};

use crate::bitmap::Bitmap;
use crate::image::scale_to_pixel_budget;

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C-unwind" {
    static kCGImageSourceCreateThumbnailFromImageAlways: &'static CFString;
    static kCGImageSourceCreateThumbnailWithTransform: &'static CFString;
    static kCGImageSourceThumbnailMaxPixelSize: &'static CFString;
    static kCGImagePropertyPixelWidth: &'static CFString;
    static kCGImagePropertyPixelHeight: &'static CFString;
    static kCGImagePropertyOrientation: &'static CFString;

    fn CGImageSourceCreateWithURL(
        url: &CFURL,
        options: Option<&CFDictionary>,
    ) -> Option<NonNull<CFType>>;
    fn CGImageSourceCopyPropertiesAtIndex(
        source: &CFType,
        index: usize,
        options: Option<&CFDictionary>,
    ) -> Option<NonNull<CFDictionary>>;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: &CFType,
        index: usize,
        options: Option<&CFDictionary>,
    ) -> Option<NonNull<CGImage>>;
}

pub(crate) struct SystemImage {
    /// Size of the image in the file, in pixels, after its orientation is applied.
    pub width: u32,
    pub height: u32,
    pub format: String,
    /// The pixels, scaled down when the full image exceeds the pixel budget.
    pub image: RgbaImage,
}

/// Decodes the first image of the file at `path` upright, at a size of at most `max_pixels`
/// pixels. ImageIO scales while it decodes, so the full-size image never has to fit in memory.
pub(crate) fn decode(path: &Path, max_pixels: u64) -> Option<SystemImage> {
    let url = CFURL::from_file_path(path)?;
    // SAFETY: `url` is a valid URL, and the function returns an owned source or null.
    let source = unsafe { CGImageSourceCreateWithURL(&url, None) }?;
    // SAFETY: the pointer was just returned to us with a +1 retain count.
    let source = unsafe { CFRetained::from_raw(source) };

    let (width, height) = oriented_size(&source)?;
    let scale = scale_to_pixel_budget(width, height, max_pixels);
    let longest_side = (width.max(height) as f32 * scale).ceil().max(1.) as i32;

    // SAFETY: the keys are constants of ImageIO, valid for the lifetime of the process.
    let keys = unsafe {
        [
            kCGImageSourceCreateThumbnailFromImageAlways,
            kCGImageSourceCreateThumbnailWithTransform,
            kCGImageSourceThumbnailMaxPixelSize,
        ]
    };
    let longest_side = CFNumber::new_i32(longest_side);
    let yes: &CFType = CFBoolean::new(true).as_ref();
    let values: [&CFType; 3] = [yes, yes, longest_side.as_ref()];
    let options = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    // SAFETY: `source` is a live image source and `options` a dictionary of the documented
    // option keys; the function returns an owned image or null.
    let image =
        unsafe { CGImageSourceCreateThumbnailAtIndex(&source, 0, Some(options.as_opaque())) }?;
    // SAFETY: the pointer was just returned to us with a +1 retain count.
    let image = unsafe { CFRetained::from_raw(image) };

    let decoded_width = u32::try_from(CGImage::width(Some(&image))).ok()?;
    let decoded_height = u32::try_from(CGImage::height(Some(&image))).ok()?;
    let bitmap = Bitmap::new(decoded_width, decoded_height)?;
    let bounds = CGRect::new(
        CGPoint::new(0., 0.),
        CGSize::new(decoded_width.into(), decoded_height.into()),
    );
    CGContext::draw_image(Some(bitmap.context()), bounds, Some(&image));

    Some(SystemImage {
        width: width.max(decoded_width),
        height: height.max(decoded_height),
        format: path
            .extension()
            .map(|extension| extension.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| "Image".to_string()),
        image: bitmap.into_image()?,
    })
}

/// Pixel size of the first image of `source` once its orientation is applied.
fn oriented_size(source: &CFType) -> Option<(u32, u32)> {
    // SAFETY: `source` is a live image source; the function returns an owned dictionary or
    // null.
    let properties = unsafe { CGImageSourceCopyPropertiesAtIndex(source, 0, None) }?;
    // SAFETY: the pointer was just returned to us with a +1 retain count, and image properties
    // are keyed by strings.
    let properties: CFRetained<CFDictionary<CFString, CFType>> =
        unsafe { CFRetained::cast_unchecked(CFRetained::from_raw(properties)) };

    let number = |key: &CFString| {
        properties
            .get(key)
            .and_then(|value| value.downcast_ref::<CFNumber>().and_then(CFNumber::as_i64))
    };
    // SAFETY: the keys are constants of ImageIO, valid for the lifetime of the process.
    let (width, height, orientation) = unsafe {
        (
            number(kCGImagePropertyPixelWidth)?,
            number(kCGImagePropertyPixelHeight)?,
            number(kCGImagePropertyOrientation).unwrap_or(1),
        )
    };
    let width = u32::try_from(width).ok()?;
    let height = u32::try_from(height).ok()?;
    // EXIF orientations 5 to 8 turn the image by a quarter turn.
    if (5..=8).contains(&orientation) {
        Some((height, width))
    } else {
        Some((width, height))
    }
}
