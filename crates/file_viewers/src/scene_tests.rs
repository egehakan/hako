use image::RgbaImage;

use super::*;
use crate::geometry::ZoomMode;
use crate::pdf::PageGeometry;

const VIEWPORT: Viewport = Viewport {
    size: Size::new(800., 600.),
    device_scale: 2.,
};

fn pyramid(width: u32, height: u32) -> Pyramid<(u32, u32)> {
    Pyramid::build(
        RgbaImage::new(width, height),
        Size::new(width as f32, height as f32),
        true,
    )
    .map(|image| image.dimensions())
}

#[test]
fn a_fitted_image_is_drawn_once_from_a_small_level() {
    let pyramid = pyramid(4000, 3000);
    let transform = ViewTransform::new(ZoomMode::Fit);
    let placement = Placement::new(&transform, pyramid.content, VIEWPORT);

    assert_eq!(placement.scale(), 0.2);
    assert_eq!(placement.device_pixels_per_unit(), 0.4);
    let tiles = image_tiles(&pyramid, &placement);
    // 0.4 device pixels per image pixel is served by the half-size level, in one tile.
    assert_eq!(tiles, vec![(Rect::new(0., 0., 800., 600.), &(2000, 1500))]);
}

#[test]
fn a_zoomed_image_draws_only_the_tiles_in_view_and_they_touch() {
    let pyramid = pyramid(5000, 3000);
    let mut transform = ViewTransform::new(ZoomMode::Manual(1.));
    transform.scroll_by(Offset::new(1700.3, 1800.7), pyramid.content, VIEWPORT.size);
    let placement = Placement::new(&transform, pyramid.content, VIEWPORT);

    let tiles = image_tiles(&pyramid, &placement);
    assert_eq!(tiles.len(), 4);
    let rects: Vec<Rect> = tiles.iter().map(|(rect, _)| *rect).collect();
    // Shared edges get the same coordinate, on a device pixel.
    assert_eq!(rects[0].right(), rects[1].x);
    assert_eq!(rects[0].bottom(), rects[2].y);
    assert_eq!(rects[1].bottom(), rects[3].y);
    assert_eq!((rects[0].right() * 2.).fract(), 0.);
    assert_eq!((rects[0].bottom() * 2.).fract(), 0.);
}

#[test]
fn a_small_image_is_centred_at_its_natural_size() {
    let pyramid = pyramid(100, 50);
    let placement = Placement::new(
        &ViewTransform::new(ZoomMode::Fit),
        pyramid.content,
        VIEWPORT,
    );
    let tiles = image_tiles(&pyramid, &placement);
    assert_eq!(tiles, vec![(Rect::new(350., 275., 100., 50.), &(100, 50))]);
}

#[test]
fn the_backdrop_covers_the_image_and_nothing_else() {
    let area = Rect::new(350., 275., 100., 50.);
    let backdrop = backdrop(area, VIEWPORT.size, 64.);

    assert_eq!(
        backdrop.tiles,
        vec![
            Rect::new(350., 275., 64., 64.),
            Rect::new(414., 275., 64., 64.),
        ]
    );
    // The second tile sticks out to the right, and both stick out below.
    assert_eq!(
        backdrop.cover,
        vec![
            Rect::new(450., 275., 64., 114.),
            Rect::new(350., 325., 100., 64.),
        ]
    );
}

#[test]
fn the_backdrop_of_a_scrolled_image_starts_at_the_tiles_in_view() {
    // An image far larger than the viewport, scrolled.
    let area = Rect::new(-1000., -500., 4000., 3000.);
    let backdrop = backdrop(area, VIEWPORT.size, 256.);

    let first = backdrop.tiles[0];
    assert_eq!(first, Rect::new(-232., -244., 256., 256.));
    assert!(backdrop.tiles.len() <= 5 * 4);
    assert!(backdrop.tiles.iter().all(|tile| tile.intersects(Rect::new(
        0.,
        0.,
        VIEWPORT.size.width,
        VIEWPORT.size.height
    ))));
    assert!(backdrop.cover.is_empty());

    let off_screen = Rect::new(900., 0., 100., 100.);
    assert_eq!(
        super::backdrop(off_screen, VIEWPORT.size, 64.),
        Backdrop::default()
    );
}

fn two_page_layout() -> PageLayout {
    let page = PageGeometry {
        width: 612.,
        height: 792.,
    };
    PageLayout::new(&[page, page, page, page])
}

#[test]
fn strips_are_wanted_for_the_view_and_a_margin_around_it() {
    let layout = two_page_layout();
    let transform = ViewTransform::new(ZoomMode::FitWidth);

    // Fit width shows 636 points across 800 pixels: about 477 points of height in view.
    let strips = wanted_strips(&layout, &transform, VIEWPORT, 0.);
    assert_eq!(strips.len(), 1);
    assert_eq!((strips[0].key.page, strips[0].key.strip), (0, 0));
    let expected_scale = pdf::raster_scale(800. / 636., 2., 612.);
    assert_eq!(strips[0].scale, expected_scale);
    assert_eq!(strips[0].pixel_width, (612. * expected_scale).ceil() as u32);

    let with_margin = wanted_strips(&layout, &transform, VIEWPORT, 1.);
    let pages: Vec<u32> = with_margin.iter().map(|strip| strip.key.page).collect();
    assert_eq!(pages, vec![0, 1]);
}

#[test]
fn strips_in_view_come_before_those_near_it() {
    let layout = two_page_layout();
    let mut transform = ViewTransform::new(ZoomMode::FitWidth);
    transform.scroll_to_content_y(820., layout.content_size(), VIEWPORT.size);

    let strips = wanted_strips(&layout, &transform, VIEWPORT, 1.);
    let pages: Vec<u32> = strips.iter().map(|strip| strip.key.page).collect();
    assert_eq!(pages[0], 1);
    assert!(pages.contains(&0));
    assert!(pages.contains(&2));
}

#[test]
fn a_raster_goes_stale_when_the_zoom_changes() {
    let layout = two_page_layout();
    let fitted = ViewTransform::new(ZoomMode::FitWidth);
    let key = wanted_strips(&layout, &fitted, VIEWPORT, 0.)[0].key;

    assert!(is_at_wanted_scale(&key, &layout, &fitted, VIEWPORT));
    let zoomed = ViewTransform::new(ZoomMode::Manual(3.));
    assert!(!is_at_wanted_scale(&key, &layout, &zoomed, VIEWPORT));
    let other_display = Viewport {
        device_scale: 1.,
        ..VIEWPORT
    };
    assert!(!is_at_wanted_scale(&key, &layout, &fitted, other_display));
}

#[test]
fn paper_is_drawn_for_the_pages_in_view() {
    let layout = two_page_layout();
    let transform = ViewTransform::new(ZoomMode::Manual(1.));
    let placement = Placement::new(&transform, layout.content_size(), VIEWPORT);

    let pages = page_rects(&layout, &placement);
    assert_eq!(pages, vec![Rect::new(94., 12., 612., 792.)]);
}
