//! The card describing the commit behind the line the cursor is on, shown when the blame after
//! that line is hovered or asked for from the keyboard.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pathfinder_color::ColorU;
use pathfinder_geometry::vector::Vector2F;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::color::internal_colors;
use warp_editor::render::model::LineCount;
use warpui::r#async::Timer;
use warpui::elements::{
    Border, ChildAnchor, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CornerRadius, CrossAxisAlignment, Flex, Hoverable, MouseStateHandle, OffsetPositioning,
    ParentAnchor, ParentElement, ParentOffsetBounds, Radius, Rect, ScrollbarWidth, Text,
};
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{AppContext, Element, SingletonEntity, ViewContext};

use super::blame::{
    BlameModel, UNCOMMITTED_LABEL, absolute_time, author_label, github_commit_url, relative_time,
    short_sha,
};
use super::local_code_editor::{LineBlame, LocalCodeEditorAction, LocalCodeEditorView};

const BLAME_CARD_WIDTH: f32 = 440.;
/// A longer commit message scrolls inside the card.
const BLAME_CARD_MESSAGE_MAX_HEIGHT: f32 = 150.;
/// The room the card needs above its line to be placed there rather than below.
const BLAME_CARD_ROOM_ABOVE: f32 = 290.;
/// How long the mouse has to stay on the blame before its card opens, so that crossing it on the
/// way elsewhere does nothing.
const BLAME_CARD_HOVER_DELAY: Duration = Duration::from_millis(350);
/// How long the mouse may be off both the blame and its card before the card closes, which is
/// what lets it travel from one to the other.
const BLAME_CARD_DISMISS_DELAY: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BlameCardTrigger {
    /// Closes when the mouse leaves the blame and the card.
    Hover,
    /// Stays until it is dismissed, the cursor leaves the line or the text changes.
    Command,
}

pub(super) struct BlameCard {
    line: LineCount,
    blame: LineBlame,
    trigger: BlameCardTrigger,
    /// The whole commit message, once git has printed it. The summary stands in until then.
    message: Option<Arc<str>>,
    copied_sha: bool,
}

/// State of the card's interactive parts that has to outlive a single render.
#[derive(Default)]
pub(super) struct BlameCardHandles {
    card: MouseStateHandle,
    copy_sha: MouseStateHandle,
    open_in_code_review: MouseStateHandle,
    open_on_github: MouseStateHandle,
    message_scroll: ClippedScrollStateHandle,
}

fn is_mouse_over(state: &MouseStateHandle) -> bool {
    state
        .lock()
        .is_ok_and(|state| state.is_mouse_over_element())
}

impl LocalCodeEditorView {
    /// Opens the card for the line the cursor is on. When the blame of that line is still being
    /// worked out, a card asked for from the keyboard opens as soon as it is known.
    pub(super) fn show_blame_card(
        &mut self,
        trigger: BlameCardTrigger,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(inline_blame) = &mut self.inline_blame else {
            return;
        };
        inline_blame.activated = true;
        inline_blame.card_requested = false;
        let Some(line) = self.editor.as_ref(ctx).primary_cursor_line(ctx) else {
            return;
        };
        let Some(blame) = self.blame_for_line(line, ctx) else {
            if let Some(inline_blame) = &mut self.inline_blame {
                inline_blame.card_requested =
                    trigger == BlameCardTrigger::Command && inline_blame.awaiting_blame;
            }
            return;
        };
        let message = Self::blame_commit_message(&blame, ctx);

        if self.lsp_hover_state.clear() {
            ctx.notify();
        }
        if let Some(inline_blame) = &mut self.inline_blame {
            inline_blame.card = Some(BlameCard {
                line,
                blame,
                trigger,
                message,
                copied_sha: false,
            });
            ctx.notify();
        }
    }

    fn blame_commit_message(blame: &LineBlame, ctx: &mut ViewContext<Self>) -> Option<Arc<str>> {
        if blame.commit.is_uncommitted() {
            return None;
        }
        BlameModel::handle(ctx).update(ctx, |model, ctx| {
            model.commit_message(&blame.file.repo_root, &blame.commit.sha, ctx)
        })
    }

    /// Returns whether a card was open.
    pub(super) fn dismiss_blame_card(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        let Some(inline_blame) = &mut self.inline_blame else {
            return false;
        };
        inline_blame.card_requested = false;
        let was_open = inline_blame.card.take().is_some();
        if was_open {
            // Nothing tells the card's element that the mouse is no longer on it once it is
            // gone, and the next card must not start out as hovered.
            if let Ok(mut mouse_state) = inline_blame.card_handles.card.lock() {
                mouse_state.reset_interaction_state();
            }
            ctx.notify();
        }
        was_open
    }

    /// Closes a card that was opened by hovering the blame, for when the blame it hung from is
    /// no longer shown.
    pub(super) fn dismiss_hovered_blame_card(&mut self, ctx: &mut ViewContext<Self>) {
        let is_hover_card = self
            .inline_blame
            .as_ref()
            .and_then(|inline_blame| inline_blame.card.as_ref())
            .is_some_and(|card| card.trigger == BlameCardTrigger::Hover);
        if is_hover_card {
            self.dismiss_blame_card(ctx);
        }
    }

    /// Closes the card when the cursor has left the line it describes.
    pub(super) fn dismiss_blame_card_off_line(&mut self, ctx: &mut ViewContext<Self>) {
        let cursor_line = self.editor.as_ref(ctx).primary_cursor_line(ctx);
        let is_off_line = self
            .inline_blame
            .as_ref()
            .and_then(|inline_blame| inline_blame.card.as_ref())
            .is_some_and(|card| Some(card.line) != cursor_line);
        if is_off_line {
            self.dismiss_blame_card(ctx);
        }
    }

    /// Brings the card up to date with what the blame model has learned: opens the one that was
    /// waiting for a blame and fills in a commit message that has arrived.
    pub(super) fn refresh_blame_card(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(inline_blame) = &self.inline_blame else {
            return;
        };
        if inline_blame.card_requested {
            self.show_blame_card(BlameCardTrigger::Command, ctx);
            return;
        }
        let Some(card) = &inline_blame.card else {
            return;
        };
        if card.message.is_some() {
            return;
        }
        let message = Self::blame_commit_message(&card.blame, ctx);
        if message.is_some()
            && let Some(card) = self
                .inline_blame
                .as_mut()
                .and_then(|inline_blame| inline_blame.card.as_mut())
        {
            card.message = message;
            ctx.notify();
        }
    }

    /// Follows the mouse, on every move over the editor: opens the card once the mouse has
    /// stayed on the blame for a moment, and closes a card opened that way once the mouse is on
    /// neither of them.
    ///
    /// The hover callbacks of the two elements are not enough for this. A hover that completes
    /// on a mouse event the framework made up after a layout change is not always reported,
    /// which the state of the mouse checked here does not depend on.
    pub(super) fn track_blame_hover(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(inline_blame) = &mut self.inline_blame else {
            return;
        };
        let is_over_annotation_element = is_mouse_over(&inline_blame.annotation_mouse_state);
        if !is_over_annotation_element {
            inline_blame.card_hover_armed = true;
        }
        // The element keeps its last state while no annotation is drawn.
        let on_annotation = inline_blame.shown.is_some() && is_over_annotation_element;
        let on_card = is_mouse_over(&inline_blame.card_handles.card);

        match &mut inline_blame.card {
            None => {
                let should_open = on_annotation
                    && inline_blame.card_hover_armed
                    && !inline_blame.card_hover_pending;
                if should_open {
                    inline_blame.card_hover_pending = true;
                    ctx.spawn(Timer::after(BLAME_CARD_HOVER_DELAY), |me, _, ctx| {
                        me.open_blame_card_if_still_hovered(ctx);
                    });
                }
            }
            Some(card) => match card.trigger {
                // Once the mouse has been on a card opened from the keyboard, leaving closes it.
                BlameCardTrigger::Command => {
                    if on_card {
                        card.trigger = BlameCardTrigger::Hover;
                    }
                }
                BlameCardTrigger::Hover => {
                    if !on_annotation && !on_card {
                        self.schedule_blame_card_dismissal(ctx);
                    }
                }
            },
        }
    }

    fn open_blame_card_if_still_hovered(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(inline_blame) = &mut self.inline_blame else {
            return;
        };
        inline_blame.card_hover_pending = false;
        let is_still_hovered = inline_blame.card.is_none()
            && inline_blame.shown.is_some()
            && is_mouse_over(&inline_blame.annotation_mouse_state);
        if is_still_hovered {
            self.show_blame_card(BlameCardTrigger::Hover, ctx);
        }
    }

    fn schedule_blame_card_dismissal(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(inline_blame) = &mut self.inline_blame else {
            return;
        };
        if inline_blame.card_dismissal_pending {
            return;
        }
        inline_blame.card_dismissal_pending = true;
        ctx.spawn(Timer::after(BLAME_CARD_DISMISS_DELAY), |me, _, ctx| {
            let Some(inline_blame) = &mut me.inline_blame else {
                return;
            };
            inline_blame.card_dismissal_pending = false;
            let is_hover_card = inline_blame
                .card
                .as_ref()
                .is_some_and(|card| card.trigger == BlameCardTrigger::Hover);
            let is_hovered = (inline_blame.shown.is_some()
                && is_mouse_over(&inline_blame.annotation_mouse_state))
                || is_mouse_over(&inline_blame.card_handles.card);
            if is_hover_card && !is_hovered {
                me.dismiss_blame_card(ctx);
            }
        });
    }

    pub(super) fn mark_blame_sha_copied(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(card) = self
            .inline_blame
            .as_mut()
            .and_then(|inline_blame| inline_blame.card.as_mut())
        {
            card.copied_sha = true;
            ctx.notify();
        }
    }

    /// Places the card where the blame of its line starts: above the line when there is room,
    /// below it otherwise. `None` while the line is scrolled out of view.
    pub(super) fn blame_card_positioning(&self, app: &AppContext) -> Option<OffsetPositioning> {
        let card = self.inline_blame.as_ref()?.card.as_ref()?;
        let editor = self.editor.as_ref(app);
        let anchor = editor.trailing_annotation_bounds_in_viewport(card.line, app)?;
        let viewport_height = editor.viewport_height(app).unwrap_or(f32::MAX);
        if anchor.origin_y() > viewport_height || anchor.max_y() < 0. {
            return None;
        }

        let x = anchor.origin_x().max(0.);
        let (y, child_anchor) = if anchor.origin_y() >= BLAME_CARD_ROOM_ABOVE {
            (anchor.origin_y(), ChildAnchor::BottomLeft)
        } else {
            (anchor.max_y(), ChildAnchor::TopLeft)
        };
        Some(OffsetPositioning::offset_from_parent(
            Vector2F::new(x, y),
            ParentOffsetBounds::ParentByPosition,
            ParentAnchor::TopLeft,
            child_anchor,
        ))
    }

    pub(super) fn render_blame_card(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        let inline_blame = self.inline_blame.as_ref()?;
        let card = inline_blame.card.as_ref()?;
        let handles = &inline_blame.card_handles;
        let commit = &card.blame.commit;

        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let main_color: ColorU = theme.active_ui_text_color().into();
        let sub_color: ColorU = theme.sub_text_color(theme.background()).into();
        let text = |content: String, color: ColorU| {
            Text::new(
                content,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(color)
            .soft_wrap(true)
            .finish()
        };
        let separator = || {
            Container::new(
                ConstrainedBox::new(
                    Rect::new()
                        .with_background(internal_colors::neutral_2(theme))
                        .finish(),
                )
                .with_height(1.)
                .finish(),
            )
            .with_vertical_padding(6.)
            .finish()
        };
        let button =
            |label: &str, mouse_state: &MouseStateHandle, action: LocalCodeEditorAction| {
                Container::new(
                    appearance
                        .ui_builder()
                        .button(ButtonVariant::Outlined, mouse_state.clone())
                        .with_text_label(label.to_string())
                        .with_style(UiComponentStyles {
                            height: Some(24.),
                            padding: Some(Coords {
                                left: 8.,
                                right: 8.,
                                ..Default::default()
                            }),
                            font_color: Some(main_color),
                            ..Default::default()
                        })
                        .build()
                        .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
                        .finish(),
                )
                .with_margin_right(6.)
                .finish()
            };

        let mut content = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        if commit.is_uncommitted() {
            content.add_child(text(UNCOMMITTED_LABEL.to_string(), main_color));
            content.add_child(text(
                "This line is not in the last commit of the checked-out branch.".to_string(),
                sub_color,
            ));
        } else {
            let you = if author_label(commit, &card.blame.repo.identity) == commit.author {
                ""
            } else {
                " (you)"
            };
            content.add_child(text(
                format!("{} <{}>{you}", commit.author, commit.author_mail),
                main_color,
            ));

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs() as i64);
            let ago = relative_time(commit.author_time, now);
            let when = match absolute_time(commit.author_time, commit.author_tz_minutes) {
                Some(date) => format!("{date} ({ago})"),
                None => ago,
            };
            content.add_child(text(when, sub_color));

            let sha_label = Text::new(
                short_sha(&commit.sha).to_string(),
                appearance.monospace_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(main_color)
            .finish();
            content.add_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(Container::new(sha_label).with_margin_right(10.).finish())
                        .with_child(button(
                            if card.copied_sha {
                                "Copied"
                            } else {
                                "Copy SHA"
                            },
                            &handles.copy_sha,
                            LocalCodeEditorAction::CopyBlameSha(commit.sha.clone()),
                        ))
                        .finish(),
                )
                .with_margin_top(6.)
                .finish(),
            );
            content.add_child(separator());

            let message = card.message.as_deref().unwrap_or(&commit.summary);
            let mut lines = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
            for line in message.lines() {
                if line.trim().is_empty() {
                    lines.add_child(
                        ConstrainedBox::new(Rect::new().finish())
                            .with_height(8.)
                            .finish(),
                    );
                } else {
                    lines.add_child(text(line.to_string(), main_color));
                }
            }
            let scrollable_message = ClippedScrollable::vertical(
                handles.message_scroll.clone(),
                lines.finish(),
                ScrollbarWidth::Auto,
                theme.disabled_ui_text_color().into(),
                theme.active_ui_text_color().into(),
                warpui::elements::Fill::None,
            )
            .finish();
            content.add_child(
                ConstrainedBox::new(scrollable_message)
                    .with_max_height(BLAME_CARD_MESSAGE_MAX_HEIGHT)
                    .finish(),
            );
            content.add_child(separator());

            let mut actions = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
            actions.add_child(button(
                "Open commit in code review",
                &handles.open_in_code_review,
                LocalCodeEditorAction::OpenBlameCommitInCodeReview {
                    repo_root: card.blame.file.repo_root.clone(),
                    sha: commit.sha.clone(),
                },
            ));
            let github_url = card
                .blame
                .repo
                .origin_url
                .as_deref()
                .and_then(|origin| github_commit_url(origin, &commit.sha));
            if let Some(url) = github_url {
                actions.add_child(button(
                    "Open on GitHub",
                    &handles.open_on_github,
                    LocalCodeEditorAction::OpenBlameCommitOnGitHub(url),
                ));
            }
            content.add_child(actions.finish());
        }

        let card_element = Container::new(
            ConstrainedBox::new(content.finish())
                .with_width(BLAME_CARD_WIDTH)
                .finish(),
        )
        .with_horizontal_padding(10.)
        .with_vertical_padding(8.)
        .with_background(theme.background())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .with_border(Border::all(1.).with_border_fill(internal_colors::neutral_4(theme)))
        .finish();

        // Hover is tracked so that the card stays open while the mouse is on it.
        Some(
            Hoverable::new(handles.card.clone(), |_| card_element)
                .on_hover(|_, ctx, _, _| {
                    ctx.dispatch_typed_action(LocalCodeEditorAction::BlameCardHoverChanged);
                })
                .finish(),
        )
    }
}
