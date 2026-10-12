//! Laying out and rasterising the pages of a PDF document.
//!
//! Pages are stacked in one column, measured in points. Each page is rasterised in horizontal
//! strips, so that a page taller than a texture can be (a document converted from a web page
//! is a single very tall one) still draws, and only the visible part of it is in memory.

use std::ops::Range;

use thiserror::Error;

use crate::geometry::{Rect, Size};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::PdfWorker;
#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
pub use unsupported::PdfWorker;

/// Space between pages and around the column of pages, in points.
pub const PAGE_GAP: f32 = 12.;

/// Height of a strip, in device pixels.
pub const STRIP_HEIGHT: u32 = 2048;

/// Width of the widest raster of a page, in device pixels. This is the texture size limit of
/// the renderer, and what bounds how far zooming in stays sharp.
pub const MAX_RASTER_WIDTH: f32 = 8192.;

/// Raster scales are compared in steps of this fraction of a pixel per point.
const SCALE_KEY_STEPS: f32 = 256.;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PdfError {
    #[error("The file is not a PDF document or is damaged.")]
    Unreadable,
    #[error("The document is protected by a password.")]
    PasswordRequired,
    #[error("The password is not the right one for this document.")]
    WrongPassword,
    #[error("The document has no pages.")]
    Empty,
    #[error("PDF documents cannot be shown on this platform.")]
    Unsupported,
    #[error("The page could not be drawn.")]
    RenderFailed,
    #[error("The request was cancelled.")]
    Cancelled,
}

/// Size of a page as it is shown, in points: its crop box after its rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageGeometry {
    pub width: f32,
    pub height: f32,
}

/// A PDF document opened for rasterising.
pub struct OpenedPdf {
    pub worker: PdfWorker,
    pub pages: Vec<PageGeometry>,
}

/// Where each page sits in the column of pages.
#[derive(Clone, Debug, PartialEq)]
pub struct PageLayout {
    pages: Vec<Rect>,
    content: Size,
}

impl PageLayout {
    pub fn new(pages: &[PageGeometry]) -> Self {
        let widest = pages.iter().map(|page| page.width).fold(0., f32::max);
        let width = widest + 2. * PAGE_GAP;
        let mut top = PAGE_GAP;
        let pages = pages
            .iter()
            .map(|page| {
                let rect = Rect::new((width - page.width) / 2., top, page.width, page.height);
                top += page.height + PAGE_GAP;
                rect
            })
            .collect();
        Self {
            pages,
            content: Size::new(width, top),
        }
    }

    pub fn content_size(&self) -> Size {
        self.content
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Where the page at `index` (counted from zero) sits, in points.
    pub fn page_rect(&self, index: usize) -> Option<Rect> {
        self.pages.get(index).copied()
    }

    /// The pages that overlap `visible`.
    pub fn pages_in(&self, visible: Rect) -> Range<usize> {
        let first = self
            .pages
            .partition_point(|page| page.bottom() <= visible.y);
        let end = self.pages.partition_point(|page| page.y < visible.bottom());
        first..end.max(first)
    }

    /// Index of the page a reader of `visible` is on: the one across the middle of the view,
    /// or the next one when the middle falls between two pages.
    pub fn current_page(&self, visible: Rect) -> usize {
        let middle = visible.y + visible.height / 2.;
        self.pages
            .partition_point(|page| page.bottom() <= middle)
            .min(self.pages.len().saturating_sub(1))
    }

    /// The content coordinate to scroll to so that the page at `index` starts at the top of the
    /// view, with the gap above it showing.
    pub fn scroll_target(&self, index: usize) -> Option<f32> {
        self.page_rect(index).map(|page| page.y - PAGE_GAP / 2.)
    }
}

/// Device pixels per point to rasterise a page `page_width` points wide at, for a view showing
/// `view_scale` logical pixels per point on a display with `device_scale` device pixels per
/// logical pixel.
pub fn raster_scale(view_scale: f32, device_scale: f32, page_width: f32) -> f32 {
    let wanted = view_scale * device_scale;
    let sharpest = if page_width > 0. {
        MAX_RASTER_WIDTH / page_width
    } else {
        wanted
    };
    let scale = wanted.min(sharpest).max(1. / SCALE_KEY_STEPS);
    // Rounded so that the scale and its key always agree.
    (scale * SCALE_KEY_STEPS).round() / SCALE_KEY_STEPS
}

/// Identifies the raster of one strip of one page at one scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StripKey {
    pub page: u32,
    pub strip: u32,
    scale_key: u32,
}

impl StripKey {
    /// Whether the strip was rasterised at `scale` device pixels per point.
    pub fn is_at_scale(&self, scale: f32) -> bool {
        self.scale_key == scale_key(scale)
    }
}

fn scale_key(scale: f32) -> u32 {
    (scale * SCALE_KEY_STEPS).round() as u32
}

/// A strip of a page to rasterise and where it goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strip {
    pub key: StripKey,
    /// Area of the column of pages the strip covers, in points.
    pub rect: Rect,
    /// Device pixels per point of the raster.
    pub scale: f32,
    /// Distance from the top of the page raster to the top of the strip, in device pixels.
    pub pixel_top: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

impl Strip {
    /// Memory the raster of the strip takes.
    pub fn byte_size(&self) -> usize {
        self.pixel_width as usize * self.pixel_height as usize * 4
    }
}

/// The strips of the page at `page_index` that overlap `visible`, for a raster at `scale`
/// device pixels per point.
pub fn strips_for_page(
    layout: &PageLayout,
    page_index: usize,
    scale: f32,
    visible: Rect,
) -> Vec<Strip> {
    let Some(page) = layout.page_rect(page_index) else {
        return Vec::new();
    };
    let pixel_width = (page.width * scale).ceil().max(1.) as u32;
    let pixel_height = (page.height * scale).ceil().max(1.) as u32;

    (0..pixel_height.div_ceil(STRIP_HEIGHT))
        .map(|strip| {
            let pixel_top = strip * STRIP_HEIGHT;
            let strip_height = STRIP_HEIGHT.min(pixel_height - pixel_top);
            let top = page.y + pixel_top as f32 / scale;
            let bottom = (page.y + (pixel_top + strip_height) as f32 / scale).min(page.bottom());
            Strip {
                key: StripKey {
                    page: page_index as u32,
                    strip,
                    scale_key: scale_key(scale),
                },
                rect: Rect::new(page.x, top, page.width, bottom - top),
                scale,
                pixel_top,
                pixel_width,
                pixel_height: strip_height,
            }
        })
        .filter(|strip| strip.rect.intersects(visible))
        .collect()
}

/// `visible` grown by `factor` of its height above and below, kept inside the content. Strips
/// in the grown area are rasterised ahead of the scroll reaching them.
pub fn with_overscan(visible: Rect, factor: f32, content: Size) -> Rect {
    let extra = visible.height * factor;
    let top = (visible.y - extra).max(0.);
    let bottom = (visible.bottom() + extra).min(content.height);
    Rect::new(0., top, content.width, (bottom - top).max(0.))
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;
