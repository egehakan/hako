//! The element a viewer draws its content into and takes its mouse input from.

use std::sync::Arc;
use std::time::Duration;

use file_viewers::geometry::Size;
use file_viewers::scene::Viewport;
use parking_lot::Mutex;
use pathfinder_color::ColorU;
use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::Vector2F;
use warpui::elements::{CornerRadius, Point, ZIndex};
use warpui::event::DispatchedEvent;
use warpui::image_cache::StaticImage;
use warpui::{
    AfterLayoutContext, AppContext, ClipBounds, Element, Event, EventContext, LayoutContext,
    PaintContext, SizeConstraint,
};

use crate::FileViewerAction;

/// Size taken along an axis on which the parent sets no limit.
const UNBOUNDED_AXIS_SIZE: f32 = 480.;

#[derive(Default)]
struct CanvasState {
    viewport: Option<Viewport>,
    /// The viewport the view was last told about.
    reported_viewport: Option<Viewport>,
    /// Position of the mouse at the last event of a drag that started on the canvas.
    drag_position: Option<Vector2F>,
}

/// State of a canvas that outlives the elements built for each frame. The viewer creates one
/// and hands a clone to every canvas it renders.
#[derive(Clone, Default)]
pub(crate) struct CanvasHandle(Arc<Mutex<CanvasState>>);

impl CanvasHandle {
    /// Where the canvas was last drawn, once it has been.
    pub(crate) fn viewport(&self) -> Option<Viewport> {
        self.0.lock().viewport
    }
}

/// Something to draw, positioned relative to the top-left corner of the canvas in logical
/// pixels.
pub(crate) enum Shape {
    Fill {
        rect: RectF,
        color: ColorU,
    },
    Image {
        rect: RectF,
        image: Arc<StaticImage>,
    },
}

/// A pattern image repeated behind the shapes of a frame and cut off at the edge of `clip`.
pub(crate) struct BackdropTiles {
    pub clip: RectF,
    pub tiles: Vec<RectF>,
    pub image: Arc<StaticImage>,
}

/// What a canvas shows for one paint.
#[derive(Default)]
pub(crate) struct Frame {
    pub backdrop: Option<BackdropTiles>,
    pub shapes: Vec<Shape>,
    /// When to paint again without being asked, for content that changes with time.
    pub repaint_after: Option<Duration>,
}

type Painter = Box<dyn Fn(Viewport) -> Frame>;

/// Fills the space it is given, clips to it, and draws what its painter returns for the size
/// it ended up with. The painter runs at paint time because that is the first moment the size
/// is known, and fitting content to the view depends on it.
pub(crate) struct ViewerCanvas {
    handle: CanvasHandle,
    background: ColorU,
    painter: Option<Painter>,
    child: Option<Box<dyn Element>>,
    size: Option<Vector2F>,
    origin: Option<Point>,
    /// Highest z-index painted by this element, which is the one its own content must not be
    /// considered covered at.
    top_z_index: Option<ZIndex>,
}

impl ViewerCanvas {
    pub(crate) fn new(handle: CanvasHandle, background: ColorU) -> Self {
        Self {
            handle,
            background,
            painter: None,
            child: None,
            size: None,
            origin: None,
            top_z_index: None,
        }
    }

    pub(crate) fn with_painter(mut self, painter: impl Fn(Viewport) -> Frame + 'static) -> Self {
        self.painter = Some(Box::new(painter));
        self
    }

    /// Lays `child` out over the whole canvas, above what the painter draws.
    pub(crate) fn with_child(mut self, child: Box<dyn Element>) -> Self {
        self.child = Some(child);
        self
    }
}

fn is_drawable(rect: &RectF) -> bool {
    let finite = |vector: Vector2F| vector.x().is_finite() && vector.y().is_finite();
    finite(rect.origin()) && finite(rect.size()) && rect.width() > 0. && rect.height() > 0.
}

impl Element for ViewerCanvas {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let along = |max: f32, min: f32| {
            if max.is_finite() {
                max
            } else {
                min.max(UNBOUNDED_AXIS_SIZE)
            }
        };
        let size = Vector2F::new(
            along(constraint.max.x(), constraint.min.x()),
            along(constraint.max.y(), constraint.min.y()),
        );
        if let Some(child) = self.child.as_mut() {
            child.layout(SizeConstraint::new(Vector2F::zero(), size), ctx, app);
        }
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        if let Some(child) = self.child.as_mut() {
            child.after_layout(ctx, app);
        }
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let Some(size) = self.size else {
            return;
        };
        let origin_point = Point::from_vec2f(origin, ctx.scene.z_index());
        self.origin = Some(origin_point);
        let viewport = Viewport {
            size: Size::new(size.x(), size.y()),
            device_scale: ctx.scene.scale_factor(),
        };
        self.handle.0.lock().viewport = Some(viewport);

        let bounds = RectF::new(origin, size);
        let Some(visible) = ctx.scene.visible_rect(origin_point, size) else {
            return;
        };
        if !is_drawable(&bounds) {
            return;
        }
        ctx.scene.start_layer(ClipBounds::BoundedBy(visible));
        ctx.scene
            .draw_rect_with_hit_recording(bounds)
            .with_background(self.background);

        let frame = self.painter.as_ref().map(|painter| painter(viewport));
        // Within a layer images are drawn over every rectangle, so the tiles cannot be trimmed
        // by painting over them. They get a clipped layer of their own instead.
        if let Some(backdrop) = frame.as_ref().and_then(|frame| frame.backdrop.as_ref()) {
            let clip = RectF::new(backdrop.clip.origin() + origin, backdrop.clip.size());
            if is_drawable(&clip) {
                ctx.scene
                    .start_layer(ClipBounds::BoundedByActiveLayerAnd(clip));
                for tile in &backdrop.tiles {
                    let rect = RectF::new(tile.origin() + origin, tile.size());
                    if is_drawable(&rect) {
                        ctx.scene.draw_image(
                            rect,
                            backdrop.image.clone(),
                            1.,
                            CornerRadius::default(),
                        );
                    }
                }
                ctx.scene.stop_layer();
            }
        }

        // The shapes go in a layer started after the backdrop's, which puts them above it.
        ctx.scene.start_layer(ClipBounds::ActiveLayer);
        if let Some(frame) = frame {
            for shape in frame.shapes {
                match shape {
                    Shape::Fill { rect, color } => {
                        let rect = RectF::new(rect.origin() + origin, rect.size());
                        if is_drawable(&rect) {
                            ctx.scene
                                .draw_rect_with_hit_recording(rect)
                                .with_background(color);
                        }
                    }
                    Shape::Image { rect, image } => {
                        let rect = RectF::new(rect.origin() + origin, rect.size());
                        if is_drawable(&rect) {
                            ctx.scene
                                .draw_image(rect, image, 1., CornerRadius::default());
                        }
                    }
                }
            }
            if let Some(delay) = frame.repaint_after {
                ctx.repaint_after(delay);
            }
        }

        if let Some(child) = self.child.as_mut() {
            child.paint(origin, ctx, app);
        }
        self.top_z_index = Some(ctx.scene.max_active_z_index());
        ctx.scene.stop_layer();
        ctx.scene.stop_layer();
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        let (Some(origin), Some(size), Some(top_z_index)) =
            (self.origin, self.size, self.top_z_index)
        else {
            return false;
        };
        if let Some(child) = self.child.as_mut()
            && child.origin().is_some()
            && child.dispatch_event(event, ctx, app)
        {
            return true;
        }

        // Painting cannot notify the view, so a change of size reaches it with the next event,
        // which for a resize is the mouse moving.
        {
            let mut state = self.handle.0.lock();
            if state.viewport != state.reported_viewport {
                state.reported_viewport = state.viewport;
                ctx.dispatch_typed_action(FileViewerAction::ViewportChanged);
            }
        }

        let Some(event) = event.at_z_index(top_z_index, ctx) else {
            return false;
        };
        let visible = ctx.visible_rect(origin, size);
        let contains =
            |position: Vector2F| visible.is_some_and(|visible| visible.contains_point(position));

        if let Event::ScrollWheel {
            position,
            delta,
            precise,
            modifiers,
        } = event
            && contains(*position)
        {
            ctx.dispatch_typed_action(FileViewerAction::Wheel {
                position: *position - origin.xy(),
                delta: *delta,
                precise: *precise,
                zoom: modifiers.cmd || modifiers.ctrl,
            });
            return true;
        }
        if let Event::LeftMouseDown {
            position,
            click_count,
            ..
        } = event
            && contains(*position)
        {
            self.handle.0.lock().drag_position = Some(*position);
            ctx.dispatch_typed_action(FileViewerAction::Pressed {
                position: *position - origin.xy(),
                click_count: *click_count,
            });
            return true;
        }
        if let Event::LeftMouseDragged { position, .. } = event {
            // A drag that started outside the canvas has no previous position and is not ours.
            let mut state = self.handle.0.lock();
            if let Some(previous) = state.drag_position {
                state.drag_position = Some(*position);
                ctx.dispatch_typed_action(FileViewerAction::Dragged {
                    delta: previous - *position,
                });
                return true;
            }
        }
        if let Event::LeftMouseUp { .. } = event {
            self.handle.0.lock().drag_position = None;
        }
        false
    }
}
