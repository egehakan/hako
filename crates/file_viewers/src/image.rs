//! Decoding raster images into tiles the renderer can draw at any zoom level.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::imageops::{self, FilterType};
use image::{
    AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, Rgba, RgbaImage,
};
use thiserror::Error;

use crate::geometry::{Rect, Size};

/// Side of the largest texture a tile becomes. Far below the 8192 pixel limit of the renderer,
/// so that a tile is cheap to upload and only the visible part of a large image is on the GPU.
pub const TILE_SIZE: u32 = 2048;

/// Pixels kept for the sharpest level of an image; larger images are shown downscaled.
pub const MAX_KEPT_PIXELS: u64 = 64_000_000;

/// Memory a decoder may allocate before the image is refused.
const MAX_DECODE_BYTES: u64 = 1 << 30;

/// Files larger than this are not read into memory.
const MAX_FILE_BYTES: u64 = 1 << 30;

/// Memory the frames of an animation may take before only its first frame is shown.
const ANIMATION_BUDGET_BYTES: u64 = 256 << 20;

/// Levels stop halving once the longest side is at most this many pixels.
const SMALLEST_LEVEL_SIDE: u32 = 1024;

/// Shortest time a frame of an animation stays up, in milliseconds. Files that ask for less
/// are slowed down the way browsers do it.
const MIN_FRAME_DELAY_MS: u32 = 20;
const DEFAULT_FRAME_DELAY_MS: u32 = 100;

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("The file could not be read: {0}")]
    Io(#[from] std::io::Error),
    #[error("The file is too large to show ({0})")]
    TooLarge(String),
    #[error("The image format is not recognised")]
    UnknownFormat,
    #[error("The image could not be decoded: {0}")]
    Decode(String),
    #[error("The image has no pixels")]
    Empty,
}

impl From<image::ImageError> for DecodeError {
    fn from(err: image::ImageError) -> Self {
        Self::Decode(err.to_string())
    }
}

/// A rectangular piece of one level of a [`Pyramid`].
#[derive(Clone, Debug)]
pub struct Tile<T> {
    /// Area of the content the tile covers, in content units.
    pub rect: Rect,
    pub image: T,
}

/// The whole content at one resolution, cut into tiles.
#[derive(Clone, Debug)]
pub struct Level<T> {
    /// Pixels of this level per content unit.
    pub pixels_per_unit: f32,
    pub tiles: Vec<Tile<T>>,
}

/// One picture at decreasing resolutions, sharpest first. Drawing from the level closest to
/// the size on screen avoids the shimmer of a large texture squeezed into a few pixels.
#[derive(Clone, Debug)]
pub struct Pyramid<T> {
    pub content: Size,
    pub levels: Vec<Level<T>>,
}

impl Pyramid<RgbaImage> {
    /// Cuts `image`, which shows content of size `content`, into tiles, with smaller levels
    /// below it when `with_smaller_levels` is set.
    pub fn build(image: RgbaImage, content: Size, with_smaller_levels: bool) -> Self {
        let mut levels = Vec::new();
        let mut current = image;
        loop {
            let (width, height) = current.dimensions();
            let smaller =
                (with_smaller_levels && width.max(height) > SMALLEST_LEVEL_SIDE).then(|| {
                    imageops::resize(
                        &current,
                        (width / 2).max(1),
                        (height / 2).max(1),
                        FilterType::Triangle,
                    )
                });
            levels.push(cut_into_tiles(current, content));
            match smaller {
                Some(smaller) => current = smaller,
                None => break,
            }
        }
        Self { content, levels }
    }
}

impl<T> Pyramid<T> {
    /// Converts the image of every tile.
    pub fn map<U>(self, mut convert: impl FnMut(T) -> U) -> Pyramid<U> {
        Pyramid {
            content: self.content,
            levels: self
                .levels
                .into_iter()
                .map(|level| Level {
                    pixels_per_unit: level.pixels_per_unit,
                    tiles: level
                        .tiles
                        .into_iter()
                        .map(|tile| Tile {
                            rect: tile.rect,
                            image: convert(tile.image),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// The level to draw from when a content unit covers `pixels_per_unit` device pixels: the
    /// smallest one that is still at least that sharp, or the sharpest there is.
    pub fn level_for(&self, pixels_per_unit: f32) -> Option<&Level<T>> {
        self.levels
            .iter()
            .rev()
            .find(|level| level.pixels_per_unit >= pixels_per_unit)
            .or(self.levels.first())
    }

    /// The tiles to draw to show `visible`, an area in content units, at `pixels_per_unit`
    /// device pixels per content unit.
    pub fn visible_tiles(
        &self,
        pixels_per_unit: f32,
        visible: Rect,
    ) -> impl Iterator<Item = &Tile<T>> {
        self.level_for(pixels_per_unit)
            .into_iter()
            .flat_map(|level| level.tiles.iter())
            .filter(move |tile| tile.rect.intersects(visible))
    }

    /// Pixels per content unit of the sharpest level.
    pub fn sharpest_pixels_per_unit(&self) -> f32 {
        self.levels
            .first()
            .map_or(0., |level| level.pixels_per_unit)
    }
}

fn cut_into_tiles(image: RgbaImage, content: Size) -> Level<RgbaImage> {
    let (width, height) = image.dimensions();
    let units_per_pixel_x = content.width / width as f32;
    let units_per_pixel_y = content.height / height as f32;
    let rect_of = |x: u32, y: u32, tile_width: u32, tile_height: u32| {
        Rect::new(
            x as f32 * units_per_pixel_x,
            y as f32 * units_per_pixel_y,
            tile_width as f32 * units_per_pixel_x,
            tile_height as f32 * units_per_pixel_y,
        )
    };

    let tiles = if width <= TILE_SIZE && height <= TILE_SIZE {
        vec![Tile {
            rect: rect_of(0, 0, width, height),
            image,
        }]
    } else {
        let mut tiles = Vec::new();
        for y in (0..height).step_by(TILE_SIZE as usize) {
            for x in (0..width).step_by(TILE_SIZE as usize) {
                let tile_width = TILE_SIZE.min(width - x);
                let tile_height = TILE_SIZE.min(height - y);
                tiles.push(Tile {
                    rect: rect_of(x, y, tile_width, tile_height),
                    image: imageops::crop_imm(&image, x, y, tile_width, tile_height).to_image(),
                });
            }
        }
        tiles
    };
    Level {
        pixels_per_unit: width as f32 / content.width,
        tiles,
    }
}

/// One frame of a decoded image. A still image has a single frame.
pub struct Frame {
    pub pyramid: Pyramid<RgbaImage>,
    /// How long the frame stays up in an animation, in milliseconds.
    pub delay_ms: u32,
}

pub struct DecodedImage {
    /// Size of the image in the file, in pixels, after its orientation is applied.
    pub width: u32,
    pub height: u32,
    /// Name of the file format, e.g. `PNG`.
    pub format: String,
    pub frames: Vec<Frame>,
    pub has_transparency: bool,
    /// What was done differently from showing the file as it is, in words for the user.
    pub notes: Vec<String>,
}

impl DecodedImage {
    pub fn content_size(&self) -> Size {
        Size::new(self.width as f32, self.height as f32)
    }

    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    /// Length of one run of the animation, in milliseconds.
    pub fn animation_duration_ms(&self) -> u64 {
        self.frames
            .iter()
            .map(|frame| u64::from(frame.delay_ms))
            .sum()
    }

    /// Index of the frame showing `elapsed_ms` after the animation started, and the time
    /// until the next one, in milliseconds.
    pub fn frame_at(&self, elapsed_ms: u64) -> (usize, u64) {
        frame_at(self.frames.iter().map(|frame| frame.delay_ms), elapsed_ms)
    }
}

/// See [`DecodedImage::frame_at`]; `delays` holds the time each frame stays up.
pub fn frame_at(delays: impl Iterator<Item = u32> + Clone, elapsed_ms: u64) -> (usize, u64) {
    let total: u64 = delays.clone().map(u64::from).sum();
    if total == 0 {
        return (0, u64::MAX);
    }
    let mut position = elapsed_ms % total;
    for (index, delay) in delays.enumerate() {
        let delay = u64::from(delay);
        if position < delay {
            return (index, delay - position);
        }
        position -= delay;
    }
    (0, u64::MAX)
}

/// Factor to scale an image of `width` by `height` pixels by so that it has at most
/// `max_pixels` pixels. `1` when it already does.
pub fn scale_to_pixel_budget(width: u32, height: u32, max_pixels: u64) -> f32 {
    let pixels = u64::from(width) * u64::from(height);
    if pixels <= max_pixels || pixels == 0 {
        return 1.;
    }
    (max_pixels as f64 / pixels as f64).sqrt() as f32
}

/// Decodes the image file at `path`.
pub fn decode_file(path: &Path) -> Result<DecodedImage, DecodeError> {
    let size = fs::metadata(path)?.len();
    if size > MAX_FILE_BYTES {
        return Err(DecodeError::TooLarge(crate::format_bytes(size)));
    }
    let bytes = fs::read(path)?;
    match decode_bytes(&bytes) {
        Ok(decoded) => Ok(decoded),
        Err(err) => decode_with_system_decoder(path, err),
    }
}

/// Falls back to the decoder of the operating system, which reads more formats and can
/// decode an image too large to hold at full size straight to a smaller one.
#[cfg(target_os = "macos")]
fn decode_with_system_decoder(
    path: &Path,
    first_error: DecodeError,
) -> Result<DecodedImage, DecodeError> {
    let Some(system) = crate::imageio::decode(path, MAX_KEPT_PIXELS) else {
        return Err(first_error);
    };
    let mut notes = Vec::new();
    add_downscale_note(
        &mut notes,
        (system.width, system.height),
        system.image.dimensions(),
    );
    Ok(still_image(
        system.image,
        (system.width, system.height),
        system.format,
        notes,
    ))
}

#[cfg(not(target_os = "macos"))]
fn decode_with_system_decoder(
    _path: &Path,
    first_error: DecodeError,
) -> Result<DecodedImage, DecodeError> {
    Err(first_error)
}

/// Decodes an image held in memory with the built-in decoders.
pub fn decode_bytes(bytes: &[u8]) -> Result<DecodedImage, DecodeError> {
    decode_bytes_within(bytes, MAX_KEPT_PIXELS)
}

/// [`decode_bytes`] keeping at most `max_pixels` pixels of a still image.
fn decode_bytes_within(bytes: &[u8], max_pixels: u64) -> Result<DecodedImage, DecodeError> {
    let format = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .format()
        .ok_or(DecodeError::UnknownFormat)?;

    let animation = match format {
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(Cursor::new(bytes))?;
            decode_animation(decoder, format)?
        }
        ImageFormat::WebP => {
            let decoder = WebPDecoder::new(Cursor::new(bytes))?;
            if decoder.has_animation() {
                decode_animation(decoder, format)?
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(animation) = animation {
        return Ok(animation);
    }

    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    let image = image.into_rgba8();
    let full_size = image.dimensions();

    let mut notes = Vec::new();
    let scale = scale_to_pixel_budget(full_size.0, full_size.1, max_pixels);
    let image = if scale < 1. {
        let width = ((full_size.0 as f32 * scale) as u32).max(1);
        let height = ((full_size.1 as f32 * scale) as u32).max(1);
        imageops::resize(&image, width, height, FilterType::Triangle)
    } else {
        image
    };
    add_downscale_note(&mut notes, full_size, image.dimensions());
    Ok(still_image(image, full_size, format_name(format), notes))
}

fn add_downscale_note(notes: &mut Vec<String>, full_size: (u32, u32), shown_size: (u32, u32)) {
    if shown_size.0 < full_size.0 || shown_size.1 < full_size.1 {
        notes.push(format!(
            "Shown at {} × {} pixels to limit memory use; zooming in past that is not sharper.",
            shown_size.0, shown_size.1
        ));
    }
}

fn still_image(
    image: RgbaImage,
    full_size: (u32, u32),
    format: impl Into<String>,
    notes: Vec<String>,
) -> DecodedImage {
    let content = Size::new(full_size.0 as f32, full_size.1 as f32);
    let has_transparency = has_transparency(&image);
    DecodedImage {
        width: full_size.0,
        height: full_size.1,
        format: format.into(),
        frames: vec![Frame {
            pyramid: Pyramid::build(image, content, true),
            delay_ms: 0,
        }],
        has_transparency,
        notes,
    }
}

/// Decodes every frame of an animation, or `None` when the file holds a single frame, which
/// the still path decodes with its orientation.
fn decode_animation<'a>(
    decoder: impl AnimationDecoder<'a> + ImageDecoder,
    format: ImageFormat,
) -> Result<Option<DecodedImage>, DecodeError> {
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(DecodeError::Empty);
    }
    let frame_bytes = u64::from(width) * u64::from(height) * 4;
    let content = Size::new(width as f32, height as f32);

    let mut frames = Vec::new();
    let mut notes = Vec::new();
    let mut has_transparency = false;
    let mut used_bytes = 0;
    for frame in decoder.into_frames() {
        let frame = match frame {
            Ok(frame) => frame,
            // A file cut short still shows the frames before the damage.
            Err(err) if !frames.is_empty() => {
                notes.push(format!("The animation stops early: {err}"));
                break;
            }
            Err(err) => return Err(err.into()),
        };
        used_bytes += frame_bytes;
        if !frames.is_empty() && used_bytes > ANIMATION_BUDGET_BYTES {
            frames.truncate(1);
            notes.push(format!(
                "Only the first frame is shown: the animation needs more than {} of memory.",
                crate::format_bytes(ANIMATION_BUDGET_BYTES)
            ));
            break;
        }

        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay_ms = match numerator.checked_div(denominator) {
            Some(0) | None => DEFAULT_FRAME_DELAY_MS,
            Some(delay) => delay.max(MIN_FRAME_DELAY_MS),
        };
        let image = frame.into_buffer();
        has_transparency |= frames.is_empty() && self::has_transparency(&image);
        frames.push(Frame {
            pyramid: Pyramid::build(image, content, false),
            delay_ms,
        });
    }

    if frames.len() < 2 && notes.is_empty() {
        return Ok(None);
    }
    if frames.is_empty() {
        return Err(DecodeError::Empty);
    }
    Ok(Some(DecodedImage {
        width,
        height,
        format: format_name(format).to_string(),
        frames,
        has_transparency,
        notes,
    }))
}

fn has_transparency(image: &RgbaImage) -> bool {
    image
        .pixels()
        .any(|Rgba([_, _, _, alpha])| *alpha < u8::MAX)
}

fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Ico => "ICO",
        ImageFormat::Tiff => "TIFF",
        // `ImageFormat` is non-exhaustive, and only the formats above are compiled in.
        _ => "Image",
    }
}

/// Converts premultiplied RGBA pixels, which is what rasterisers produce, to straight alpha,
/// which is what decoded images use.
pub(crate) fn unpremultiply(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

/// A light and dark checkerboard of `cells` by `cells` squares of `cell_size` pixels, drawn
/// behind images so that their transparent areas can be told from white or black ones.
pub fn checkerboard(cell_size: u32, cells: u32, light: [u8; 4], dark: [u8; 4]) -> RgbaImage {
    let cell_size = cell_size.max(1);
    RgbaImage::from_fn(cell_size * cells, cell_size * cells, |x, y| {
        if (x / cell_size + y / cell_size) % 2 == 0 {
            Rgba(light)
        } else {
            Rgba(dark)
        }
    })
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
