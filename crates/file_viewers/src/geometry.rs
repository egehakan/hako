//! Zoom, fit and scroll maths shared by the viewers.
//!
//! Content is measured in its own units (image pixels, PDF points) and shown at a scale of
//! logical pixels per content unit inside a viewport measured in logical pixels.

/// Zoom levels the zoom in and zoom out commands step through.
const ZOOM_STEPS: &[f32] = &[
    0.05, 0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1., 1.25, 1.5, 2., 3., 4., 6., 8., 12., 16., 32.,
];

pub const MIN_SCALE: f32 = 0.02;
pub const MAX_SCALE: f32 = 64.;

/// How fast a zoom gesture changes the scale, per logical pixel of wheel travel.
const WHEEL_ZOOM_RATE: f32 = 0.0045;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    pub fn is_empty(self) -> bool {
        self.width <= 0. || self.height <= 0.
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Offset {
    pub x: f32,
    pub y: f32,
}

impl Offset {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    pub fn intersects(self, other: Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZoomMode {
    /// The whole content is visible. Content smaller than the viewport keeps its natural size.
    Fit,
    /// The content is as wide as the viewport.
    FitWidth,
    /// A fixed scale, in logical pixels per content unit.
    Manual(f32),
}

/// Where the content sits in the viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewTransform {
    pub mode: ZoomMode,
    /// Distance from the top-left corner of the scaled content to that of the viewport, in
    /// logical pixels. Ignored along an axis on which the content fits.
    pub scroll: Offset,
}

impl ViewTransform {
    pub fn new(mode: ZoomMode) -> Self {
        Self {
            mode,
            scroll: Offset::default(),
        }
    }

    /// Logical pixels per content unit.
    pub fn scale(&self, content: Size, viewport: Size) -> f32 {
        if content.is_empty() || viewport.is_empty() {
            return match self.mode {
                ZoomMode::Manual(scale) => scale.clamp(MIN_SCALE, MAX_SCALE),
                ZoomMode::Fit | ZoomMode::FitWidth => 1.,
            };
        }
        let scale = match self.mode {
            ZoomMode::Fit => (viewport.width / content.width)
                .min(viewport.height / content.height)
                .min(1.),
            ZoomMode::FitWidth => viewport.width / content.width,
            ZoomMode::Manual(scale) => scale,
        };
        scale.clamp(MIN_SCALE, MAX_SCALE)
    }

    fn max_scroll(&self, content: Size, viewport: Size) -> Offset {
        let scale = self.scale(content, viewport);
        Offset::new(
            (content.width * scale - viewport.width).max(0.),
            (content.height * scale - viewport.height).max(0.),
        )
    }

    /// The scroll offset kept inside the content.
    pub fn clamped_scroll(&self, content: Size, viewport: Size) -> Offset {
        let max = self.max_scroll(content, viewport);
        Offset::new(
            self.scroll.x.clamp(0., max.x),
            self.scroll.y.clamp(0., max.y),
        )
    }

    /// Position of the content's top-left corner relative to the viewport's. Content smaller
    /// than the viewport along an axis is centred on it.
    pub fn content_origin(&self, content: Size, viewport: Size) -> Offset {
        let scale = self.scale(content, viewport);
        let scroll = self.clamped_scroll(content, viewport);
        let along = |content: f32, viewport: f32, scroll: f32| {
            let scaled = content * scale;
            if scaled < viewport {
                (viewport - scaled) / 2.
            } else {
                -scroll
            }
        };
        Offset::new(
            along(content.width, viewport.width, scroll.x),
            along(content.height, viewport.height, scroll.y),
        )
    }

    /// The part of the content inside the viewport, in content units.
    pub fn visible_content(&self, content: Size, viewport: Size) -> Rect {
        let scale = self.scale(content, viewport);
        let origin = self.content_origin(content, viewport);
        let left = (-origin.x / scale).clamp(0., content.width);
        let top = (-origin.y / scale).clamp(0., content.height);
        let right = ((viewport.width - origin.x) / scale).clamp(0., content.width);
        let bottom = ((viewport.height - origin.y) / scale).clamp(0., content.height);
        Rect::new(left, top, right - left, bottom - top)
    }

    /// Moves the view by `delta` logical pixels, stopping at the edges of the content.
    pub fn scroll_by(&mut self, delta: Offset, content: Size, viewport: Size) {
        let scroll = self.clamped_scroll(content, viewport);
        self.scroll = Offset::new(scroll.x + delta.x, scroll.y + delta.y);
        self.scroll = self.clamped_scroll(content, viewport);
    }

    /// Scrolls so that the content coordinate `y` is at the top of the viewport.
    pub fn scroll_to_content_y(&mut self, y: f32, content: Size, viewport: Size) {
        self.scroll.y = y * self.scale(content, viewport);
        self.scroll = self.clamped_scroll(content, viewport);
    }

    /// Changes the scale to `scale`, keeping the content under `anchor` (a point of the
    /// viewport) where it is.
    pub fn zoom_to(&mut self, scale: f32, anchor: Offset, content: Size, viewport: Size) {
        let old_scale = self.scale(content, viewport);
        let origin = self.content_origin(content, viewport);
        let anchored = Offset::new(
            (anchor.x - origin.x) / old_scale,
            (anchor.y - origin.y) / old_scale,
        );

        let scale = scale.clamp(MIN_SCALE, MAX_SCALE);
        self.mode = ZoomMode::Manual(scale);
        self.scroll = Offset::new(anchored.x * scale - anchor.x, anchored.y * scale - anchor.y);
        self.scroll = self.clamped_scroll(content, viewport);
    }

    /// Multiplies the scale by `factor` around `anchor`.
    pub fn zoom_by(&mut self, factor: f32, anchor: Offset, content: Size, viewport: Size) {
        let scale = self.scale(content, viewport) * factor;
        self.zoom_to(scale, anchor, content, viewport);
    }

    /// Switches to `mode`, keeping the same content at the top of the viewport when the scale
    /// changes.
    pub fn set_mode(&mut self, mode: ZoomMode, content: Size, viewport: Size) {
        let top = self.visible_content(content, viewport).y;
        self.mode = mode;
        self.scroll.x = 0.;
        self.scroll_to_content_y(top, content, viewport);
    }
}

/// The next zoom step above (`zoom_in`) or below `scale`.
pub fn next_zoom_step(scale: f32, zoom_in: bool) -> f32 {
    // Steps within a rounding error of the current scale count as the current one.
    const TOLERANCE: f32 = 1.005;
    let step = if zoom_in {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|step| *step > scale * TOLERANCE)
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step * TOLERANCE < scale)
    };
    step.unwrap_or(scale).clamp(MIN_SCALE, MAX_SCALE)
}

/// Scale factor for a zoom gesture that moved the wheel by `delta` logical pixels. Scrolling
/// up zooms in.
pub fn wheel_zoom_factor(delta: f32) -> f32 {
    (delta * WHEEL_ZOOM_RATE).clamp(-1., 1.).exp()
}

/// How far a notch of a wheel that reports lines scrolls, in logical pixels.
const WHEEL_LINE: f32 = 40.;

/// The distance to scroll for a wheel event, in logical pixels to the right and down. A wheel
/// reports how the content should move, which is the opposite way, and a wheel without
/// precise deltas reports lines.
pub fn wheel_scroll(delta: Offset, precise: bool) -> Offset {
    let unit = if precise { 1. } else { WHEEL_LINE };
    Offset::new(-delta.x * unit, -delta.y * unit)
}

/// Rounds a logical coordinate to the device pixel grid. Tiles that share an edge get the same
/// rounded coordinate for it, so no gap opens between them.
pub fn snap_to_device_pixels(value: f32, device_scale: f32) -> f32 {
    if device_scale <= 0. {
        return value;
    }
    (value * device_scale).round() / device_scale
}

/// A scale shown as a percentage, e.g. `125%`.
pub fn format_zoom(scale: f32) -> String {
    let percent = scale * 100.;
    if percent < 10. {
        format!("{percent:.1}%")
    } else {
        format!("{percent:.0}%")
    }
}

#[cfg(test)]
#[path = "geometry_tests.rs"]
mod tests;
