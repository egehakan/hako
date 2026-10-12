//! Source-control overview of every repository nested under a folder that is not itself a
//! repository.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use pathfinder_color::ColorU;
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    Clipped, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Element, Empty, Fill,
    Flex, Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement, Radius,
    ScrollStateHandle, Scrollable, ScrollableElement, ScrollbarWidth, Shrinkable, Text,
    UniformList, UniformListState,
};
use warpui::fonts::{Properties, Weight};
use warpui::platform::Cursor;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    WeakViewHandle,
};

use super::code_review_view::{CONTENT_LEFT_MARGIN, CONTENT_RIGHT_MARGIN};
use super::diff_state::GitFileStatus;
use super::nested_repos::{
    NestedRepoEntry, NestedRepoFile, NestedRepoKind, NestedReposEvent, NestedReposModel,
};
use crate::appearance::Appearance;
use crate::code::editor::{add_color, remove_color, replace_color};
use crate::ui_components::blended_colors;
use crate::ui_components::buttons::icon_button_with_color;
use crate::ui_components::icons::Icon;
use crate::ui_components::item_highlight::ItemHighlightState;

/// Every row shares one height so the list can be virtualised.
const ROW_HEIGHT: f32 = 26.;
const ROW_HORIZONTAL_PADDING: f32 = 8.;
const LIST_HORIZONTAL_PADDING: f32 = 8.;
const CHEVRON_SIZE: f32 = 16.;
const TRACKING_ICON_SIZE: f32 = 12.;
const FILE_ROW_INDENT: f32 = 20.;
const STATUS_LETTER_WIDTH: f32 = 18.;
const REPO_NAME_MAX_WIDTH: f32 = 320.;
const FILE_NAME_MAX_WIDTH: f32 = 240.;
const SECONDARY_FONT_SIZE_RATIO: f32 = 0.9;

/// Repositories with more changed files than this start collapsed, so that one busy repository
/// does not push every other one out of view.
const MAX_FILES_EXPANDED_BY_DEFAULT: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NestedRepoGitAction {
    Commit,
    Push,
}

/// A request to show one repository in its full review view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenNestedRepo {
    pub repo_root: PathBuf,
    /// Repository-relative path of the changed file to bring into view.
    pub file: Option<String>,
    /// Git dialog to open once the repository is shown.
    pub action: Option<NestedRepoGitAction>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NestedReposViewAction {
    ToggleRepo(PathBuf),
    OpenRepo(OpenNestedRepo),
    Refresh,
}

#[derive(Clone, Debug)]
pub enum NestedReposViewEvent {
    OpenRepo(OpenNestedRepo),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverviewRow {
    Repo {
        repo_index: usize,
    },
    File {
        repo_index: usize,
        file_index: usize,
    },
}

/// Lays repositories out as one flat list of rows, given each repository's changed-file count and
/// whether it is expanded.
fn flatten_rows(repos: impl IntoIterator<Item = (usize, bool)>) -> Vec<OverviewRow> {
    let mut rows = Vec::new();
    for (repo_index, (file_count, is_expanded)) in repos.into_iter().enumerate() {
        rows.push(OverviewRow::Repo { repo_index });
        if is_expanded {
            rows.extend((0..file_count).map(|file_index| OverviewRow::File {
                repo_index,
                file_index,
            }));
        }
    }
    rows
}

fn is_expanded_by_default(changed_file_count: usize) -> bool {
    changed_file_count <= MAX_FILES_EXPANDED_BY_DEFAULT
}

fn status_letter(status: &GitFileStatus) -> &'static str {
    match status {
        GitFileStatus::New => "A",
        GitFileStatus::Modified => "M",
        GitFileStatus::Deleted => "D",
        GitFileStatus::Renamed { .. } => "R",
        GitFileStatus::Copied { .. } => "C",
        GitFileStatus::Untracked => "U",
        GitFileStatus::Conflicted => "!",
    }
}

fn status_color(status: &GitFileStatus, appearance: &Appearance) -> ColorU {
    match status {
        GitFileStatus::New | GitFileStatus::Copied { .. } | GitFileStatus::Untracked => {
            add_color(appearance)
        }
        GitFileStatus::Modified | GitFileStatus::Renamed { .. } => replace_color(appearance),
        GitFileStatus::Deleted | GitFileStatus::Conflicted => remove_color(appearance),
    }
}

fn changed_file_count(entry: &NestedRepoEntry) -> usize {
    entry.status.as_ref().map_or(0, |status| status.files.len())
}

#[derive(Default)]
struct RepoMouseStates {
    header: MouseStateHandle,
    open_button: MouseStateHandle,
    commit_button: MouseStateHandle,
    push_button: MouseStateHandle,
    /// One handle per changed file, in the order the files are listed.
    files: Vec<MouseStateHandle>,
}

pub struct NestedReposView {
    model: ModelHandle<NestedReposModel>,
    view_handle: WeakViewHandle<Self>,
    rows: Vec<OverviewRow>,
    /// Whether each repository is expanded, keyed by its root. Seeded from the default when a
    /// repository's status first loads, and changed only by the user after that.
    expanded: HashMap<PathBuf, bool>,
    mouse_states: HashMap<PathBuf, RepoMouseStates>,
    refresh_button_mouse_state: MouseStateHandle,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
}

impl NestedReposView {
    pub fn new(model: ModelHandle<NestedReposModel>, ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&model, |me, _, event, ctx| match event {
            NestedReposEvent::Updated => me.sync_with_model(ctx),
        });
        let mut view = Self {
            model,
            view_handle: ctx.handle(),
            rows: Vec::new(),
            expanded: HashMap::new(),
            mouse_states: HashMap::new(),
            refresh_button_mouse_state: Default::default(),
            list_state: UniformListState::new(),
            scroll_state: ScrollStateHandle::default(),
        };
        view.sync_with_model(ctx);
        view
    }

    fn is_expanded(&self, repo_root: &Path) -> bool {
        self.expanded.get(repo_root).copied().unwrap_or(true)
    }

    fn sync_with_model(&mut self, ctx: &mut ViewContext<Self>) {
        let model = self.model.clone();
        let entries = model.as_ref(ctx).entries();

        self.mouse_states
            .retain(|root, _| entries.iter().any(|entry| &entry.repo.root == root));
        for entry in entries {
            let file_count = changed_file_count(entry);
            if entry.status.is_some() {
                self.expanded
                    .entry(entry.repo.root.clone())
                    .or_insert_with(|| is_expanded_by_default(file_count));
            }
            self.mouse_states
                .entry(entry.repo.root.clone())
                .or_default()
                .files
                .resize_with(file_count, Default::default);
        }
        self.rows = flatten_rows(entries.iter().map(|entry| {
            (
                changed_file_count(entry),
                self.is_expanded(&entry.repo.root),
            )
        }));
        ctx.notify();
    }

    fn render_summary(
        &self,
        model: &NestedReposModel,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let sub_text_color = blended_colors::text_sub(theme, theme.background());

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new_inline(
                    "Source Control",
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_style(Properties::default().weight(Weight::Semibold))
                .with_color(blended_colors::text_main(theme, theme.background()))
                .finish(),
            );
        if model.has_completed_scan() {
            let count = model.entries().len();
            let label = if count == 1 {
                "1 repository".to_owned()
            } else {
                format!("{count} repositories")
            };
            row.add_child(
                Container::new(
                    Text::new_inline(
                        label,
                        appearance.ui_font_family(),
                        appearance.ui_font_size(),
                    )
                    .with_color(sub_text_color)
                    .finish(),
                )
                .with_margin_left(8.)
                .finish(),
            );
        }
        row.add_child(Shrinkable::new(1., Empty::new().finish()).finish());

        let tooltip = appearance
            .ui_builder()
            .tool_tip("Refresh repositories".to_string())
            .build()
            .finish();
        row.add_child(
            icon_button_with_color(
                appearance,
                Icon::Refresh,
                false,
                self.refresh_button_mouse_state.clone(),
                sub_text_color.into(),
            )
            .with_tooltip(move || tooltip)
            .build()
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(NestedReposViewAction::Refresh))
            .with_cursor(Cursor::PointingHand)
            .finish(),
        );

        Container::new(row.finish())
            .with_padding_left(CONTENT_LEFT_MARGIN)
            .with_padding_right(CONTENT_RIGHT_MARGIN)
            .with_padding_bottom(4.)
            .finish()
    }

    fn render_message(message: &'static str, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        Container::new(
            Text::new(
                message,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(blended_colors::text_sub(theme, theme.background()))
            .finish(),
        )
        .with_padding_left(CONTENT_LEFT_MARGIN)
        .with_padding_right(CONTENT_LEFT_MARGIN)
        .with_padding_top(8.)
        .finish()
    }

    fn render_list(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let view_handle = self.view_handle.clone();
        let list = UniformList::new(
            self.list_state.clone(),
            self.rows.len(),
            move |range: Range<usize>, app: &AppContext| {
                let rows = match view_handle.upgrade(app) {
                    Some(view) => {
                        let view = view.as_ref(app);
                        let appearance = Appearance::as_ref(app);
                        range
                            .map(|index| view.render_row(index, appearance, app))
                            .collect()
                    }
                    None => Vec::new(),
                };
                rows.into_iter()
            },
        )
        .finish_scrollable();

        Container::new(
            Scrollable::vertical(
                self.scroll_state.clone(),
                list,
                ScrollbarWidth::Auto,
                theme.nonactive_ui_detail().into(),
                theme.active_ui_detail().into(),
                Fill::None,
            )
            .with_overlayed_scrollbar()
            .finish(),
        )
        .with_horizontal_padding(LIST_HORIZONTAL_PADDING)
        .with_padding_bottom(8.)
        .finish()
    }

    fn render_row(
        &self,
        index: usize,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let entries = self.model.as_ref(app).entries();
        let row = match self.rows.get(index) {
            Some(OverviewRow::Repo { repo_index }) => entries.get(*repo_index).and_then(|entry| {
                let mouse_states = self.mouse_states.get(&entry.repo.root)?;
                Some(self.render_repo_row(entry, mouse_states, appearance))
            }),
            Some(OverviewRow::File {
                repo_index,
                file_index,
            }) => entries.get(*repo_index).and_then(|entry| {
                let file = entry.status.as_ref()?.files.get(*file_index)?;
                let mouse_state = self
                    .mouse_states
                    .get(&entry.repo.root)?
                    .files
                    .get(*file_index)?;
                Some(Self::render_file_row(entry, file, mouse_state, appearance))
            }),
            None => None,
        };
        ConstrainedBox::new(row.unwrap_or_else(|| Empty::new().finish()))
            .with_height(ROW_HEIGHT)
            .finish()
    }

    fn render_repo_row(
        &self,
        entry: &NestedRepoEntry,
        mouse_states: &RepoMouseStates,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font_family = appearance.ui_font_family();
        let font_size = appearance.ui_font_size();
        let secondary_font_size = font_size * SECONDARY_FONT_SIZE_RATIO;
        let sub_text_color = blended_colors::text_sub(theme, theme.background());

        let repo_root = entry.repo.root.clone();
        let is_expanded = self.is_expanded(&repo_root);
        let status = entry.status.as_ref();
        let file_count = changed_file_count(entry);

        let mut details = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                ConstrainedBox::new(
                    Text::new_inline(entry.display_name.clone(), font_family, font_size)
                        .with_style(Properties::default().weight(Weight::Semibold))
                        .with_color(blended_colors::text_main(theme, theme.background()))
                        .with_clip(ClipConfig::start())
                        .finish(),
                )
                .with_max_width(REPO_NAME_MAX_WIDTH)
                .finish(),
            );
        match entry.repo.kind {
            NestedRepoKind::Repository => {}
            NestedRepoKind::Worktree => details.add_child(
                Container::new(
                    Text::new_inline("worktree", font_family, secondary_font_size)
                        .with_color(sub_text_color)
                        .finish(),
                )
                .with_horizontal_padding(5.)
                .with_vertical_padding(1.)
                .with_margin_left(6.)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
                .with_background(internal_colors::fg_overlay_2(theme))
                .finish(),
            ),
        }
        if let Some(status) = status {
            for (count, icon) in [
                (status.ahead, Icon::ArrowUp),
                (status.behind, Icon::ArrowDown),
            ] {
                if count == 0 {
                    continue;
                }
                details.add_child(
                    Container::new(
                        ConstrainedBox::new(icon.to_warpui_icon(sub_text_color.into()).finish())
                            .with_width(TRACKING_ICON_SIZE)
                            .with_height(TRACKING_ICON_SIZE)
                            .finish(),
                    )
                    .with_margin_left(6.)
                    .finish(),
                );
                details.add_child(
                    Text::new_inline(count.to_string(), font_family, secondary_font_size)
                        .with_color(sub_text_color)
                        .finish(),
                );
            }
            details.add_child(
                Shrinkable::new(
                    1.,
                    Container::new(
                        Text::new_inline(status.branch.clone(), font_family, secondary_font_size)
                            .with_color(sub_text_color)
                            .with_clip(ClipConfig::end())
                            .finish(),
                    )
                    .with_margin_left(8.)
                    .finish(),
                )
                .finish(),
            );
        }

        let open_request = |action| OpenNestedRepo {
            repo_root: repo_root.clone(),
            file: None,
            action,
        };
        let mut hover_buttons = vec![Self::render_hover_button(
            "Open",
            &mouse_states.open_button,
            open_request(None),
            appearance,
        )];
        if file_count > 0 {
            hover_buttons.push(Self::render_hover_button(
                "Commit",
                &mouse_states.commit_button,
                open_request(Some(NestedRepoGitAction::Commit)),
                appearance,
            ));
        }
        if status.is_some_and(|status| status.ahead > 0) {
            hover_buttons.push(Self::render_hover_button(
                "Push",
                &mouse_states.push_button,
                open_request(Some(NestedRepoGitAction::Push)),
                appearance,
            ));
        }

        let count_label = match status {
            Some(_) => file_count.to_string(),
            None => "…".to_owned(),
        };
        let chevron = if is_expanded {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };

        let toggled_root = repo_root.clone();
        Hoverable::new(mouse_states.header.clone(), |mouse_state| {
            let highlight = ItemHighlightState::new(false, mouse_state);

            let mut trailing = Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Center);
            if mouse_state.is_hovered() {
                trailing.extend(hover_buttons);
            }
            trailing.add_child(
                Container::new(
                    Text::new_inline(count_label, font_family, secondary_font_size)
                        .with_color(sub_text_color)
                        .finish(),
                )
                .with_horizontal_padding(6.)
                .with_vertical_padding(1.)
                .with_margin_left(6.)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
                .with_background(internal_colors::fg_overlay_2(theme))
                .finish(),
            );

            let row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        1.,
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_child(
                                Container::new(
                                    ConstrainedBox::new(
                                        chevron
                                            .to_warpui_icon(
                                                highlight.text_and_icon_color(appearance).into(),
                                            )
                                            .finish(),
                                    )
                                    .with_width(CHEVRON_SIZE)
                                    .with_height(CHEVRON_SIZE)
                                    .finish(),
                                )
                                .with_margin_right(4.)
                                .finish(),
                            )
                            .with_child(
                                Shrinkable::new(1., Clipped::new(details.finish()).finish())
                                    .finish(),
                            )
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(trailing.finish())
                .finish();

            Self::finish_row(row, highlight, appearance)
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(NestedReposViewAction::ToggleRepo(toggled_root.clone()));
        })
        // The hover buttons sit inside this row; their clicks must not also toggle it.
        .with_defer_events_to_children()
        .with_cursor(Cursor::PointingHand)
        .finish()
    }

    fn render_hover_button(
        label: &'static str,
        mouse_state: &MouseStateHandle,
        request: OpenNestedRepo,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let button = Hoverable::new(mouse_state.clone(), |mouse_state| {
            let background = if mouse_state.is_hovered() {
                internal_colors::fg_overlay_5(theme)
            } else {
                internal_colors::fg_overlay_3(theme)
            };
            Container::new(
                Text::new_inline(
                    label,
                    appearance.ui_font_family(),
                    appearance.ui_font_size() * SECONDARY_FONT_SIZE_RATIO,
                )
                .with_color(blended_colors::text_main(theme, theme.background()))
                .finish(),
            )
            .with_horizontal_padding(6.)
            .with_vertical_padding(1.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .with_background(background)
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(NestedReposViewAction::OpenRepo(request.clone()));
        })
        .with_cursor(Cursor::PointingHand)
        .finish();
        Container::new(button).with_margin_left(4.).finish()
    }

    fn render_file_row(
        entry: &NestedRepoEntry,
        file: &NestedRepoFile,
        mouse_state: &MouseStateHandle,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font_family = appearance.ui_font_family();
        let font_size = appearance.ui_font_size();

        let path = Path::new(&file.path);
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&file.path)
            .to_owned();
        let directory = path
            .parent()
            .and_then(|parent| parent.to_str())
            .unwrap_or_default()
            .to_owned();
        let request = OpenNestedRepo {
            repo_root: entry.repo.root.clone(),
            file: Some(file.path.clone()),
            action: None,
        };

        Hoverable::new(mouse_state.clone(), |mouse_state| {
            let highlight = ItemHighlightState::new(false, mouse_state);
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Container::new(
                        ConstrainedBox::new(
                            Text::new_inline(status_letter(&file.status), font_family, font_size)
                                .with_style(Properties::default().weight(Weight::Semibold))
                                .with_color(status_color(&file.status, appearance))
                                .finish(),
                        )
                        .with_width(STATUS_LETTER_WIDTH)
                        .finish(),
                    )
                    .with_margin_left(FILE_ROW_INDENT)
                    .finish(),
                )
                .with_child(
                    ConstrainedBox::new(
                        Text::new_inline(file_name, font_family, font_size)
                            .with_color(blended_colors::text_main(theme, theme.background()))
                            .with_clip(ClipConfig::end())
                            .finish(),
                    )
                    .with_max_width(FILE_NAME_MAX_WIDTH)
                    .finish(),
                );
            if !directory.is_empty() {
                row.add_child(
                    Shrinkable::new(
                        1.,
                        Container::new(
                            Text::new_inline(
                                directory,
                                font_family,
                                font_size * SECONDARY_FONT_SIZE_RATIO,
                            )
                            .with_color(blended_colors::text_sub(theme, theme.background()))
                            .with_clip(ClipConfig::start())
                            .finish(),
                        )
                        .with_margin_left(6.)
                        .finish(),
                    )
                    .finish(),
                );
            }
            Self::finish_row(Clipped::new(row.finish()).finish(), highlight, appearance)
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(NestedReposViewAction::OpenRepo(request.clone()));
        })
        .with_cursor(Cursor::PointingHand)
        .finish()
    }

    /// Wraps a row's content so it fills the row height and shows the hover highlight.
    fn finish_row(
        content: Box<dyn Element>,
        highlight: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let mut container = Container::new(
            ConstrainedBox::new(content)
                .with_height(ROW_HEIGHT)
                .finish(),
        )
        .with_horizontal_padding(ROW_HORIZONTAL_PADDING);
        if let Some(background) = highlight.background_color(appearance) {
            container = container.with_background(background);
        }
        if let Some(corner_radius) = highlight.corner_radius() {
            container = container.with_corner_radius(corner_radius);
        }
        container.finish()
    }
}

impl Entity for NestedReposView {
    type Event = NestedReposViewEvent;
}

impl TypedActionView for NestedReposView {
    type Action = NestedReposViewAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            NestedReposViewAction::ToggleRepo(repo_root) => {
                let is_expanded = self.is_expanded(repo_root);
                self.expanded.insert(repo_root.clone(), !is_expanded);
                self.sync_with_model(ctx);
            }
            NestedReposViewAction::OpenRepo(request) => {
                ctx.emit(NestedReposViewEvent::OpenRepo(request.clone()));
            }
            NestedReposViewAction::Refresh => {
                self.model.update(ctx, |model, ctx| model.refresh(ctx));
            }
        }
    }
}

impl View for NestedReposView {
    fn ui_name() -> &'static str {
        "NestedReposView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let model = self.model.as_ref(app);
        let body = if !model.has_completed_scan() {
            Self::render_message("Scanning for repositories…", appearance)
        } else if model.entries().is_empty() {
            Self::render_message("No repositories found in this folder", appearance)
        } else {
            self.render_list(appearance)
        };
        Flex::column()
            .with_child(self.render_summary(model, appearance))
            .with_child(Shrinkable::new(1., body).finish())
            .finish()
    }
}

#[cfg(test)]
#[path = "nested_repos_view_tests.rs"]
mod tests;
