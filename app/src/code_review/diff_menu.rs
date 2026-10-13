//! Overlay menu for the code review diff selector: pinned search input, a choice of how
//! branches and revisions are compared, and a filtered list of rows grouped by kind.
use std::cmp;

use fuzzy_match::match_indices_case_insensitive;
use warp_core::ui::theme::Fill;
use warp_editor::editor::NavigationKey;
use warpui::color::ColorU;
use warpui::elements::{
    Border, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Dismiss,
    DispatchEventResult, DropShadow, Element, Empty, EventHandler, Expanded, Flex, Highlight,
    MainAxisSize, MouseInBehavior, MouseStateHandle, ParentElement, Radius, ScrollStateHandle,
    Scrollable, ScrollableElement, ScrollbarWidth, Text, UniformList, UniformListState,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::FixedBinding;
use warpui::platform::Cursor;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Entity, FocusContext, SingletonEntity as _, TypedActionView, View, ViewContext,
    ViewHandle, id,
};

use crate::appearance::Appearance;
use crate::code_review::diff_selector::{DiffTarget, DiffTargetBase, DiffTargetSection};
use crate::code_review::diff_state::{DiffMode, RevisionComparison};
use crate::code_review::revisions::parse_revision_input;
use crate::editor::{
    EditorOptions, EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys, TextOptions,
};
use crate::ui_components::icons::Icon;

const MENU_WIDTH: f32 = 480.;
const MENU_MAX_LIST_HEIGHT: f32 = 320.;
const MENU_CORNER_RADIUS: f32 = 6.;
const ROW_HORIZONTAL_PADDING: f32 = 14.;
const ROW_VERTICAL_PADDING: f32 = 5.;
const LIST_BOTTOM_PADDING: f32 = 9.;
const SEARCH_INPUT_HORIZONTAL_PADDING: f32 = 8.;
const CHECK_GAP: f32 = 8.;
const CHECK_SLOT_SIZE: f32 = 12.;
const DETAIL_MAX_WIDTH: f32 = 200.;
const DETAIL_FONT_SIZE_RATIO: f32 = 0.9;
const COMPARISON_BUTTON_CORNER_RADIUS: f32 = 4.;

const NO_MATCHES_NOTE: &str = "No matches";
const LOADING_REVISIONS_NOTE: &str = "Listing remote branches, tags and commits\u{2026}";
const MERGE_BASE_TOOLTIP: &str = "Changes since the working tree's branch and the picked one diverged (git diff $(git merge-base HEAD <revision>))";
const DIRECT_TOOLTIP: &str =
    "Everything that differs from the picked revision itself (git diff <revision>)";

/// Progress of comparing with the text typed into the search input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum TypedRevisionState {
    #[default]
    Idle,
    /// Git is being asked whether the text names a commit.
    Checking,
    Failed(String),
}

/// One line of the menu's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MenuRow {
    Header(&'static str),
    Target {
        /// Index into the menu's targets.
        index: usize,
        /// Characters of the label that matched the search, to be emphasized.
        highlighted: Vec<usize>,
    },
    /// The typed text, offered as a revision to compare with.
    TypedRevision,
    Error(String),
    Note(&'static str),
}

impl MenuRow {
    fn is_selectable(&self) -> bool {
        match self {
            MenuRow::Target { .. } | MenuRow::TypedRevision => true,
            MenuRow::Header(_) | MenuRow::Error(_) | MenuRow::Note(_) => false,
        }
    }
}

/// Position of `needle` in `haystack` ignoring case, as the indices of the matched characters.
pub(super) fn substring_match(haystack: &str, needle: &str) -> Option<Vec<usize>> {
    let fold = |text: &str| -> Vec<char> {
        text.chars()
            .map(|character| character.to_lowercase().next().unwrap_or(character))
            .collect()
    };
    let (haystack, needle) = (fold(haystack), fold(needle));
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle.as_slice())
        .map(|start| (start..start + needle.len()).collect())
}

/// Whether a target passes the search, and if so which characters of its label matched.
///
/// Names are matched loosely, as they are typed from memory. Commits are matched literally, in
/// their message, id, author or age: a loose match in prose matches nearly everything.
fn target_match(target: &DiffTarget, query: &str) -> Option<Vec<usize>> {
    if let Some(highlighted) = substring_match(&target.label, query) {
        return Some(highlighted);
    }
    if target
        .detail
        .as_deref()
        .is_some_and(|detail| substring_match(detail, query).is_some())
    {
        return Some(Vec::new());
    }
    match target.section {
        DiffTargetSection::WorkingTree
        | DiffTargetSection::Branches
        | DiffTargetSection::RemoteBranches
        | DiffTargetSection::Tags => match_indices_case_insensitive(&target.label, query)
            .map(|matched| matched.matched_indices),
        DiffTargetSection::Commits => None,
    }
}

/// Whether picking `target` is the same as comparing with the typed `rev`.
fn names_revision(target: &DiffTarget, rev: &str) -> bool {
    match &target.base {
        DiffTargetBase::UncommittedChanges => false,
        DiffTargetBase::MainBranch(name)
        | DiffTargetBase::Branch(name)
        | DiffTargetBase::Revision(name) => name == rev,
    }
}

/// What the rows of the menu depend on besides the targets and the search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MenuRowOptions {
    /// Whether typed text can be compared with, when it is not one of the targets.
    pub offers_revisions: bool,
    pub is_loading_revisions: bool,
    pub typed_revision: TypedRevisionState,
}

/// Builds the menu's list: the targets that pass the search, in their order and under the
/// heading of their group, then the typed text as a revision of its own.
pub(crate) fn menu_rows(
    targets: &[DiffTarget],
    query: &str,
    options: &MenuRowOptions,
) -> Vec<MenuRow> {
    let query = query.trim();
    let mut rows = Vec::new();
    let mut section = None;
    for (index, target) in targets.iter().enumerate() {
        let highlighted = if query.is_empty() {
            Vec::new()
        } else {
            match target_match(target, query) {
                Some(highlighted) => highlighted,
                None => continue,
            }
        };
        if section != Some(target.section) {
            section = Some(target.section);
            rows.extend(target.section.title().map(MenuRow::Header));
        }
        rows.push(MenuRow::Target { index, highlighted });
    }

    let is_typed_revision_offered = options.offers_revisions
        && !query.is_empty()
        && !targets.iter().any(|target| names_revision(target, query));
    if is_typed_revision_offered {
        rows.push(MenuRow::TypedRevision);
        match &options.typed_revision {
            TypedRevisionState::Idle | TypedRevisionState::Checking => {}
            TypedRevisionState::Failed(message) => rows.push(MenuRow::Error(message.clone())),
        }
    }
    if options.is_loading_revisions {
        rows.push(MenuRow::Note(LOADING_REVISIONS_NOTE));
    }
    if rows.is_empty() && !query.is_empty() {
        rows.push(MenuRow::Note(NO_MATCHES_NOTE));
    }
    rows
}

/// The selectable row nearest to `from` in the given direction, if there is one.
pub(crate) fn next_selectable_row(rows: &[MenuRow], from: usize, forward: bool) -> Option<usize> {
    if forward {
        (from + 1..rows.len()).find(|index| rows[*index].is_selectable())
    } else {
        (0..from.min(rows.len()))
            .rev()
            .find(|index| rows[*index].is_selectable())
    }
}

#[derive(Clone, Debug)]
pub enum CodeReviewDiffMenuEvent {
    Select(DiffMode),
    /// Typed text was picked. It is compared with once git confirms that it names a commit.
    ResolveRevision {
        rev: String,
        comparison: RevisionComparison,
    },
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CodeReviewDiffMenuAction {
    ClickRow { index: usize },
    HoverRow { index: usize },
    SelectUp,
    SelectDown,
    SelectEnter,
    SetComparison(RevisionComparison),
    Close,
}

pub fn init(app: &mut AppContext) {
    app.register_fixed_bindings([
        FixedBinding::new(
            "up",
            CodeReviewDiffMenuAction::SelectUp,
            id!(CodeReviewDiffMenu::ui_name()),
        ),
        FixedBinding::new(
            "down",
            CodeReviewDiffMenuAction::SelectDown,
            id!(CodeReviewDiffMenu::ui_name()),
        ),
        FixedBinding::new(
            "enter",
            CodeReviewDiffMenuAction::SelectEnter,
            id!(CodeReviewDiffMenu::ui_name()),
        ),
        FixedBinding::new(
            "escape",
            CodeReviewDiffMenuAction::Close,
            id!(CodeReviewDiffMenu::ui_name()),
        ),
    ]);
}

pub struct CodeReviewDiffMenu {
    targets: Vec<DiffTarget>,
    /// The list as currently filtered by the search.
    rows: Vec<MenuRow>,
    /// Index into `rows` of the keyboard-focused row.
    selected_index: Option<usize>,
    /// How a picked branch or revision is compared.
    comparison: RevisionComparison,
    /// How the base in effect is compared, which is what the choice starts at when the menu
    /// opens.
    comparison_in_effect: RevisionComparison,
    row_options: MenuRowOptions,
    search_input: ViewHandle<EditorView>,
    search_query: String,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
    merge_base_button: MouseStateHandle,
    direct_button: MouseStateHandle,
}

impl CodeReviewDiffMenu {
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
            editor.set_placeholder_text(
                "Search branches, tags and commits, or type a revision\u{2026}",
                ctx,
            );
            editor
        });

        ctx.subscribe_to_view(&search_input, |menu, _, event, ctx| match event {
            EditorEvent::Edited(_) => {
                let new_query = menu
                    .search_input
                    .read(ctx, |editor, ctx| editor.buffer_text(ctx).to_string());
                if new_query != menu.search_query {
                    menu.set_search_query(new_query, ctx);
                }
            }
            EditorEvent::Escape => menu.emit_close(ctx),
            EditorEvent::Navigate(NavigationKey::Up) => menu.select_prev(ctx),
            EditorEvent::Navigate(NavigationKey::Down) => menu.select_next(ctx),
            EditorEvent::Enter => menu.select_enter(ctx),
            _ => {}
        });

        Self {
            targets: Vec::new(),
            rows: Vec::new(),
            selected_index: None,
            comparison: RevisionComparison::default(),
            comparison_in_effect: RevisionComparison::default(),
            row_options: MenuRowOptions::default(),
            search_input,
            search_query: String::new(),
            list_state: Default::default(),
            scroll_state: Default::default(),
            merge_base_button: Default::default(),
            direct_button: Default::default(),
        }
    }

    /// Replace the row set. The search stays as typed, so that rows listed while the menu is
    /// open appear under it.
    pub fn set_content(
        &mut self,
        targets: Vec<DiffTarget>,
        comparison: RevisionComparison,
        offers_revisions: bool,
        is_loading_revisions: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let is_same_list = self.targets == targets;
        self.targets = targets;
        self.comparison_in_effect = comparison;
        self.row_options.offers_revisions = offers_revisions;
        self.row_options.is_loading_revisions = is_loading_revisions;
        if is_same_list {
            self.refresh_rows_keeping_selection();
        } else {
            self.refresh_rows();
            self.scroll_list_to_top();
        }
        ctx.notify();
    }

    /// Reset the menu to a fresh state: empty query, selection/scroll back
    /// to the top, and rows re-computed against the current targets.
    pub fn reset(&mut self, ctx: &mut ViewContext<Self>) {
        self.search_query.clear();
        self.comparison = self.comparison_in_effect;
        self.row_options.typed_revision = TypedRevisionState::Idle;
        self.search_input.update(ctx, |editor, ctx| {
            editor.clear_buffer(ctx);
        });
        self.refresh_rows();
        self.scroll_list_to_top();
        ctx.notify();
    }

    /// Reports that the typed revision `rev` cannot be compared with.
    pub fn show_revision_error(&mut self, rev: &str, message: String, ctx: &mut ViewContext<Self>) {
        // The answer to a revision that has since been edited is of no use.
        if self.search_query.trim() != rev {
            return;
        }
        self.row_options.typed_revision = TypedRevisionState::Failed(message);
        self.refresh_rows_keeping_selection();
        ctx.notify();
    }

    fn set_search_query(&mut self, query: String, ctx: &mut ViewContext<Self>) {
        self.search_query = query;
        self.row_options.typed_revision = TypedRevisionState::Idle;
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
        self.rows = menu_rows(&self.targets, &self.search_query, &self.row_options);
        self.selected_index = self.rows.iter().position(MenuRow::is_selectable);
    }

    fn refresh_rows_keeping_selection(&mut self) {
        let selected = self
            .selected_index
            .and_then(|index| self.rows.get(index).cloned());
        self.refresh_rows();
        if let Some(index) = selected.and_then(|row| self.rows.iter().position(|r| *r == row)) {
            self.selected_index = Some(index);
        }
    }

    fn select_prev(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(current) = self.selected_index else {
            return;
        };
        let Some(previous) = next_selectable_row(&self.rows, current, false) else {
            // Bring a heading above the first row back into view.
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
            Some(current) => next_selectable_row(&self.rows, current, true),
            None => self.rows.iter().position(MenuRow::is_selectable),
        };
        let Some(next) = next else {
            return;
        };
        self.selected_index = Some(next);
        self.list_state.scroll_to(next);
        ctx.notify();
    }

    fn select_enter(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(selected) = self.selected_index else {
            return;
        };
        self.select_row(selected, ctx);
    }

    fn select_row(&mut self, row_index: usize, ctx: &mut ViewContext<Self>) {
        match self.rows.get(row_index).cloned() {
            Some(MenuRow::Target { index, .. }) => {
                let Some(target) = self.targets.get(index) else {
                    return;
                };
                let mode = target.base.mode(self.comparison);
                ctx.emit(CodeReviewDiffMenuEvent::Select(mode));
            }
            Some(MenuRow::TypedRevision) => self.pick_typed_revision(ctx),
            Some(MenuRow::Header(_) | MenuRow::Error(_) | MenuRow::Note(_)) | None => return,
        }
        ctx.notify();
    }

    fn pick_typed_revision(&mut self, ctx: &mut ViewContext<Self>) {
        if self.row_options.typed_revision == TypedRevisionState::Checking {
            return;
        }
        match parse_revision_input(&self.search_query) {
            Ok(rev) => {
                let rev = rev.to_string();
                self.row_options.typed_revision = TypedRevisionState::Checking;
                ctx.emit(CodeReviewDiffMenuEvent::ResolveRevision {
                    rev,
                    comparison: self.comparison,
                });
            }
            Err(err) => {
                self.row_options.typed_revision = TypedRevisionState::Failed(err.to_string());
            }
        }
        self.refresh_rows_keeping_selection();
    }

    fn emit_close(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(CodeReviewDiffMenuEvent::Close);
        ctx.notify();
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

        // Search bar inherits the menu card surface; a hairline bottom
        // border separates it from the row list below.
        Container::new(search_input)
            .with_horizontal_padding(SEARCH_INPUT_HORIZONTAL_PADDING)
            .with_vertical_padding(2.)
            .with_border(Border::bottom(1.0).with_border_color(theme.outline().into()))
            .finish()
    }

    fn render_comparison_button(
        &self,
        comparison: RevisionComparison,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let (label, tooltip_text, mouse_state) = match comparison {
            RevisionComparison::MergeBase => (
                "Since common ancestor",
                MERGE_BASE_TOOLTIP,
                &self.merge_base_button,
            ),
            RevisionComparison::Direct => ("Direct", DIRECT_TOOLTIP, &self.direct_button),
        };
        let chosen_styles = UiComponentStyles {
            background: Some(theme.surface_3().into()),
            ..Default::default()
        };
        let ui_builder = appearance.ui_builder().clone();
        let mut button = appearance
            .ui_builder()
            .button(ButtonVariant::Text, mouse_state.clone())
            .with_text_label(label.to_string())
            .with_style(UiComponentStyles {
                font_family_id: Some(appearance.ui_font_family()),
                font_size: Some(appearance.ui_font_size()),
                font_color: Some(theme.main_text_color(theme.surface_2()).into_solid()),
                padding: Some(Coords {
                    top: 2.,
                    bottom: 2.,
                    left: 8.,
                    right: 8.,
                }),
                border_radius: Some(CornerRadius::with_all(Radius::Pixels(
                    COMPARISON_BUTTON_CORNER_RADIUS,
                ))),
                ..Default::default()
            })
            .with_hovered_styles(chosen_styles)
            .with_active_styles(chosen_styles)
            .with_tooltip(move || {
                ui_builder
                    .tool_tip(tooltip_text.to_string())
                    .build()
                    .finish()
            });
        if self.comparison == comparison {
            button = button.active();
        }
        Container::new(
            button
                .build()
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeReviewDiffMenuAction::SetComparison(comparison));
                })
                .finish(),
        )
        .with_margin_left(4.)
        .finish()
    }

    /// The choice of how a picked branch or revision is compared.
    fn render_comparison_choice(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let caption = Text::new_inline(
            "Compare",
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(theme.sub_text_color(theme.surface_2()).into_solid())
        .finish();

        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(caption)
                .with_child(
                    self.render_comparison_button(RevisionComparison::MergeBase, appearance),
                )
                .with_child(self.render_comparison_button(RevisionComparison::Direct, appearance))
                .finish(),
        )
        .with_horizontal_padding(ROW_HORIZONTAL_PADDING)
        .with_vertical_padding(6.)
        .with_border(Border::bottom(1.0).with_border_color(theme.outline().into()))
        .finish()
    }

    /// What each row of the list shows, detached from the menu so that the list can own it.
    fn displayed_rows(&self) -> Vec<DisplayedRow> {
        let query = self.search_query.trim();
        self.rows
            .iter()
            .map(|row| match row {
                MenuRow::Header(title) => DisplayedRow::Header(*title),
                MenuRow::Target { index, highlighted } => match self.targets.get(*index) {
                    Some(target) => DisplayedRow::Target {
                        label: target.label.clone(),
                        detail: target.detail.clone(),
                        highlighted: highlighted.clone(),
                        is_checked: target.is_selected,
                    },
                    None => DisplayedRow::Note(String::new()),
                },
                MenuRow::TypedRevision => DisplayedRow::Target {
                    label: match self.row_options.typed_revision {
                        TypedRevisionState::Idle | TypedRevisionState::Failed(_) => {
                            format!("Compare with \u{2018}{query}\u{2019}")
                        }
                        TypedRevisionState::Checking => {
                            format!("Checking \u{2018}{query}\u{2019}\u{2026}")
                        }
                    },
                    detail: None,
                    highlighted: Vec::new(),
                    is_checked: false,
                },
                MenuRow::Error(message) => DisplayedRow::Error(message.clone()),
                MenuRow::Note(note) => DisplayedRow::Note((*note).to_string()),
            })
            .collect()
    }

    fn render_rows(&self, ctx: &AppContext) -> Box<dyn Element> {
        if self.rows.is_empty() {
            return Empty::new().finish();
        }

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

/// A row of the list as it is drawn.
enum DisplayedRow {
    Header(&'static str),
    Target {
        label: String,
        detail: Option<String>,
        highlighted: Vec<usize>,
        is_checked: bool,
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

        let (label, detail, highlighted, is_checked) = match self {
            DisplayedRow::Target {
                label,
                detail,
                highlighted,
                is_checked,
            } => (label, detail, highlighted, *is_checked),
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

        let (text_color, detail_color, background) = if is_focused {
            let background = theme.accent();
            let text_color = theme.main_text_color(background).into_solid();
            (text_color, text_color, Some(background))
        } else {
            (
                theme.main_text_color(theme.surface_2()).into_solid(),
                sub_text_color,
                None,
            )
        };

        let check: Box<dyn Element> = if is_checked {
            Icon::Check.to_warpui_icon(Fill::Solid(text_color)).finish()
        } else {
            Empty::new().finish()
        };
        let check_slot = ConstrainedBox::new(check)
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
                Container::new(check_slot)
                    .with_margin_right(CHECK_GAP)
                    .finish(),
            )
            .with_child(Expanded::new(1., label_text.finish()).finish());
        if let Some(detail) = detail {
            row.add_child(
                Container::new(
                    ConstrainedBox::new(
                        Text::new_inline(
                            detail.clone(),
                            font_family,
                            font_size * DETAIL_FONT_SIZE_RATIO,
                        )
                        .with_color(detail_color)
                        .with_clip(ClipConfig::ellipsis())
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

        EventHandler::new(container.finish())
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action(CodeReviewDiffMenuAction::ClickRow { index: row_index });
                DispatchEventResult::StopPropagation
            })
            .on_mouse_in(
                move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeReviewDiffMenuAction::HoverRow {
                        index: row_index,
                    });
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

impl Entity for CodeReviewDiffMenu {
    type Event = CodeReviewDiffMenuEvent;
}

impl TypedActionView for CodeReviewDiffMenu {
    type Action = CodeReviewDiffMenuAction;

    fn handle_action(&mut self, action: &CodeReviewDiffMenuAction, ctx: &mut ViewContext<Self>) {
        match action {
            CodeReviewDiffMenuAction::ClickRow { index } => {
                // Sync keyboard selection with the clicked row.
                if self.rows.get(*index).is_some_and(MenuRow::is_selectable) {
                    self.selected_index = Some(*index);
                    self.select_row(*index, ctx);
                }
            }
            CodeReviewDiffMenuAction::HoverRow { index } => {
                if self.rows.get(*index).is_some_and(MenuRow::is_selectable)
                    && self.selected_index != Some(*index)
                {
                    self.selected_index = Some(*index);
                    ctx.notify();
                }
            }
            CodeReviewDiffMenuAction::SelectUp => self.select_prev(ctx),
            CodeReviewDiffMenuAction::SelectDown => self.select_next(ctx),
            CodeReviewDiffMenuAction::SelectEnter => self.select_enter(ctx),
            CodeReviewDiffMenuAction::SetComparison(comparison) => {
                if self.comparison != *comparison {
                    self.comparison = *comparison;
                    self.row_options.typed_revision = TypedRevisionState::Idle;
                    self.refresh_rows_keeping_selection();
                    ctx.notify();
                }
                ctx.focus(&self.search_input);
            }
            CodeReviewDiffMenuAction::Close => self.emit_close(ctx),
        }
    }
}

impl View for CodeReviewDiffMenu {
    fn ui_name() -> &'static str {
        "CodeReviewDiffMenu"
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
        if self.row_options.offers_revisions {
            column.add_child(self.render_comparison_choice(appearance));
        }
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
                ctx.dispatch_typed_action(CodeReviewDiffMenuAction::Close);
            })
            .prevent_interaction_with_other_elements()
            .finish()
    }
}

#[cfg(test)]
#[path = "diff_menu_tests.rs"]
mod tests;
