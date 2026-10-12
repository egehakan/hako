//! The replace side of global search: the replacement field, what each result would become, the
//! results set aside from replacing, and the replace itself.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pathfinder_geometry::vector::vec2f;
use project_replace::apply::FileTarget;
use project_replace::engine::SearchSpec;
use project_replace::preview::{Previewer, RowPreview, preview_row};
use project_replace::replacement::ReplaceSpec;
use project_replace::report::{
    ReplaceCounts, confirmation_title, replace_all_label, replace_summary,
};
use project_replace::selection::{Dismissals, Selection, TargetsBuilder, count_targets};
use warp_core::ui::Icon;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::Fill as ThemeFill;
use warp_util::local_or_remote_path::LocalOrRemotePath;
use warpui::elements::{
    ChildAnchor, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Flex, Hoverable,
    MouseStateHandle, OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds,
    Shrinkable, Stack, Text,
};
use warpui::modals::{AlertDialogWithCallbacks, ModalButton};
use warpui::platform::Cursor;
use warpui::ui_components::components::UiComponent as _;
use warpui::{AppContext, Element, ModelHandle, SingletonEntity, ViewContext, ViewHandle};

use super::{GlobalSearchAction, GlobalSearchView, Match, MatchedPath, ROW_ICON_SIZE};
use crate::editor::{EditorView, Event as EditorEvent};
use crate::ui_components::blended_colors;
use crate::ui_components::icons::Icon as UiIcon;
use crate::view_components::DismissibleToast;
use crate::view_components::action_button::{
    ActionButton, ButtonSize, DisabledSecondaryTheme, NakedTheme, SecondaryTheme,
};
use crate::workspace::view::global_search::replace_model::{
    ProjectReplace, ProjectReplaceEvent, ReplaceJob,
};
use crate::workspace::{ToastStack, WorkspaceAction};

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
    pub(super) model: ModelHandle<ProjectReplace>,
    pub(super) replace_all_button: ViewHandle<ActionButton>,
    pub(super) cancel_button: ViewHandle<ActionButton>,
    /// Whether a replace can start: the results are complete and are those of the search the
    /// fields describe, and there is a replacement.
    pub(super) is_ready: bool,
    /// What replacing everything would cover, while a replace can start.
    pub(super) counts: ReplaceCounts,
    /// The file the search was in when it stopped at its limit of matches, which may hold more
    /// of them than are listed.
    pub(super) truncated_file: Option<PathBuf>,
    /// Whether the search ended early, so that files may be missing matches or missing
    /// altogether.
    pub(super) search_incomplete: bool,
    /// Where the results were scrolled to before a replace made them search again.
    pub(super) scroll_top_to_restore: Option<f32>,
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

        let replace_all_button = ctx.add_typed_action_view(|ctx| {
            let mut button = ActionButton::new("Replace All", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .with_disabled_theme(DisabledSecondaryTheme)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(GlobalSearchAction::ReplaceAll);
                });
            button.set_disabled(true, ctx);
            button
        });

        let cancel_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new("Cancel", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(GlobalSearchAction::CancelReplace);
                })
        });

        let model = ctx.add_model(|_| ProjectReplace::new());
        ctx.subscribe_to_model(&model, |me, _handle, event, ctx| {
            me.handle_replace_event(event, ctx);
        });

        Self {
            is_open: false,
            toggle_mouse_state: MouseStateHandle::default(),
            editor,
            preserve_case_enabled: false,
            preserve_case_button,
            dismissals: Dismissals::default(),
            previewer: None,
            model,
            replace_all_button,
            cancel_button,
            is_ready: false,
            counts: ReplaceCounts::default(),
            truncated_file: None,
            search_incomplete: false,
            scroll_top_to_restore: None,
        }
    }
}

/// A small button inside a result row.
pub(super) struct RowButton {
    icon: Icon,
    tooltip: &'static str,
    mouse_state: MouseStateHandle,
    action: GlobalSearchAction,
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

    fn handle_replace_event(&mut self, event: &ProjectReplaceEvent, ctx: &mut ViewContext<Self>) {
        match event {
            ProjectReplaceEvent::Progress => {}
            ProjectReplaceEvent::Finished(report) => {
                let summary = replace_summary(
                    report.counts(),
                    &report.skipped,
                    report.was_cancelled().then_some(report.not_attempted),
                );
                self.show_replace_toast(DismissibleToast::default(summary), ctx);

                // The results on screen describe the files as they were, so search again, and
                // come back to the same place in the list.
                self.replace.scroll_top_to_restore =
                    Some(self.uniform_list_state.scroll_top().as_f64() as f32);
                self.rerun_search_from_query(ctx, true);
            }
            ProjectReplaceEvent::Failed(message) => {
                self.show_replace_toast(DismissibleToast::error(message.clone()), ctx);
            }
        }
        self.refresh_replace_controls(ctx);
    }

    fn show_replace_toast(
        &self,
        toast: DismissibleToast<WorkspaceAction>,
        ctx: &mut ViewContext<Self>,
    ) {
        let window_id = ctx.window_id();
        let toast = toast.with_object_id(format!("project_replace_{}", ctx.view_id()));
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(toast, window_id, ctx);
        });
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
        self.refresh_replace_controls(ctx);
    }

    /// Whether a replace can start right now.
    fn is_replace_ready(&self, app: &AppContext) -> bool {
        let Some(searched) = &self.searched_spec else {
            return false;
        };
        // Results that are still coming in, or that belong to a search other than the one the
        // fields describe, are not what the user is looking at.
        let described = SearchSpec {
            pattern: self.query_editor.as_ref(app).buffer_text(app),
            use_regex: self.regex_search_enabled,
            case_sensitive: self.case_sensitivity_enabled,
            whole_word: self.whole_word_enabled,
        };
        let scope_is_searched = self
            .last_searched_options
            .as_ref()
            .is_some_and(|options| options.scope == self.search_scope(app));

        self.replace.is_open
            && self.replace.previewer.is_some()
            && !self.is_search_in_progress
            && !self.replace.search_incomplete
            && self.last_error.is_none()
            && *searched == described
            && scope_is_searched
            && !self.replace.model.as_ref(app).is_running()
    }

    /// Brings the button that replaces everything in line with what there is to replace and with
    /// whether a replace can start.
    pub(super) fn refresh_replace_controls(&mut self, ctx: &mut ViewContext<Self>) {
        self.replace.is_ready = self.is_replace_ready(ctx);
        self.replace.counts = if self.replace.is_ready {
            count_targets(&self.replace_targets(Selection::All))
        } else {
            ReplaceCounts::default()
        };

        let counts = self.replace.counts;
        let label = if counts.is_empty() {
            "Replace All".to_string()
        } else {
            replace_all_label(counts)
        };
        self.replace.replace_all_button.update(ctx, |button, ctx| {
            button.set_label(label, ctx);
            button.set_disabled(counts.is_empty(), ctx);
        });
        ctx.notify();
    }

    /// The files a replace of `selection` goes through, each with every match the search found
    /// in it.
    fn replace_targets(&self, selection: Selection<'_>) -> Vec<FileTarget> {
        let mut builder = TargetsBuilder::new(&self.replace.dismissals, selection);
        for directory in &self.directory_entries {
            for matched_path in &directory.matched_paths.paths {
                for matched in &matched_path.matches {
                    let (Some((path, column)), Some(submatch)) = (
                        Self::replaceable_key(matched_path, matched),
                        matched.submatches.first(),
                    ) else {
                        continue;
                    };
                    builder.add_row(
                        path,
                        matched.line_number,
                        column,
                        &matched.line_text,
                        submatch.byte_start.as_usize()..submatch.byte_end.as_usize(),
                    );
                }
            }
        }
        if let Some(path) = &self.replace.truncated_file {
            builder.mark_truncated(path);
        }
        builder.build()
    }

    /// Asks before replacing everything that was not set aside.
    pub(super) fn confirm_replace_all(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.is_replace_ready(ctx) {
            return;
        }
        let Some(search) = &self.searched_spec else {
            return;
        };
        let files = self.replace_targets(Selection::All);
        let counts = count_targets(&files);
        if counts.is_empty() {
            return;
        }

        let title = confirmation_title(counts, &search.pattern, &self.replacement_text(ctx));
        ctx.show_native_platform_modal(AlertDialogWithCallbacks::for_view(
            title,
            "Files that changed since the search are left alone.",
            vec![
                // The first button is the one Return presses.
                ModalButton::for_view("Cancel", |_: &mut Self, _| {}),
                ModalButton::for_view("Replace", move |me: &mut Self, ctx| {
                    me.start_replace(files, ctx);
                }),
            ],
            |_, _| {},
        ));
    }

    pub(super) fn replace_in_file(
        &mut self,
        location: &LocalOrRemotePath,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(path) = location.to_local_path() else {
            return;
        };
        let files = self.replace_targets(Selection::File(path));
        self.start_replace(files, ctx);
    }

    pub(super) fn replace_match(
        &mut self,
        location: &LocalOrRemotePath,
        line_number: u32,
        column: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(path) = location.to_local_path() else {
            return;
        };
        let files = self.replace_targets(Selection::Match {
            path,
            line: line_number,
            column,
        });
        self.start_replace(files, ctx);
    }

    fn start_replace(&mut self, files: Vec<FileTarget>, ctx: &mut ViewContext<Self>) {
        if files.is_empty() || !self.is_replace_ready(ctx) {
            return;
        }
        let (Some(search), Some(options)) =
            (self.searched_spec.clone(), &self.last_searched_options)
        else {
            return;
        };
        let job = ReplaceJob {
            search,
            replace: ReplaceSpec {
                replacement: self.replacement_text(ctx),
                preserve_case: self.replace.preserve_case_enabled,
            },
            roots: self
                .search_roots
                .iter()
                .filter_map(|root| root.to_local_path().map(Path::to_path_buf))
                .collect(),
            scope: options.scope.clone(),
            files,
        };
        self.replace.model.update(ctx, |model, ctx| {
            model.start(job, ctx);
        });
    }

    pub(super) fn cancel_replace(&mut self, ctx: &mut ViewContext<Self>) {
        self.replace.model.update(ctx, |model, ctx| {
            model.cancel(ctx);
        });
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
            self.refresh_replace_controls(ctx);
        }
    }

    pub(super) fn toggle_file_dismissed(
        &mut self,
        location: &LocalOrRemotePath,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(path) = location.to_local_path() {
            self.replace.dismissals.toggle_file(path);
            self.refresh_replace_controls(ctx);
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

    /// The buttons a text match shows while the replacement field does.
    pub(super) fn match_row_buttons(
        &self,
        matched_path: &MatchedPath,
        matched: &Match,
    ) -> Vec<RowButton> {
        let Some((_, column)) = Self::replaceable_key(matched_path, matched) else {
            return Vec::new();
        };
        if !self.replace.is_open {
            return Vec::new();
        }

        let location = &matched_path.path;
        let line_number = matched.line_number;
        let is_dismissed = self.is_match_dismissed(matched_path, matched);
        let mut buttons = Vec::new();
        if self.replace.is_ready && !is_dismissed {
            buttons.push(RowButton {
                icon: Icon::Check,
                tooltip: "Replace",
                mouse_state: matched.replace_mouse_state.clone(),
                action: GlobalSearchAction::ReplaceMatch {
                    location: location.clone(),
                    line_number,
                    column,
                },
            });
        }
        buttons.push(RowButton {
            icon: if is_dismissed { Icon::Plus } else { Icon::X },
            tooltip: if is_dismissed {
                "Include in Replace"
            } else {
                "Leave out of Replace"
            },
            mouse_state: matched.dismiss_mouse_state.clone(),
            action: GlobalSearchAction::ToggleMatchDismissed {
                location: location.clone(),
                line_number,
                column,
            },
        });
        buttons
    }

    /// The buttons the header of a file shows while the replacement field does.
    pub(super) fn file_row_buttons(&self, matched_path: &MatchedPath) -> Vec<RowButton> {
        if !self.replace.is_open || !matched_path.path.is_local() {
            return Vec::new();
        }

        let location = &matched_path.path;
        let is_dismissed = self.is_file_dismissed(matched_path);
        let mut buttons = Vec::new();
        if self.replace.is_ready && !is_dismissed {
            buttons.push(RowButton {
                icon: Icon::Check,
                tooltip: "Replace All in File",
                mouse_state: matched_path.replace_mouse_state.clone(),
                action: GlobalSearchAction::ReplaceInFile {
                    location: location.clone(),
                },
            });
        }
        buttons.push(RowButton {
            icon: if is_dismissed { Icon::Plus } else { Icon::X },
            tooltip: if is_dismissed {
                "Include in Replace"
            } else {
                "Leave out of Replace"
            },
            mouse_state: matched_path.dismiss_mouse_state.clone(),
            action: GlobalSearchAction::ToggleFileDismissed {
                location: location.clone(),
            },
        });
        buttons
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

    /// Renders the button that replaces everything, or how far a running replace got and the
    /// button that stops it.
    pub(super) fn render_replace_actions(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.replace.model.as_ref(app);

        let hint = |text: String| {
            Text::new_inline(text, appearance.ui_font_family(), 12.)
                .with_color(blended_colors::text_sub(theme, theme.background()))
                .finish()
        };
        let row = if model.is_running() {
            let progress = match model.progress() {
                Some(progress) => {
                    format!("Replacing… {} of {} files", progress.done, progress.total)
                }
                None => "Replacing…".to_string(),
            };
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(ChildView::new(&self.replace.cancel_button).finish())
                .with_child(
                    Shrinkable::new(
                        1.0,
                        Container::new(hint(progress)).with_margin_left(8.).finish(),
                    )
                    .finish(),
                )
        } else {
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(ChildView::new(&self.replace.replace_all_button).finish());
            if let Some(text) = self.replace_hint(app) {
                row.add_child(
                    Shrinkable::new(
                        1.0,
                        Container::new(hint(text.to_string()))
                            .with_margin_left(8.)
                            .finish(),
                    )
                    .finish(),
                );
            }
            row
        };
        Container::new(row.finish()).with_margin_bottom(4.).finish()
    }

    /// Why nothing can be replaced right now, when the reason is not plain from the results.
    fn replace_hint(&self, app: &AppContext) -> Option<&'static str> {
        if self.replace.is_ready {
            return None;
        }
        if self.replace.search_incomplete {
            Some("The search ended early. Search again to replace.")
        } else if self.is_search_in_progress {
            Some("Waiting for the search to finish.")
        } else if self.replacement_text(app).is_empty() {
            Some("Type what to replace matches with.")
        } else {
            None
        }
    }

    /// Renders the buttons of a result row, in order, to go at the end of the row.
    pub(super) fn render_row_buttons(
        buttons: Vec<RowButton>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        for button in buttons {
            row.add_child(Self::render_row_button(button, appearance));
        }
        row.finish()
    }

    fn render_row_button(button: RowButton, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let RowButton {
            icon,
            tooltip,
            mouse_state,
            action,
        } = button;
        Hoverable::new(mouse_state, move |state| {
            let color = if state.is_hovered() {
                theme.foreground().into()
            } else {
                blended_colors::text_sub(theme, theme.background())
            };
            let icon = Container::new(
                ConstrainedBox::new(icon.to_warpui_icon(ThemeFill::Solid(color)).finish())
                    .with_width(ROW_ICON_SIZE)
                    .with_height(ROW_ICON_SIZE)
                    .finish(),
            )
            .with_margin_left(6.)
            .finish();
            if !state.is_hovered() {
                return icon;
            }

            let tooltip = appearance
                .ui_builder()
                .tool_tip(tooltip.to_string())
                .build()
                .finish();
            let mut stack = Stack::new().with_child(icon);
            stack.add_positioned_overlay_child(
                tooltip,
                OffsetPositioning::offset_from_parent(
                    vec2f(0., 4.),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::BottomMiddle,
                    ChildAnchor::TopLeft,
                ),
            );
            stack.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(action.clone());
        })
        .finish()
    }
}
