//! The lanes of a history graph as they cross one row of the source control overview.

use ::hako_remote::git_graph::{GraphRow, LaneKind, Link};
use pathfinder_color::ColorU;
use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use warpui::elements::{Border, CornerRadius, Fill, Point, Radius};
use warpui::event::DispatchedEvent;
use warpui::{
    AfterLayoutContext, AppContext, Element, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};

pub const LANE_WIDTH: f32 = 12.;
const LINE_WIDTH: f32 = 2.;
const DOT_SIZE: f32 = 8.;
const HOLLOW_DOT_BORDER: f32 = 2.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaneColors {
    pub current: ColorU,
    pub upstream: ColorU,
    pub other: ColorU,
    /// What the rows are drawn on, which is what shows inside a hollow dot.
    pub background: ColorU,
}

impl LaneColors {
    pub fn of(&self, kind: LaneKind) -> ColorU {
        match kind {
            LaneKind::Current => self.current,
            LaneKind::Upstream => self.upstream,
            LaneKind::Other => self.other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DotStyle {
    Filled,
    /// A ring, for a commit that the checked-out branch does not have yet.
    Hollow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Dot {
    lane: usize,
    kind: LaneKind,
    style: DotStyle,
}

/// A straight piece of a lane, in lanes across and in halves of the row down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Stroke {
    /// Down the middle of `lane`, over the top half of the row or the bottom one.
    Vertical {
        lane: usize,
        is_top_half: bool,
        kind: LaneKind,
    },
    /// Along the middle of the row, between the middles of two lanes.
    Horizontal {
        from: usize,
        to: usize,
        kind: LaneKind,
    },
}

impl Stroke {
    fn kind(&self) -> LaneKind {
        match self {
            Stroke::Vertical { kind, .. } | Stroke::Horizontal { kind, .. } => *kind,
        }
    }
}

/// The strokes that draw a row's links, the lines of the current branch last so that they lie
/// on top where lines cross.
pub(super) fn strokes(above: &[Link], below: &[Link]) -> Vec<Stroke> {
    let mut strokes = Vec::new();
    for link in above {
        strokes.push(Stroke::Vertical {
            lane: link.from,
            is_top_half: true,
            kind: link.kind,
        });
        if link.from != link.to {
            strokes.push(Stroke::Horizontal {
                from: link.from.min(link.to),
                to: link.from.max(link.to),
                kind: link.kind,
            });
        }
    }
    for link in below {
        if link.from != link.to {
            strokes.push(Stroke::Horizontal {
                from: link.from.min(link.to),
                to: link.from.max(link.to),
                kind: link.kind,
            });
        }
        strokes.push(Stroke::Vertical {
            lane: link.to,
            is_top_half: false,
            kind: link.kind,
        });
    }
    strokes.sort_by_key(Stroke::kind);
    strokes
}

pub struct GraphLanes {
    strokes: Vec<Stroke>,
    dot: Option<Dot>,
    colors: LaneColors,
    /// Lanes further right than this many are left out.
    max_lanes: usize,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl GraphLanes {
    /// The row of a commit.
    pub fn commit(row: &GraphRow, style: DotStyle, colors: LaneColors, max_lanes: usize) -> Self {
        Self {
            strokes: strokes(&row.above, &row.below),
            dot: Some(Dot {
                lane: row.column,
                kind: row.kind,
                style,
            }),
            colors,
            max_lanes,
            size: None,
            origin: None,
        }
    }

    /// A row between two commits, which every one of `lanes` passes straight through.
    pub fn passing(lanes: &[LaneKind], colors: LaneColors, max_lanes: usize) -> Self {
        let links: Vec<Link> = lanes
            .iter()
            .enumerate()
            .map(|(lane, kind)| Link {
                from: lane,
                to: lane,
                kind: *kind,
            })
            .collect();
        Self {
            strokes: strokes(&links, &links),
            dot: None,
            colors,
            max_lanes,
            size: None,
            origin: None,
        }
    }

    fn lane_middle(lane: usize) -> f32 {
        (LANE_WIDTH * (lane as f32 + 0.5)).floor()
    }

    fn stroke_rect(&self, stroke: Stroke, origin: Vector2F, size: Vector2F) -> Option<RectF> {
        let middle = (size.y() / 2.).floor();
        let half_line = LINE_WIDTH / 2.;
        match stroke {
            Stroke::Vertical {
                lane, is_top_half, ..
            } => {
                if lane >= self.max_lanes {
                    return None;
                }
                let (top, bottom) = if is_top_half {
                    (0., middle + half_line)
                } else {
                    (middle - half_line, size.y())
                };
                Some(RectF::new(
                    origin + vec2f(Self::lane_middle(lane) - half_line, top),
                    vec2f(LINE_WIDTH, bottom - top),
                ))
            }
            Stroke::Horizontal { from, to, .. } => {
                if from >= self.max_lanes {
                    return None;
                }
                let left = Self::lane_middle(from) - half_line;
                let right = if to >= self.max_lanes {
                    size.x()
                } else {
                    Self::lane_middle(to) + half_line
                };
                Some(RectF::new(
                    origin + vec2f(left, middle - half_line),
                    vec2f(right - left, LINE_WIDTH),
                ))
            }
        }
    }
}

impl Element for GraphLanes {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _: &mut LayoutContext,
        _: &AppContext,
    ) -> Vector2F {
        let size = constraint.max;
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, _: &mut AfterLayoutContext, _: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let Some(size) = self.size else {
            return;
        };
        for stroke in &self.strokes {
            if let Some(rect) = self.stroke_rect(*stroke, origin, size) {
                ctx.scene
                    .draw_rect_with_hit_recording(rect)
                    .with_background(Fill::Solid(self.colors.of(stroke.kind())));
            }
        }
        let Some(dot) = self.dot.filter(|dot| dot.lane < self.max_lanes) else {
            return;
        };
        let color = self.colors.of(dot.kind);
        let center = vec2f(Self::lane_middle(dot.lane), (size.y() / 2.).floor());
        let rect = ctx
            .scene
            .draw_rect_with_hit_recording(RectF::new(
                origin + center - vec2f(DOT_SIZE / 2., DOT_SIZE / 2.),
                vec2f(DOT_SIZE, DOT_SIZE),
            ))
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)));
        match dot.style {
            DotStyle::Filled => {
                rect.with_background(Fill::Solid(color));
            }
            DotStyle::Hollow => {
                rect.with_background(Fill::Solid(self.colors.background))
                    .with_border(Border::all(HOLLOW_DOT_BORDER).with_border_color(color));
            }
        }
    }

    fn dispatch_event(
        &mut self,
        _: &DispatchedEvent,
        _: &mut EventContext,
        _: &AppContext,
    ) -> bool {
        false
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

#[cfg(test)]
#[path = "graph_lanes_tests.rs"]
mod tests;
