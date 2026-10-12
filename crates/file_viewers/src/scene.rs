//! What a viewer draws for a given view of its content, as rectangles in viewport coordinates.

use crate::geometry::{Offset, Rect, Size, ViewTransform, snap_to_device_pixels};
use crate::image::Pyramid;
use crate::pdf::{self, PageLayout, Strip, StripKey};

/// Where a viewer is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Size in logical pixels.
    pub size: Size,
    /// Device pixels per logical pixel.
    pub device_scale: f32,
}

impl Viewport {
    /// A plausible viewport for work that starts before the viewer has been drawn once.
    pub const ASSUMED: Self = Self {
        size: Size::new(1200., 900.),
        device_scale: 2.,
    };
}

/// Maps content to a viewport for one state of the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    scale: f32,
    origin: Offset,
    viewport: Viewport,
    visible: Rect,
}

impl Placement {
    pub fn new(transform: &ViewTransform, content: Size, viewport: Viewport) -> Self {
        Self {
            scale: transform.scale(content, viewport.size),
            origin: transform.content_origin(content, viewport.size),
            viewport,
            visible: transform.visible_content(content, viewport.size),
        }
    }

    /// Logical pixels per content unit.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Device pixels per content unit.
    pub fn device_pixels_per_unit(&self) -> f32 {
        self.scale * self.viewport.device_scale
    }

    /// The part of the content in view, in content units.
    pub fn visible(&self) -> Rect {
        self.visible
    }

    /// Where `rect`, given in content units, lands in the viewport. Edges are put on device
    /// pixels, so rectangles that touch in the content touch on screen.
    pub fn place(&self, rect: Rect) -> Rect {
        let snap = |value: f32| snap_to_device_pixels(value, self.viewport.device_scale);
        let left = snap(self.origin.x + rect.x * self.scale);
        let top = snap(self.origin.y + rect.y * self.scale);
        let right = snap(self.origin.x + rect.right() * self.scale);
        let bottom = snap(self.origin.y + rect.bottom() * self.scale);
        Rect::new(left, top, right - left, bottom - top)
    }
}

/// The tiles of `pyramid` to draw and where, from the level that suits the scale of
/// `placement`.
pub fn image_tiles<'a, T>(pyramid: &'a Pyramid<T>, placement: &Placement) -> Vec<(Rect, &'a T)> {
    pyramid
        .visible_tiles(placement.device_pixels_per_unit(), placement.visible())
        .map(|tile| (placement.place(tile.rect), &tile.image))
        .collect()
}

/// A repeating pattern drawn behind an image.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Backdrop {
    /// Where to draw the pattern tile. Tiles in the last column and row stick out of the area.
    pub tiles: Vec<Rect>,
    /// What to paint over with the background colour to hide the parts that stick out.
    pub cover: Vec<Rect>,
}

/// Lays square tiles of side `tile` over the part of `area` that is inside a viewport of size
/// `viewport`. The pattern is anchored to the corner of `area`, so it moves with the image.
pub fn backdrop(area: Rect, viewport: Size, tile: f32) -> Backdrop {
    let left = area.x.max(0.);
    let top = area.y.max(0.);
    let right = area.right().min(viewport.width);
    let bottom = area.bottom().min(viewport.height);
    if tile <= 0. || right <= left || bottom <= top {
        return Backdrop::default();
    }

    let first_column = ((left - area.x) / tile).floor() as usize;
    let first_row = ((top - area.y) / tile).floor() as usize;
    let end_column = ((right - area.x) / tile).ceil() as usize;
    let end_row = ((bottom - area.y) / tile).ceil() as usize;
    let mut tiles = Vec::new();
    for row in first_row..end_row {
        for column in first_column..end_column {
            tiles.push(Rect::new(
                area.x + column as f32 * tile,
                area.y + row as f32 * tile,
                tile,
                tile,
            ));
        }
    }

    let mut cover = Vec::new();
    if area.right() < viewport.width {
        cover.push(Rect::new(area.right(), top, tile, bottom - top + tile));
    }
    if area.bottom() < viewport.height {
        cover.push(Rect::new(left, area.bottom(), right - left, tile));
    }
    Backdrop { tiles, cover }
}

/// The strips of a document to have rasterised for the view `transform` gives of it: the ones
/// in view and those within `overscan` viewport heights above and below.
pub fn wanted_strips(
    layout: &PageLayout,
    transform: &ViewTransform,
    viewport: Viewport,
    overscan: f32,
) -> Vec<Strip> {
    let content = layout.content_size();
    let view_scale = transform.scale(content, viewport.size);
    let visible = transform.visible_content(content, viewport.size);
    let area = pdf::with_overscan(visible, overscan, content);

    let mut strips = Vec::new();
    for page in layout.pages_in(area) {
        let Some(page_rect) = layout.page_rect(page) else {
            continue;
        };
        let scale = pdf::raster_scale(view_scale, viewport.device_scale, page_rect.width);
        strips.extend(pdf::strips_for_page(layout, page, scale, area));
    }
    // Rasterise what is on screen before what is only near it.
    strips.sort_by_key(|strip| !strip.rect.intersects(visible));
    strips
}

/// Whether the raster `key` identifies is at the scale the view `transform` gives calls for.
pub fn is_at_wanted_scale(
    key: &StripKey,
    layout: &PageLayout,
    transform: &ViewTransform,
    viewport: Viewport,
) -> bool {
    let Some(page_rect) = layout.page_rect(key.page as usize) else {
        return false;
    };
    let view_scale = transform.scale(layout.content_size(), viewport.size);
    key.is_at_scale(pdf::raster_scale(
        view_scale,
        viewport.device_scale,
        page_rect.width,
    ))
}

/// Where the pages in view land in the viewport, for drawing the paper under their rasters.
pub fn page_rects(layout: &PageLayout, placement: &Placement) -> Vec<Rect> {
    layout
        .pages_in(placement.visible())
        .filter_map(|page| layout.page_rect(page))
        .map(|rect| placement.place(rect))
        .collect()
}

#[cfg(test)]
#[path = "scene_tests.rs"]
mod tests;
