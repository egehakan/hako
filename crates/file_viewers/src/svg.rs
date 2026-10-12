//! Rasterising SVG documents.

use std::sync::{Arc, LazyLock};

use image::RgbaImage;
use resvg::{tiny_skia, usvg};
use thiserror::Error;

use crate::geometry::Size;
use crate::image::unpremultiply;

/// Longest side of a raster, in pixels.
const MAX_RASTER_SIDE: f32 = 8192.;

/// Pixels a raster may have. Zooming in further than this allows stretches the raster.
const MAX_RASTER_PIXELS: f32 = 48_000_000.;

static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut fonts = usvg::fontdb::Database::new();
    fonts.load_system_fonts();
    Arc::new(fonts)
});

#[derive(Debug, Error)]
pub enum SvgError {
    #[error("The SVG could not be parsed: {0}")]
    Parse(String),
    #[error("The SVG has no size")]
    Empty,
    #[error("The SVG is too large to draw")]
    TooLarge,
}

pub struct SvgRaster {
    /// Size the document asks for, in CSS pixels.
    pub size: Size,
    /// Pixels of `image` per CSS pixel.
    pub pixels_per_unit: f32,
    pub image: RgbaImage,
}

/// The highest scale, in pixels per CSS pixel, at which a document of `size` may be rasterised.
pub fn max_pixels_per_unit(size: Size) -> f32 {
    if size.is_empty() {
        return 1.;
    }
    let by_side = MAX_RASTER_SIDE / size.width.max(size.height);
    let by_area = (MAX_RASTER_PIXELS / (size.width * size.height)).sqrt();
    by_side.min(by_area)
}

/// The scale to rasterise at so that a view showing `pixels_per_unit` device pixels per CSS
/// pixel is sharp. Scales are rounded up to a power of two so that zooming re-rasterises only
/// now and then.
pub fn raster_scale_for(pixels_per_unit: f32, size: Size) -> f32 {
    let wanted = pixels_per_unit.max(1.).log2().ceil().exp2();
    wanted.min(max_pixels_per_unit(size))
}

/// Draws the SVG document `source` at `pixels_per_unit` pixels per CSS pixel, lowered as far
/// as the size limits of a raster require.
pub fn render(source: &str, pixels_per_unit: f32) -> Result<SvgRaster, SvgError> {
    let options = usvg::Options {
        fontdb: FONTS.clone(),
        ..Default::default()
    };
    let tree =
        usvg::Tree::from_str(source, &options).map_err(|err| SvgError::Parse(err.to_string()))?;
    let size = Size::new(tree.size().width(), tree.size().height());
    if size.is_empty() {
        return Err(SvgError::Empty);
    }

    let pixels_per_unit = pixels_per_unit.min(max_pixels_per_unit(size));
    let width = (size.width * pixels_per_unit).ceil().max(1.) as u32;
    let height = (size.height * pixels_per_unit).ceil().max(1.) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or(SvgError::TooLarge)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(pixels_per_unit, pixels_per_unit),
        &mut pixmap.as_mut(),
    );

    let mut pixels = pixmap.take();
    unpremultiply(&mut pixels);
    let image = RgbaImage::from_raw(width, height, pixels).ok_or(SvgError::TooLarge)?;
    Ok(SvgRaster {
        size,
        pixels_per_unit,
        image,
    })
}

#[cfg(test)]
#[path = "svg_tests.rs"]
mod tests;
