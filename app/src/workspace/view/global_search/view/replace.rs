//! The replace side of global search: the replacement field, what each result would become, and
//! the results set aside from replacing.

use std::path::Path;
use std::sync::Arc;

use project_replace::preview::{Previewer, RowPreview, preview_row};
use project_replace::replacement::ReplaceSpec;
use project_replace::selection::Dismissals;
use warp_core::ui::Icon;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::Fill as ThemeFill;
use warp_util::local_or_remote_path::LocalOrRemotePath;
use warpui::elements::{ChildView, ConstrainedBox, Container, Hoverable, MouseStateHandle};
use warpui::platform::Cursor;
use warpui::{AppContext, Element, ViewContext, ViewHandle};

use super::{GlobalSearchAction, GlobalSearchView, Match, MatchedPath, ROW_ICON_SIZE};
use crate::editor::{EditorView, Event as EditorEvent};
use crate::ui_components::blended_colors;
use crate::ui_components::icons::Icon as UiIcon;
use crate::view_components::action_button::{ActionButton, ButtonSize, NakedTheme};

/// The state of the replace side of a global search.
pub(super) struct ReplaceUi {
    /// Whether the replacement field shows.
    pub(super) is_open: bool,
    pub(super) toggle_mouse_state: MouseStateHandle,
    pub(super) editor: ViewHandle<EditorView>,
    pub(super) preserve_case_enabled: bool,
    pub(super) preserve_case_button: ViewHandle<ActionButton>,
    /// The results a replace leaves alone.
    pub(super) dismissals: Dismissals,
    /// Works out what each result would be replaced with. Present while the replacement field
    /// shows and holds something.
    pub(super) previewer: Option<Previewer>,
}

impl ReplaceUi {
    pub(super) fn new(ctx: &mut ViewContext<GlobalSearchView>) -> Self {
        let editor = GlobalSearchView::add_field_editor("Replace", ctx);
        ctx.subscribe_to_view(&editor, |me, _handle, event, ctx| {
            me.handle_replace_editor_event(event, ctx);
        });

        let preserve_case_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new_with_boxed_theme(String::new(), Arc::new(NakedTheme))
                .with_icon(UiIcon::PreserveCase)
                .with_tooltip("Toggle Preserve Case")
                .with_size(ButtonSize::Small)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(GlobalSearchAction::TogglePreserveCase);
                })
        });

        Self {
            is_open: false,
            toggle_mouse_state: MouseStateHandle::default(),
            editor,
            preserve_case_enabled: false,
            preserve_case_button,
            dismissals: Dismissals::default(),
            previewer: None,
        }
    }
}

impl GlobalSearchView {
    fn handle_replace_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Focused => self.on_field_editor_focused(ctx),
            EditorEvent::Edited(_)
            | EditorEvent::BufferReplaced
            | EditorEvent::BufferReinitialized => self.refresh_replace_preview(ctx),
            _ => {}
        }
    }

    /// Shows the replacement field.
    pub fn show_replace(&mut self, ctx: &mut ViewContext<Self>) {
        self.replace.is_open = true;
        self.refresh_replace_preview(ctx);
    }

    pub(super) fn toggle_replace_open(&mut self, ctx: &mut ViewContext<Self>) {
        self.replace.is_open = !self.replace.is_open;
        self.refresh_replace_preview(ctx);
    }

    pub(super) fn toggle_preserve_case(&mut self, ctx: &mut ViewContext<Self>) {
        self.replace.preserve_case_enabled = !self.replace.preserve_case_enabled;
        let is_active = self.replace.preserve_case_enabled;
        self.replace
            .preserve_case_button
            .update(ctx, |button, ctx| {
                button.set_active(is_active, ctx);
            });
        self.refresh_replace_preview(ctx);
    }

    /// The text to replace results with, as typed.
    pub(super) fn replacement_text(&self, app: &AppContext) -> String {
        self.replace.editor.as_ref(app).buffer_text(app)
    }

    /// Brings what the results would be replaced with in line with the search that found them and
    /// with the replacement field.
    pub(super) fn refresh_replace_preview(&mut self, ctx: &mut ViewContext<Self>) {
        let replacement = self.replacement_text(ctx);
        self.replace.previewer = match &self.searched_spec {
            Some(search) if self.replace.is_open && !replacement.is_empty() => {
                Some(Previewer::new(
                    search,
                    &ReplaceSpec {
                        replacement,
                        preserve_case: self.replace.preserve_case_enabled,
                    },
                ))
            }
            Some(_) | None => None,
        };
        ctx.notify();
    }

    pub(super) fn toggle_match_dismissed(
        &mut self,
        location: &LocalOrRemotePath,
        line_number: u32,
        column: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(path) = location.to_local_path() {
            self.replace
                .dismissals
                .toggle_match(path, line_number, column);
            ctx.notify();
        }
    }

    pub(super) fn toggle_file_dismissed(
        &mut self,
        location: &LocalOrRemotePath,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(path) = location.to_local_path() {
            self.replace.dismissals.toggle_file(path);
            ctx.notify();
        }
    }

    /// The file and column that, with its line, identify a result for replacing. `None` for a
    /// result that cannot be replaced: one on another host, or one the search could not place in
    /// its line.
    pub(super) fn replaceable_key<'a>(
        matched_path: &'a MatchedPath,
        matched: &Match,
    ) -> Option<(&'a Path, usize)> {
        Some((matched_path.path.to_local_path()?, matched.column_num?))
    }

    pub(super) fn is_file_dismissed(&self, matched_path: &MatchedPath) -> bool {
        matched_path
            .path
            .to_local_path()
            .is_some_and(|path| self.replace.dismissals.is_file_dismissed(path))
    }

    pub(super) fn is_match_dismissed(&self, matched_path: &MatchedPath, matched: &Match) -> bool {
        Self::replaceable_key(matched_path, matched).is_some_and(|(path, column)| {
            self.replace
                .dismissals
                .is_match_dismissed(path, matched.line_number, column)
        })
    }

    /// What the result would look like once replaced, while there is a replacement to show for
    /// it.
    pub(super) fn replace_preview(
        &self,
        matched_path: &MatchedPath,
        matched: &Match,
    ) -> Option<RowPreview> {
        let previewer = self.replace.previewer.as_ref()?;
        Self::replaceable_key(matched_path, matched)?;
        if self.is_match_dismissed(matched_path, matched) {
            return None;
        }
        let submatch = matched.submatches.first()?;
        let range = submatch.byte_start.as_usize()..submatch.byte_end.as_usize();
        let replacement = previewer.replacement_for(&matched.line_text, range.clone());
        Some(preview_row(&matched.line_text, range, &replacement))
    }

    /// Renders the chevron that shows and hides the replacement field.
    pub(super) fn render_replace_toggle(&self, appearance: &Appearance) -> Box<dyn Element> {
        let is_open = self.replace.is_open;
        let theme = appearance.theme();
        Hoverable::new(self.replace.toggle_mouse_state.clone(), move |state| {
            let color = if state.is_hovered() {
                theme.foreground().into()
            } else {
                blended_colors::text_sub(theme, theme.background())
            };
            let icon = if is_open {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            };
            Container::new(
                ConstrainedBox::new(icon.to_warpui_icon(ThemeFill::Solid(color)).finish())
                    .with_width(ROW_ICON_SIZE)
                    .with_height(ROW_ICON_SIZE)
                    .finish(),
            )
            .with_margin_right(4.)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(|ctx, _, _| {
            ctx.dispatch_typed_action(GlobalSearchAction::ToggleReplaceOpen);
        })
        .finish()
    }

    pub(super) fn render_replace_row(&self, app: &AppContext) -> Box<dyn Element> {
        let preserve_case_button =
            Container::new(ChildView::new(&self.replace.preserve_case_button).finish()).finish();
        self.render_field_row(&self.replace.editor, Some(preserve_case_button), app)
    }

    /// Renders a small icon inside a result row that runs `action` when clicked.
    pub(super) fn render_row_button(
        icon: Icon,
        mouse_state: MouseStateHandle,
        action: GlobalSearchAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        Hoverable::new(mouse_state, move |state| {
            let color = if state.is_hovered() {
                theme.foreground().into()
            } else {
                blended_colors::text_sub(theme, theme.background())
            };
            Container::new(
                ConstrainedBox::new(icon.to_warpui_icon(ThemeFill::Solid(color)).finish())
                    .with_width(ROW_ICON_SIZE)
                    .with_height(ROW_ICON_SIZE)
                    .finish(),
            )
            .with_margin_left(6.)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(action.clone());
        })
        .finish()
    }
}
