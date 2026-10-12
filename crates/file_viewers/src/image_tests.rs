use std::fs;
use std::io::Cursor;
use std::time::Duration;

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, DynamicImage, Frame as GifFrame};

use super::*;

const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);

/// An image whose left half is red and right half blue.
fn two_tone(width: u32, height: u32) -> RgbaImage {
    RgbaImage::from_fn(width, height, |x, _| if x < width / 2 { RED } else { BLUE })
}

fn encode(image: &RgbaImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    let image = DynamicImage::ImageRgba8(image.clone());
    match format {
        // JPEG has no alpha channel.
        ImageFormat::Jpeg => DynamicImage::ImageRgb8(image.to_rgb8())
            .write_to(&mut bytes, format)
            .unwrap(),
        _ => image.write_to(&mut bytes, format).unwrap(),
    }
    bytes.into_inner()
}

/// Inserts an EXIF block holding only `orientation` at the start of a JPEG file.
fn with_exif_orientation(jpeg: &[u8], orientation: u8) -> Vec<u8> {
    let mut exif = b"Exif\0\0".to_vec();
    // A big-endian TIFF header, then one directory with the orientation tag (0x0112).
    exif.extend_from_slice(b"MM\0\x2a\0\0\0\x08");
    exif.extend_from_slice(&[0, 1]);
    exif.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, orientation, 0, 0]);
    exif.extend_from_slice(&[0, 0, 0, 0]);

    let mut file = jpeg[..2].to_vec();
    file.extend_from_slice(&[0xff, 0xe1]);
    file.extend_from_slice(&(exif.len() as u16 + 2).to_be_bytes());
    file.extend_from_slice(&exif);
    file.extend_from_slice(&jpeg[2..]);
    file
}

fn first_frame(decoded: &DecodedImage) -> &RgbaImage {
    &decoded.frames[0].pyramid.levels[0].tiles[0].image
}

fn is_reddish(pixel: &Rgba<u8>) -> bool {
    pixel[0] > 180 && pixel[2] < 80
}

fn is_bluish(pixel: &Rgba<u8>) -> bool {
    pixel[2] > 180 && pixel[0] < 80
}

#[test]
fn decodes_every_portable_format() {
    let source = two_tone(16, 8);
    for (format, name) in [
        (ImageFormat::Png, "PNG"),
        (ImageFormat::Jpeg, "JPEG"),
        (ImageFormat::Gif, "GIF"),
        (ImageFormat::WebP, "WebP"),
        (ImageFormat::Bmp, "BMP"),
        (ImageFormat::Tiff, "TIFF"),
        (ImageFormat::Ico, "ICO"),
    ] {
        let decoded = decode_bytes(&encode(&source, format)).unwrap();
        assert_eq!((decoded.width, decoded.height), (16, 8), "{name}");
        assert_eq!(decoded.format, name);
        assert_eq!(decoded.frames.len(), 1, "{name}");
        assert!(!decoded.is_animated());
        let image = first_frame(&decoded);
        assert!(is_reddish(image.get_pixel(2, 4)), "{name}");
        assert!(is_bluish(image.get_pixel(13, 4)), "{name}");
    }
}

#[test]
fn applies_the_exif_orientation() {
    let jpeg = encode(&two_tone(32, 16), ImageFormat::Jpeg);

    // Orientation 6: the stored image is shown turned a quarter turn clockwise, which puts
    // its left edge at the top.
    let decoded = decode_bytes(&with_exif_orientation(&jpeg, 6)).unwrap();
    assert_eq!((decoded.width, decoded.height), (16, 32));
    let image = first_frame(&decoded);
    assert!(is_reddish(image.get_pixel(8, 4)));
    assert!(is_bluish(image.get_pixel(8, 28)));

    // Orientation 3: upside down, which swaps the halves.
    let decoded = decode_bytes(&with_exif_orientation(&jpeg, 3)).unwrap();
    assert_eq!((decoded.width, decoded.height), (32, 16));
    let image = first_frame(&decoded);
    assert!(is_bluish(image.get_pixel(4, 8)));
    assert!(is_reddish(image.get_pixel(28, 8)));
}

fn animated_gif(frames: usize, delay_ms: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        encoder.set_repeat(Repeat::Infinite).unwrap();
        for index in 0..frames {
            let color = if index % 2 == 0 { RED } else { BLUE };
            let frame = GifFrame::from_parts(
                RgbaImage::from_pixel(8, 8, color),
                0,
                0,
                Delay::from_saturating_duration(Duration::from_millis(delay_ms)),
            );
            encoder.encode_frame(frame).unwrap();
        }
    }
    bytes
}

#[test]
fn decodes_the_frames_of_an_animation() {
    let decoded = decode_bytes(&animated_gif(3, 70)).unwrap();

    assert!(decoded.is_animated());
    assert_eq!(decoded.frames.len(), 3);
    assert!(decoded.frames.iter().all(|frame| frame.delay_ms == 70));
    assert_eq!(decoded.animation_duration_ms(), 210);
    assert!(is_reddish(first_frame(&decoded).get_pixel(1, 1)));
    assert!(is_bluish(
        decoded.frames[1].pyramid.levels[0].tiles[0]
            .image
            .get_pixel(1, 1)
    ));
}

#[test]
fn a_frame_without_a_delay_gets_the_default_one() {
    let decoded = decode_bytes(&animated_gif(2, 0)).unwrap();
    assert!(decoded.frames.iter().all(|frame| frame.delay_ms == 100));
}

#[test]
fn picks_the_frame_for_a_point_in_time() {
    let delays = [100u32, 50, 200];
    assert_eq!(frame_at(delays.iter().copied(), 0), (0, 100));
    assert_eq!(frame_at(delays.iter().copied(), 99), (0, 1));
    assert_eq!(frame_at(delays.iter().copied(), 100), (1, 50));
    assert_eq!(frame_at(delays.iter().copied(), 349), (2, 1));
    // The animation loops.
    assert_eq!(frame_at(delays.iter().copied(), 350), (0, 100));
    assert_eq!(frame_at(delays.iter().copied(), 3_610), (1, 40));
    assert_eq!(frame_at([0u32].iter().copied(), 5), (0, u64::MAX));
}

#[test]
fn reports_transparency() {
    let opaque = decode_bytes(&encode(&two_tone(4, 4), ImageFormat::Png)).unwrap();
    assert!(!opaque.has_transparency);

    let mut image = two_tone(4, 4);
    image.put_pixel(1, 1, Rgba([0, 0, 0, 0]));
    let transparent = decode_bytes(&encode(&image, ImageFormat::Png)).unwrap();
    assert!(transparent.has_transparency);
}

#[test]
fn refuses_what_is_not_an_image() {
    assert!(matches!(
        decode_bytes(b"version https://git-lfs.github.com/spec/v1\n"),
        Err(DecodeError::UnknownFormat)
    ));
    let mut truncated = encode(&two_tone(64, 64), ImageFormat::Png);
    truncated.truncate(40);
    assert!(matches!(
        decode_bytes(&truncated),
        Err(DecodeError::Decode(_))
    ));
}

#[test]
fn a_large_image_is_kept_within_the_pixel_budget() {
    let png = encode(&two_tone(400, 200), ImageFormat::Png);
    let decoded = decode_bytes_within(&png, 20_000).unwrap();

    // The size reported is the one of the file; the pixels kept are fewer.
    assert_eq!((decoded.width, decoded.height), (400, 200));
    let pyramid = &decoded.frames[0].pyramid;
    assert_eq!(pyramid.content, Size::new(400., 200.));
    let image = first_frame(&decoded);
    assert!(u64::from(image.width()) * u64::from(image.height()) <= 20_000);
    assert_eq!(image.dimensions(), (200, 100));
    assert!((pyramid.sharpest_pixels_per_unit() - 0.5).abs() < 1e-6);
    assert_eq!(
        pyramid.levels[0].tiles[0].rect,
        Rect::new(0., 0., 400., 200.)
    );
    assert_eq!(decoded.notes.len(), 1);
    assert!(decoded.notes[0].contains("200 × 100"));
}

#[test]
fn pixel_budget_scale() {
    assert_eq!(scale_to_pixel_budget(100, 100, 10_000), 1.);
    assert_eq!(scale_to_pixel_budget(200, 100, 5_000), 0.5);
    assert_eq!(scale_to_pixel_budget(0, 100, 5_000), 1.);
}

#[test]
fn a_large_image_is_cut_into_tiles_at_every_level() {
    let pyramid = Pyramid::build(two_tone(5000, 3000), Size::new(5000., 3000.), true);

    let sizes: Vec<f32> = pyramid
        .levels
        .iter()
        .map(|level| level.pixels_per_unit)
        .collect();
    assert_eq!(sizes, vec![1., 0.5, 0.25, 0.125]);
    let tile_counts: Vec<usize> = pyramid
        .levels
        .iter()
        .map(|level| level.tiles.len())
        .collect();
    assert_eq!(tile_counts, vec![6, 2, 1, 1]);

    for level in &pyramid.levels {
        for tile in &level.tiles {
            assert!(tile.image.width() <= TILE_SIZE && tile.image.height() <= TILE_SIZE);
        }
    }
    // Tiles of a level cover the content exactly, with the last column and row narrower.
    let last = pyramid.levels[0].tiles.last().unwrap();
    assert_eq!(last.rect, Rect::new(4096., 2048., 904., 952.));
    assert_eq!(last.image.dimensions(), (904, 952));
    assert_eq!(*last.image.get_pixel(0, 0), BLUE);
}

#[test]
fn draws_from_the_smallest_level_that_is_sharp_enough() {
    let pyramid = Pyramid::build(two_tone(5000, 3000), Size::new(5000., 3000.), true)
        .map(|image| image.dimensions());
    let ppu = |needed: f32| pyramid.level_for(needed).unwrap().pixels_per_unit;

    assert_eq!(ppu(0.05), 0.125);
    assert_eq!(ppu(0.125), 0.125);
    assert_eq!(ppu(0.2), 0.25);
    assert_eq!(ppu(0.6), 1.);
    // Zoomed in past the sharpest level there is nothing sharper to draw.
    assert_eq!(ppu(4.), 1.);
}

#[test]
fn only_the_visible_tiles_are_drawn() {
    let pyramid = Pyramid::build(two_tone(5000, 3000), Size::new(5000., 3000.), true)
        .map(|image| image.dimensions());

    let corner: Vec<Rect> = pyramid
        .visible_tiles(1., Rect::new(0., 0., 800., 600.))
        .map(|tile| tile.rect)
        .collect();
    assert_eq!(corner, vec![Rect::new(0., 0., 2048., 2048.)]);

    let across_a_seam = pyramid
        .visible_tiles(1., Rect::new(1800., 1900., 800., 600.))
        .count();
    assert_eq!(across_a_seam, 4);

    let zoomed_out = pyramid
        .visible_tiles(0.1, Rect::new(0., 0., 5000., 3000.))
        .count();
    assert_eq!(zoomed_out, 1);
}

#[test]
fn an_animation_frame_has_a_single_level() {
    let pyramid = Pyramid::build(two_tone(3000, 100), Size::new(3000., 100.), false);
    assert_eq!(pyramid.levels.len(), 1);
    assert_eq!(pyramid.levels[0].tiles.len(), 2);
}

#[test]
fn removes_premultiplication() {
    let mut pixels = [128, 64, 0, 128, 10, 20, 30, 255, 9, 9, 9, 0];
    unpremultiply(&mut pixels);
    assert_eq!(pixels, [255, 128, 0, 128, 10, 20, 30, 255, 9, 9, 9, 0]);
}

#[test]
fn checkerboard_alternates() {
    let board = checkerboard(2, 4, [255; 4], [0, 0, 0, 255]);
    assert_eq!(board.dimensions(), (8, 8));
    assert_eq!(board.get_pixel(0, 0).0, [255; 4]);
    assert_eq!(board.get_pixel(1, 1).0, [255; 4]);
    assert_eq!(board.get_pixel(2, 0).0, [0, 0, 0, 255]);
    assert_eq!(board.get_pixel(2, 2).0, [255; 4]);
}

#[test]
fn decodes_a_file_and_reports_a_missing_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two-tone.png");
    fs::write(&path, encode(&two_tone(16, 8), ImageFormat::Png)).unwrap();

    let decoded = decode_file(&path).unwrap();
    assert_eq!((decoded.width, decoded.height), (16, 8));
    assert_eq!(decoded.content_size(), Size::new(16., 8.));

    assert!(matches!(
        decode_file(&dir.path().join("missing.png")),
        Err(DecodeError::Io(_))
    ));
}

#[cfg(target_os = "macos")]
mod system_decoder {
    use command::blocking::Command;

    use super::*;

    #[test]
    fn decodes_upright_and_within_the_pixel_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        let jpeg = encode(&two_tone(320, 160), ImageFormat::Jpeg);
        fs::write(&path, with_exif_orientation(&jpeg, 6)).unwrap();

        let full = crate::imageio::decode(&path, MAX_KEPT_PIXELS).unwrap();
        assert_eq!((full.width, full.height), (160, 320));
        assert_eq!(full.image.dimensions(), (160, 320));
        assert!(is_reddish(full.image.get_pixel(80, 20)));
        assert!(is_bluish(full.image.get_pixel(80, 300)));

        let reduced = crate::imageio::decode(&path, 160 * 320 / 4).unwrap();
        assert_eq!((reduced.width, reduced.height), (160, 320));
        assert_eq!(reduced.image.dimensions(), (80, 160));
    }

    #[test]
    fn falls_back_for_a_format_without_a_built_in_decoder() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("source.png");
        fs::write(&png, encode(&two_tone(64, 32), ImageFormat::Png)).unwrap();
        let heic = dir.path().join("photo.heic");
        let converted = Command::new("/usr/bin/sips")
            .args(["-s", "format", "heic"])
            .arg(&png)
            .arg("--out")
            .arg(&heic)
            .output()
            .is_ok_and(|output| output.status.success());
        if !converted || !heic.is_file() {
            eprintln!("skipped: this system cannot write HEIC files");
            return;
        }

        let decoded = decode_file(&heic).unwrap();
        assert_eq!((decoded.width, decoded.height), (64, 32));
        assert_eq!(decoded.format, "HEIC");
        let image = first_frame(&decoded);
        assert!(is_reddish(image.get_pixel(8, 16)));
        assert!(is_bluish(image.get_pixel(56, 16)));
    }
}
