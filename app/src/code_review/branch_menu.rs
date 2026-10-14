//! Overlay menu of a repository's branches in the source control overview: a search input, the
//! local branches and then the remote ones, a way to name a new branch, and a way to merge a
//! branch into the one that is checked out.
use std::cmp;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use ::hako_remote::protocol::{GitBranch, GitBranchKind, GitBranches, GitMergeKind, GitSwitchKind};
use fuzzy_match::match_indices_case_insensitive;
use warp_core::ui::theme::Fill;
use warp_editor::editor::NavigationKey;
use warpui::color::ColorU;
use warpui::elements::{
    Border, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Dismiss,
    DispatchEventResult, DropShadow, Element, Empty, EventHandler, Expanded, Flex, Highlight,
    MainAxisSize, MouseInBehavior, ParentElement, Radius, ScrollStateHandle, Scrollable,
    ScrollableElement, ScrollbarWidth, Text, UniformList, UniformListState,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::FixedBinding;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Entity, FocusContext, SingletonEntity as _, TypedActionView, View, ViewContext,
    ViewHandle, id,
};

use super::diff_menu::substring_match;
use super::nested_repo_history::one_line;
use crate::appearance::Appearance;
use crate::code::blame::{relative_time, short_sha};
use crate::editor::{
    EditorOptions, EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys, TextOptions,
};
use crate::ui_components::icons::Icon;

const MENU_WIDTH: f32 = 440.;
const MENU_MAX_LIST_HEIGHT: f32 = 320.;
const MENU_CORNER_RADIUS: f32 = 6.;
const ROW_HORIZONTAL_PADDING: f32 = 14.;
const ROW_VERTICAL_PADDING: f32 = 5.;
const LIST_BOTTOM_PADDING: f32 = 9.;
const SEARCH_INPUT_HORIZONTAL_PADDING: f32 = 8.;
const CHECK_GAP: f32 = 8.;
const CHECK_SLOT_SIZE: f32 = 12.;
const DETAIL_MAX_WIDTH: f32 = 220.;
const DETAIL_FONT_SIZE_RATIO: f32 = 0.9;
const MERGE_BUTTON_MAX_WIDTH: f32 = 200.;

const SEARCH_PLACEHOLDER: &str = "Search branches\u{2026}";
const NAME_PLACEHOLDER: &str = "Name of the new branch";
const NEW_BRANCH_LABEL: &str = "New branch\u{2026}";
const LISTING_NOTE: &str = "Listing branches\u{2026}";
const NO_MATCHES_NOTE: &str = "No matching branches";
const TRUNCATED_NOTE: &str = "More branches exist than are listed";
const NAME_NOTE: &str = "Type a name. The branch is made at the current commit.";

/// The branches of the repository the menu is open for, as far as they are known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BranchList {
    Loading,
    Loaded(GitBranches),
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuMode {
    /// The input filters the branches.
    Listing,
    /// The input holds the name of a branch to make.
    Naming,
}

/// One line of the menu's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BranchRow {
    /// Leads to naming a new branch.
    NewBranch,
    Header(&'static str),
    Branch {
        /// Index into the listed branches.
        index: usize,
        /// Characters of the name that matched the search, to be emphasized.
        highlighted: Vec<usize>,
    },
    /// Makes the branch that has been named.
    Create,
    Note(&'static str),
    Error(String),
}

/// Why a branch cannot be switched to from this working tree, when it cannot.
fn unavailable_reason(branch: &GitBranch) -> Option<String> {
    let worktree = branch.worktree.as_deref()?;
    Some(format!("checked out in {}", home_relative(worktree)))
}

/// `path` with the home directory written as `~`.
fn home_relative(path: &str) -> String {
    dirs::home_dir()
        .and_then(|home| {
            let rest = Path::new(path).strip_prefix(&home).ok()?;
            Some(format!("~/{}", rest.display()))
        })
        .unwrap_or_else(|| path.to_owned())
}

fn is_current(branch: &GitBranch, branches: &GitBranches) -> bool {
    branch.kind == GitBranchKind::Local && branches.current.as_deref() == Some(&branch.name)
}

/// The branch a row offers to merge into the one that is checked out, and its kind: any branch
/// but the current one, while a branch is checked out.
pub(crate) fn merge_target(row: &BranchRow, list: &BranchList) -> Option<(String, GitMergeKind)> {
    let (BranchRow::Branch { index, .. }, BranchList::Loaded(branches)) = (row, list) else {
        return None;
    };
    let branch = branches.branches.get(*index)?;
    if branches.current.is_none() || is_current(branch, branches) {
        return None;
    }
    let kind = match branch.kind {
        GitBranchKind::Local => GitMergeKind::Local,
        GitBranchKind::Remote => GitMergeKind::Remote,
    };
    Some((branch.name.clone(), kind))
}

/// Whether the row can be focused: it can be picked, or it offers a merge.
fn is_focusable(row: &BranchRow, list: &BranchList) -> bool {
    is_selectable(row, list) || merge_target(row, list).is_some()
}

/// Whether picking the row does something.
pub(crate) fn is_selectable(row: &BranchRow, list: &BranchList) -> bool {
    match row {
        BranchRow::NewBranch | BranchRow::Create => true,
        BranchRow::Branch { index, .. } => match list {
            BranchList::Loaded(branches) => branches
                .branches
                .get(*index)
                .is_some_and(|branch| branch.worktree.is_none() && !is_current(branch, branches)),
            BranchList::Loading | BranchList::Failed(_) => false,
        },
        BranchRow::Header(_) | BranchRow::Note(_) | BranchRow::Error(_) => false,
    }
}

fn section_title(kind: GitBranchKind) -> &'static str {
    match kind {
        GitBranchKind::Local => "Branches",
        GitBranchKind::Remote => "Remote branches",
    }
}

/// Whether a branch passes the search, and if so which characters of its name matched.
fn branch_match(branch: &GitBranch, query: &str) -> Option<Vec<usize>> {
    substring_match(&branch.name, query).or_else(|| {
        match_indices_case_insensitive(&branch.name, query).map(|matched| matched.matched_indices)
    })
}

/// Builds the menu's list. While branches are listed, that is the way to a new branch and then
/// the branches that pass the search, in the order git lists them under the heading of their
/// kind. While a branch is named, it is the one row that makes it.
pub(crate) fn branch_rows(list: &BranchList, query: &str, mode: MenuMode) -> Vec<BranchRow> {
    let query = query.trim();
    match mode {
        MenuMode::Naming => {
            return if query.is_empty() {
                vec![BranchRow::Note(NAME_NOTE)]
            } else {
                vec![BranchRow::Create]
            };
        }
        MenuMode::Listing => {}
    }

    let mut rows = vec![BranchRow::NewBranch];
    let branches = match list {
        BranchList::Loading => {
            rows.push(BranchRow::Note(LISTING_NOTE));
            return rows;
        }
        BranchList::Failed(message) => {
            rows.push(BranchRow::Error(one_line(message)));
            return rows;
        }
        BranchList::Loaded(branches) => branches,
    };
    let mut section = None;
    let mut matched = 0;
    for (index, branch) in branches.branches.iter().enumerate() {
        let highlighted = if query.is_empty() {
            Vec::new()
        } else {
            match branch_match(branch, query) {
                Some(highlighted) => highlighted,
                None => continue,
            }
        };
        if section != Some(branch.kind) {
            section = Some(branch.kind);
            rows.push(BranchRow::Header(section_title(branch.kind)));
        }
        rows.push(BranchRow::Branch { index, highlighted });
        matched += 1;
    }
    if matched == 0 && !query.is_empty() {
        rows.push(BranchRow::Note(NO_MATCHES_NOTE));
    }
    if branches.truncated {
        rows.push(BranchRow::Note(TRUNCATED_NOTE));
    }
    rows
}

fn tracking_counts(branch: &GitBranch) -> Option<String> {
    let counts: Vec<String> = [(branch.ahead, '\u{2191}'), (branch.behind, '\u{2193}')]
        .into_iter()
        .filter_map(|(count, arrow)| match count {
            Some(count) if count > 0 => Some(format!("{arrow}{count}")),
            Some(_) | None => None,
        })
        .collect();
    (!counts.is_empty()).then(|| counts.join(" "))
}

/// The secondary text of a branch: why it cannot be switched to, or how far it is from its
/// upstream and when its last commit was made. `now` is in seconds since the Unix epoch.
pub(crate) fn branch_detail(branch: &GitBranch, now: i64) -> String {
    if let Some(reason) = unavailable_reason(branch) {
        return reason;
    }
    [
        tracking_counts(branch),
        Some(relative_time(branch.time / 1000, now)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" \u{b7} ")
}

/// The focusable row nearest to `from` in the given direction, if there is one.
fn next_selectable_row(
    rows: &[BranchRow],
    list: &BranchList,
    from: usize,
    forward: bool,
) -> Option<usize> {
    if forward {
        (from + 1..rows.len()).find(|index| is_focusable(&rows[*index], list))
    } else {
        (0..from.min(rows.len()))
            .rev()
            .find(|index| is_focusable(&rows[*index], list))
    }
}

#[derive(Clone, Debug)]
pub enum BranchMenuEvent {
    Switch {
        branch: String,
        kind: GitSwitchKind,
    },
    /// Merge the branch into the one that is checked out.
    Merge {
        branch: String,
        kind: GitMergeKind,
    },
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BranchMenuAction {
    ClickRow {
        index: usize,
    },
    HoverRow {
        index: usize,
    },
    MergeRow {
        index: usize,
    },
    SelectUp,
    SelectDown,
    SelectEnter,
    /// Leaves the naming of a new branch for the list, and the list for the panel.
    Back,
    Close,
}

pub fn init(app: &mut AppContext) {
    app.register_fixed_bindings([
        FixedBinding::new("up", BranchMenuAction::SelectUp, id!(BranchMenu::ui_name())),
        FixedBinding::new(
            "down",
            BranchMenuAction::SelectDown,
            id!(BranchMenu::ui_name()),
        ),
        FixedBinding::new(
            "enter",
            BranchMenuAction::SelectEnter,
            id!(BranchMenu::ui_name()),
        ),
        FixedBinding::new("escape", BranchMenuAction::Back, id!(BranchMenu::ui_name())),
    ]);
}

pub struct BranchMenu {
    list: BranchList,
    mode: MenuMode,
    /// The list as currently filtered by the search.
    rows: Vec<BranchRow>,
    /// Index into `rows` of the keyboard-focused row.
    selected_index: Option<usize>,
    search_input: ViewHandle<EditorView>,
    search_query: String,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
}

impl BranchMenu {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let search_input = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let ui_font_family = appearance.ui_font_family();

            let mut text_options = TextOptions::ui_font_size(appearance);
            text_options.font_family_override = Some(ui_font_family);

            let options = EditorOptions {
                autogrow: false,
                soft_wrap: false,
                single_line: true,
                text: text_options,
                propagate_and_no_op_vertical_navigation_keys:
                    PropagateAndNoOpNavigationKeys::Always,
                ..Default::default()
            };
            let mut editor = EditorView::new(options, ctx);
            editor.set_placeholder_text(SEARCH_PLACEHOLDER, ctx);
            editor
        });

        ctx.subscribe_to_view(&search_input, |menu, _, event, ctx| match event {
            EditorEvent::Edited(_) => {
                let new_query = menu
                    .search_input
                    .read(ctx, |editor, ctx| editor.buffer_text(ctx).to_string());
                if new_query != menu.search_query {
                    menu.search_query = new_query;
                    menu.refresh_rows();
                    menu.scroll_list_to_top();
                    ctx.notify();
                }
            }
            EditorEvent::Escape => menu.go_back(ctx),
            EditorEvent::Navigate(NavigationKey::Up) => menu.select_prev(ctx),
            EditorEvent::Navigate(NavigationKey::Down) => menu.select_next(ctx),
            EditorEvent::Enter => menu.select_enter(ctx),
            _ => {}
        });

        let mut menu = Self {
            list: BranchList::Loading,
            mode: MenuMode::Listing,
            rows: Vec::new(),
            selected_index: None,
            search_input,
            search_query: String::new(),
            list_state: Default::default(),
            scroll_state: Default::default(),
        };
        menu.refresh_rows();
        menu
    }

    /// Puts the menu back to how it opens: no search, and branches still to be listed.
    pub fn reset(&mut self, ctx: &mut ViewContext<Self>) {
        self.list = BranchList::Loading;
        self.set_mode(MenuMode::Listing, String::new(), ctx);
    }

    /// Takes in the branches that were listed. The search stays as typed.
    pub fn set_branches(
        &mut self,
        result: Result<GitBranches, String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let list = match result {
            Ok(branches) => BranchList::Loaded(branches),
            Err(message) => BranchList::Failed(message),
        };
        if self.list == list {
            return;
        }
        let selected = self
            .selected_index
            .and_then(|index| self.rows.get(index).cloned());
        self.list = list;
        self.refresh_rows();
        if let Some(index) =
            selected.and_then(|row| self.rows.iter().position(|candidate| *candidate == row))
        {
            self.selected_index = Some(index);
        }
        ctx.notify();
    }

    fn first_branch_row(&self) -> Option<usize> {
        self.rows.iter().position(|row| {
            matches!(row, BranchRow::Branch { .. }) && is_selectable(row, &self.list)
        })
    }

    fn set_mode(&mut self, mode: MenuMode, text: String, ctx: &mut ViewContext<Self>) {
        self.mode = mode;
        self.search_query = text.clone();
        let placeholder = match mode {
            MenuMode::Listing => SEARCH_PLACEHOLDER,
            MenuMode::Naming => NAME_PLACEHOLDER,
        };
        self.search_input.update(ctx, |editor, ctx| {
            editor.set_placeholder_text(placeholder, ctx);
            editor.set_buffer_text(&text, ctx);
        });
        self.refresh_rows();
        self.scroll_list_to_top();
        ctx.notify();
    }

    fn scroll_list_to_top(&mut self) {
        if !self.rows.is_empty() {
            self.list_state.scroll_to(0);
        }
    }

    fn refresh_rows(&mut self) {
        self.rows = branch_rows(&self.list, &self.search_query, self.mode);
        let is_searching = !self.search_query.trim().is_empty();
        self.selected_index = match self.mode {
            MenuMode::Listing if is_searching => self.first_branch_row(),
            MenuMode::Listing | MenuMode::Naming => None,
        }
        .or_else(|| {
            self.rows
                .iter()
                .position(|row| is_selectable(row, &self.list))
        });
    }

    fn select_prev(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(current) = self.selected_index else {
            return;
        };
        let Some(previous) = next_selectable_row(&self.rows, &self.list, current, false) else {
            self.list_state.scroll_to(0);
            ctx.notify();
            return;
        };
        self.selected_index = Some(previous);
        self.list_state.scroll_to(previous);
        ctx.notify();
    }

    fn select_next(&mut self, ctx: &mut ViewContext<Self>) {
        let next = match self.selected_index {
            Some(current) => next_selectable_row(&self.rows, &self.list, current, true),
            None => self
                .rows
                .iter()
                .position(|row| is_focusable(row, &self.list)),
        };
        let Some(next) = next else {
            return;
        };
        self.selected_index = Some(next);
        self.list_state.scroll_to(next);
        ctx.notify();
    }

    fn select_enter(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(selected) = self.selected_index {
            self.select_row(selected, ctx);
        }
    }

    fn select_row(&mut self, row_index: usize, ctx: &mut ViewContext<Self>) {
        let Some(row) = self.rows.get(row_index).cloned() else {
            return;
        };
        if !is_selectable(&row, &self.list) {
            return;
        }
        match row {
            BranchRow::NewBranch => {
                let typed = self.search_query.trim().to_owned();
                self.set_mode(MenuMode::Naming, typed, ctx);
            }
            BranchRow::Create => ctx.emit(BranchMenuEvent::Switch {
                branch: self.search_query.trim().to_owned(),
                kind: GitSwitchKind::New,
            }),
            BranchRow::Branch { index, .. } => {
                let BranchList::Loaded(branches) = &self.list else {
                    return;
                };
                let Some(branch) = branches.branches.get(index) else {
                    return;
                };
                ctx.emit(BranchMenuEvent::Switch {
                    branch: branch.name.clone(),
                    kind: match branch.kind {
                        GitBranchKind::Local => GitSwitchKind::Local,
                        GitBranchKind::Remote => GitSwitchKind::Remote,
                    },
                });
            }
            BranchRow::Header(_) | BranchRow::Note(_) | BranchRow::Error(_) => {}
        }
    }

    fn merge_row(&mut self, row_index: usize, ctx: &mut ViewContext<Self>) {
        let target = self
            .rows
            .get(row_index)
            .and_then(|row| merge_target(row, &self.list));
        if let Some((branch, kind)) = target {
            ctx.emit(BranchMenuEvent::Merge { branch, kind });
        }
    }

    fn go_back(&mut self, ctx: &mut ViewContext<Self>) {
        match self.mode {
            MenuMode::Naming => self.set_mode(MenuMode::Listing, String::new(), ctx),
            MenuMode::Listing => ctx.emit(BranchMenuEvent::Close),
        }
    }

    fn render_search_input(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let search_input = appearance
            .ui_builder()
            .text_input(self.search_input.clone())
            .with_style(UiComponentStyles {
                background: Some(Fill::Solid(ColorU::new(0, 0, 0, 0)).into()),
                border_color: None,
                border_width: Some(0.),
                border_radius: None,
                width: Some(MENU_WIDTH - (SEARCH_INPUT_HORIZONTAL_PADDING * 2.)),
                padding: Some(Coords::uniform(4.)),
                ..Default::default()
            })
            .build()
            .finish();

        Container::new(search_input)
            .with_horizontal_padding(SEARCH_INPUT_HORIZONTAL_PADDING)
            .with_vertical_padding(2.)
            .with_border(Border::bottom(1.0).with_border_color(theme.outline().into()))
            .finish()
    }

    /// What each row of the list shows, detached from the menu so that the list can own it.
    fn displayed_rows(&self) -> Vec<DisplayedRow> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() as i64);
        let (branches, head) = match &self.list {
            BranchList::Loaded(branches) => (Some(branches), branches.head.as_deref()),
            BranchList::Loading | BranchList::Failed(_) => (None, None),
        };
        let current = branches.and_then(|branches| branches.current.as_deref());
        self.rows
            .iter()
            .map(|row| {
                let is_enabled = is_selectable(row, &self.list);
                let is_focusable = is_focusable(row, &self.list);
                let merge_label = merge_target(row, &self.list)
                    .zip(current)
                    .map(|(_, current)| format!("Merge into {current}"));
                match row {
                    BranchRow::NewBranch => DisplayedRow::Item {
                        label: NEW_BRANCH_LABEL.to_owned(),
                        detail: None,
                        highlighted: Vec::new(),
                        mark: RowMark::Plus,
                        is_enabled,
                        is_focusable,
                        merge_label: None,
                    },
                    BranchRow::Create => DisplayedRow::Item {
                        label: format!(
                            "Create branch \u{2018}{}\u{2019}",
                            self.search_query.trim()
                        ),
                        detail: head.map(|head| format!("at {}", short_sha(head))),
                        highlighted: Vec::new(),
                        mark: RowMark::Plus,
                        is_enabled,
                        is_focusable,
                        merge_label: None,
                    },
                    BranchRow::Branch { index, highlighted } => {
                        match branches
                            .and_then(|branches| Some((branches.branches.get(*index)?, branches)))
                        {
                            Some((branch, branches)) => DisplayedRow::Item {
                                label: branch.name.clone(),
                                detail: Some(branch_detail(branch, now)),
                                highlighted: highlighted.clone(),
                                mark: if is_current(branch, branches) {
                                    RowMark::Check
                                } else {
                                    RowMark::None
                                },
                                // The current branch is not picked, and reads as usual.
                                is_enabled: is_enabled || is_current(branch, branches),
                                is_focusable,
                                merge_label,
                            },
                            None => DisplayedRow::Note(String::new()),
                        }
                    }
                    BranchRow::Header(title) => DisplayedRow::Header(*title),
                    BranchRow::Note(note) => DisplayedRow::Note((*note).to_owned()),
                    BranchRow::Error(message) => DisplayedRow::Error(message.clone()),
                }
            })
            .collect()
    }

    fn render_rows(&self, ctx: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(ctx);
        let theme = appearance.theme();
        let selected = self.selected_index;
        let displayed_rows = self.displayed_rows();
        let row_count = displayed_rows.len();

        let list = UniformList::new(self.list_state.clone(), row_count, move |mut range, app| {
            let appearance = Appearance::as_ref(app);
            range.end = cmp::min(range.end, row_count);
            range
                .map(|row_index| {
                    displayed_rows[row_index].render(
                        row_index,
                        selected == Some(row_index),
                        appearance,
                    )
                })
                .collect::<Vec<_>>()
                .into_iter()
        });

        let scrollable = Scrollable::vertical(
            self.scroll_state.clone(),
            list.finish_scrollable(),
            ScrollbarWidth::None,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            warpui::elements::Fill::None,
        )
        .with_padding_end(0.)
        .with_padding_start(0.);

        ConstrainedBox::new(scrollable.finish())
            .with_width(MENU_WIDTH)
            .with_max_height(MENU_MAX_LIST_HEIGHT)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowMark {
    None,
    Check,
    Plus,
}

/// A row of the list as it is drawn.
enum DisplayedRow {
    Header(&'static str),
    Item {
        label: String,
        detail: Option<String>,
        highlighted: Vec<usize>,
        mark: RowMark,
        is_enabled: bool,
        /// Takes the focus when hovered, which an item that only offers a merge does too.
        is_focusable: bool,
        /// Shown on the focused item, which then merges its branch when clicked.
        merge_label: Option<String>,
    },
    Error(String),
    Note(String),
}

impl DisplayedRow {
    /// Every kind of row is one line of text at the same size, which keeps the rows of the list
    /// the same height.
    fn render(
        &self,
        row_index: usize,
        is_focused: bool,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font_family = appearance.ui_font_family();
        let font_size = appearance.ui_font_size();
        let sub_text_color = theme.sub_text_color(theme.surface_2()).into_solid();

        let (label, detail, highlighted, mark, is_enabled, is_focusable, merge_label) = match self {
            DisplayedRow::Item {
                label,
                detail,
                highlighted,
                mark,
                is_enabled,
                is_focusable,
                merge_label,
            } => (
                label,
                detail,
                highlighted,
                *mark,
                *is_enabled,
                *is_focusable,
                merge_label,
            ),
            DisplayedRow::Header(title) => {
                return Self::render_plain(
                    Text::new_inline(*title, font_family, font_size)
                        .with_style(Properties::default().weight(Weight::Semibold))
                        .with_color(sub_text_color)
                        .finish(),
                    0.,
                );
            }
            DisplayedRow::Error(message) => {
                return Self::render_plain(
                    Text::new_inline(message.clone(), font_family, font_size)
                        .with_color(theme.ui_error_color())
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                    CHECK_SLOT_SIZE + CHECK_GAP,
                );
            }
            DisplayedRow::Note(note) => {
                return Self::render_plain(
                    Text::new_inline(note.clone(), font_family, font_size)
                        .with_color(sub_text_color)
                        .finish(),
                    CHECK_SLOT_SIZE + CHECK_GAP,
                );
            }
        };

        let (text_color, detail_color, background) = if is_focused && is_focusable {
            let background = theme.accent();
            let text_color = theme.main_text_color(background).into_solid();
            (text_color, text_color, Some(background))
        } else if is_enabled {
            (
                theme.main_text_color(theme.surface_2()).into_solid(),
                sub_text_color,
                None,
            )
        } else {
            let disabled = theme.disabled_text_color(theme.surface_2()).into_solid();
            (disabled, disabled, None)
        };

        let mark: Box<dyn Element> = match mark {
            RowMark::Check => Icon::Check.to_warpui_icon(Fill::Solid(text_color)).finish(),
            RowMark::Plus => Icon::Plus.to_warpui_icon(Fill::Solid(text_color)).finish(),
            RowMark::None => Empty::new().finish(),
        };
        let mark_slot = ConstrainedBox::new(mark)
            .with_width(CHECK_SLOT_SIZE)
            .with_height(CHECK_SLOT_SIZE)
            .finish();

        let mut label_text = Text::new_inline(label.clone(), font_family, font_size)
            .with_color(text_color)
            .with_clip(ClipConfig::ellipsis());
        if !highlighted.is_empty() {
            label_text = label_text.with_single_highlight(
                Highlight::new()
                    .with_properties(Properties::default().weight(Weight::Bold))
                    .with_foreground_color(text_color),
                highlighted.clone(),
            );
        }

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(
                Container::new(mark_slot)
                    .with_margin_right(CHECK_GAP)
                    .finish(),
            )
            .with_child(Expanded::new(1., label_text.finish()).finish());
        let merge_label = merge_label.as_ref().filter(|_| is_focused);
        if let Some(merge_label) = merge_label {
            row.add_child(Self::render_merge_button(
                merge_label,
                row_index,
                appearance,
            ));
        } else if let Some(detail) = detail {
            row.add_child(
                Container::new(
                    ConstrainedBox::new(
                        Text::new_inline(
                            detail.clone(),
                            font_family,
                            font_size * DETAIL_FONT_SIZE_RATIO,
                        )
                        .with_color(detail_color)
                        // The end of a path says more than its start.
                        .with_clip(ClipConfig::start())
                        .finish(),
                    )
                    .with_max_width(DETAIL_MAX_WIDTH)
                    .finish(),
                )
                .with_margin_left(12.)
                .finish(),
            );
        }

        let mut container = Container::new(row.finish())
            .with_horizontal_padding(ROW_HORIZONTAL_PADDING)
            .with_vertical_padding(ROW_VERTICAL_PADDING);
        if let Some(background) = background {
            container = container.with_background(background);
        }
        if !is_focusable {
            return container.finish();
        }

        EventHandler::new(container.finish())
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action(BranchMenuAction::ClickRow { index: row_index });
                DispatchEventResult::StopPropagation
            })
            .on_mouse_in(
                move |ctx, _, _| {
                    ctx.dispatch_typed_action(BranchMenuAction::HoverRow { index: row_index });
                    ctx.notify();
                    DispatchEventResult::StopPropagation
                },
                Some(MouseInBehavior {
                    fire_on_synthetic_events: false,
                    fire_when_covered: false,
                }),
            )
            .finish()
    }

    /// The secondary action of the focused branch, which merges it into the current one.
    fn render_merge_button(
        label: &str,
        row_index: usize,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let background = theme.surface_2();
        let button = Container::new(
            ConstrainedBox::new(
                Text::new_inline(
                    label.to_owned(),
                    appearance.ui_font_family(),
                    appearance.ui_font_size() * DETAIL_FONT_SIZE_RATIO,
                )
                .with_color(theme.main_text_color(background).into_solid())
                .with_clip(ClipConfig::ellipsis())
                .finish(),
            )
            .with_max_width(MERGE_BUTTON_MAX_WIDTH)
            .finish(),
        )
        .with_horizontal_padding(6.)
        .with_vertical_padding(1.)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .with_background(background)
        .finish();
        Container::new(
            EventHandler::new(button)
                .on_left_mouse_down(move |ctx, _, _| {
                    ctx.dispatch_typed_action(BranchMenuAction::MergeRow { index: row_index });
                    DispatchEventResult::StopPropagation
                })
                .finish(),
        )
        .with_margin_left(12.)
        .finish()
    }

    /// A row that cannot be picked: a heading, an error or a note.
    fn render_plain(text: Box<dyn Element>, indent: f32) -> Box<dyn Element> {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(Expanded::new(1., text).finish())
                .finish(),
        )
        .with_padding_left(ROW_HORIZONTAL_PADDING + indent)
        .with_padding_right(ROW_HORIZONTAL_PADDING)
        .with_vertical_padding(ROW_VERTICAL_PADDING)
        .finish()
    }
}

impl Entity for BranchMenu {
    type Event = BranchMenuEvent;
}

impl TypedActionView for BranchMenu {
    type Action = BranchMenuAction;

    fn handle_action(&mut self, action: &BranchMenuAction, ctx: &mut ViewContext<Self>) {
        match action {
            BranchMenuAction::ClickRow { index } => {
                if self
                    .rows
                    .get(*index)
                    .is_some_and(|row| is_selectable(row, &self.list))
                {
                    self.selected_index = Some(*index);
                    self.select_row(*index, ctx);
                }
            }
            BranchMenuAction::HoverRow { index } => {
                if self
                    .rows
                    .get(*index)
                    .is_some_and(|row| is_focusable(row, &self.list))
                    && self.selected_index != Some(*index)
                {
                    self.selected_index = Some(*index);
                    ctx.notify();
                }
            }
            BranchMenuAction::MergeRow { index } => self.merge_row(*index, ctx),
            BranchMenuAction::SelectUp => self.select_prev(ctx),
            BranchMenuAction::SelectDown => self.select_next(ctx),
            BranchMenuAction::SelectEnter => self.select_enter(ctx),
            BranchMenuAction::Back => self.go_back(ctx),
            BranchMenuAction::Close => ctx.emit(BranchMenuEvent::Close),
        }
    }
}

impl View for BranchMenu {
    fn ui_name() -> &'static str {
        "SourceControlBranchMenu"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.search_input);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let mut column = Flex::column().with_child(self.render_search_input(appearance));
        if !self.rows.is_empty() {
            column.add_child(
                Container::new(self.render_rows(app))
                    .with_padding_bottom(LIST_BOTTOM_PADDING)
                    .finish(),
            );
        }

        let menu_card = ConstrainedBox::new(
            Container::new(column.finish())
                .with_background(theme.surface_2())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(MENU_CORNER_RADIUS)))
                .with_drop_shadow(DropShadow::default())
                .finish(),
        )
        .with_width(MENU_WIDTH)
        .finish();

        Dismiss::new(menu_card)
            .on_dismiss(|ctx, _app| {
                ctx.dispatch_typed_action(BranchMenuAction::Close);
            })
            .prevent_interaction_with_other_elements()
            .finish()
    }
}

#[cfg(test)]
#[path = "branch_menu_tests.rs"]
mod tests;
