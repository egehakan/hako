use std::any::Any;

use warpui::elements::Point;
use warpui::event::DispatchedEvent;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::Vector2F;
use warpui::{
    AfterLayoutContext, AppContext, Element, Event, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};

use super::{FileTreeAction, FileTreeIdentifier};

/// Wraps a part of the file tree so that files dragged in from another app can be dropped on it.
pub(super) struct ExternalFileDropTarget {
    child: Box<dyn Element>,
    /// The item the wrapped element shows, or `None` for the tree as a whole, which takes the
    /// drops that land on none of its items.
    target: Option<FileTreeIdentifier>,
}

impl ExternalFileDropTarget {
    pub(super) fn new(child: Box<dyn Element>, target: Option<FileTreeIdentifier>) -> Self {
        Self { child, target }
    }

    fn contains(&self, position: Vector2F) -> bool {
        self.bounds()
            .is_some_and(|bounds| bounds.contains_point(position))
    }
}

impl Element for ExternalFileDropTarget {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        self.child.layout(constraint, ctx, app)
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.child.paint(origin, ctx, app);
    }

    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }

    fn bounds(&self) -> Option<RectF> {
        self.child.bounds()
    }

    fn parent_data(&self) -> Option<&dyn Any> {
        self.child.parent_data()
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        let handled_by_child = self.child.dispatch_event(event, ctx, app);
        if handled_by_child {
            return true;
        }

        match event.raw_event() {
            Event::DragFiles { location } if self.contains(*location) => {
                ctx.dispatch_typed_action(FileTreeAction::DragOverItem {
                    target: self.target.clone(),
                });
                true
            }
            Event::DragFileExit if self.target.is_none() => {
                ctx.dispatch_typed_action(FileTreeAction::DragOverItem { target: None });
                false
            }
            Event::DragAndDropFiles { paths, location }
                if self.contains(*location) && !paths.is_empty() =>
            {
                ctx.dispatch_typed_action(FileTreeAction::ExternalFilesDropped {
                    paths: paths.clone(),
                    target: self.target.clone(),
                });
                true
            }
            _ => false,
        }
    }
}
