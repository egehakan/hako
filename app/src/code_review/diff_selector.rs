//! Trigger button + [`CodeReviewDiffMenu`] overlay for picking the diff
//! target in the code review header.
use itertools::Itertools as _;
use pathfinder_geometry::vector::vec2f;
use warp_core::ui::theme::Fill;
use warpui::elements::{
    ChildAnchor, ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Element,
    Flex, MouseStateHandle, OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds,
    Radius, Stack, Text,
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
use crate::code_review::diff_menu::{CodeReviewDiffMenu, CodeReviewDiffMenuEvent};
use crate::code_review::diff_state::{DiffMode, RevisionComparison, display_revision};
use crate::code_review::revisions::{RevisionCandidates, RevisionEntry, RevisionKind};
use crate::ui_components::blended_colors;
use crate::ui_components::buttons::icon_button_with_color;
use crate::ui_components::icons::Icon;
use crate::util::git::BranchEntry;

/// What a row of the diff selector compares the working directory with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffTargetBase {
    UncommittedChanges,
    /// The repository's main branch, by name.
    MainBranch(String),
    /// A local branch other than the main one.
    Branch(String),
    /// A remote branch, a tag, a commit, or text that git resolves to one.
    Revision(String),
}

impl DiffTargetBase {
    /// The diff mode that picking this base asks for.
    pub fn mode(&self, comparison: RevisionComparison) -> DiffMode {
        match (self, comparison) {
            (DiffTargetBase::UncommittedChanges, _) => DiffMode::Head,
            (DiffTargetBase::MainBranch(_), RevisionComparison::MergeBase) => DiffMode::MainBranch,
            (DiffTargetBase::Branch(name), RevisionComparison::MergeBase) => {
                DiffMode::OtherBranch(name.clone())
            }
            (
                DiffTargetBase::MainBranch(rev) | DiffTargetBase::Branch(rev),
                RevisionComparison::Direct,
            )
            | (DiffTargetBase::Revision(rev), _) => DiffMode::Revision {
                rev: rev.clone(),
                comparison,
            },
        }
    }

    /// Whether `mode` compares with this base, whichever way it does.
    pub fn is_base_of(&self, mode: &DiffMode) -> bool {
        match mode {
            DiffMode::Head => matches!(self, DiffTargetBase::UncommittedChanges),
            DiffMode::MainBranch => matches!(self, DiffTargetBase::MainBranch(_)),
            DiffMode::OtherBranch(rev) | DiffMode::Revision { rev, .. } => match self {
                DiffTargetBase::UncommittedChanges => false,
                DiffTargetBase::MainBranch(name)
                | DiffTargetBase::Branch(name)
                | DiffTargetBase::Revision(name) => name == rev,
            },
        }
    }
}

/// Group a row of the diff selector is listed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTargetSection {
    /// The uncommitted changes, and a selected base that no other group lists.
    WorkingTree,
    Branches,
    RemoteBranches,
    Tags,
    Commits,
}

impl DiffTargetSection {
    /// Heading shown above the group's rows.
    pub fn title(&self) -> Option<&'static str> {
        match self {
            DiffTargetSection::WorkingTree => None,
            DiffTargetSection::Branches => Some("Branches"),
            DiffTargetSection::RemoteBranches => Some("Remote branches"),
            DiffTargetSection::Tags => Some("Tags"),
            DiffTargetSection::Commits => Some("Recent commits"),
        }
    }
}

/// A single selectable target in the diff selector menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffTarget {
    pub label: String,
    /// Secondary text, such as a commit's id, author and age.
    pub detail: Option<String>,
    pub section: DiffTargetSection,
    pub base: DiffTargetBase,
    pub is_selected: bool,
}

/// What the targets of a repository are built from.
pub struct DiffTargetSources<'a> {
    pub current_mode: &'a DiffMode,
    pub current_branch: Option<&'a str>,
    pub branches: &'a [BranchEntry],
    /// `None` until the revisions have been listed, and for repositories they are not listed for.
    pub revisions: Option<&'a RevisionCandidates>,
}

fn revision_detail(entry: &RevisionEntry) -> String {
    [&entry.short_sha, &entry.author, &entry.relative_date]
        .into_iter()
        .filter(|part| !part.is_empty())
        .join(" \u{b7} ")
}

/// Builds the rows of the diff selector: the uncommitted changes first, then local branches with
/// the main one leading, remote branches, tags and recent commits. A selected base that none of
/// those lists follows the uncommitted changes, so the selection is always visible.
pub fn build_diff_targets(sources: DiffTargetSources<'_>) -> Vec<DiffTarget> {
    let DiffTargetSources {
        current_mode,
        current_branch,
        branches,
        revisions,
    } = sources;
    let target = |label: &str, detail: Option<String>, section, base: DiffTargetBase| DiffTarget {
        label: label.to_string(),
        detail,
        section,
        is_selected: base.is_base_of(current_mode),
        base,
    };

    let mut listed = Vec::new();
    if let Some(main) = branches.iter().find(|entry| entry.is_main) {
        listed.push(target(
            &main.name,
            None,
            DiffTargetSection::Branches,
            DiffTargetBase::MainBranch(main.name.clone()),
        ));
    }
    // The checked-out branch is left out: comparing with it is the uncommitted changes.
    for entry in branches {
        if entry.is_main || current_branch == Some(entry.name.as_str()) {
            continue;
        }
        listed.push(target(
            &entry.name,
            None,
            DiffTargetSection::Branches,
            DiffTargetBase::Branch(entry.name.clone()),
        ));
    }
    if let Some(revisions) = revisions {
        let groups = [
            (
                &revisions.remote_branches,
                DiffTargetSection::RemoteBranches,
            ),
            (&revisions.tags, DiffTargetSection::Tags),
            (&revisions.commits, DiffTargetSection::Commits),
        ];
        for (entries, section) in groups {
            for entry in entries {
                let label = match entry.kind {
                    RevisionKind::RemoteBranch | RevisionKind::Tag => entry.rev.as_str(),
                    RevisionKind::Commit if entry.subject.is_empty() => "(no commit message)",
                    RevisionKind::Commit => entry.subject.as_str(),
                };
                listed.push(target(
                    label,
                    Some(revision_detail(entry)),
                    section,
                    DiffTargetBase::Revision(entry.rev.clone()),
                ));
            }
        }
    }

    let mut targets = vec![target(
        "Uncommitted changes",
        None,
        DiffTargetSection::WorkingTree,
        DiffTargetBase::UncommittedChanges,
    )];
    if !listed.iter().any(|target| target.is_selected) {
        let unlisted = match current_mode {
            DiffMode::Head | DiffMode::MainBranch => None,
            DiffMode::OtherBranch(branch) => Some(DiffTargetBase::Branch(branch.clone())),
            DiffMode::Revision { rev, .. } => Some(DiffTargetBase::Revision(rev.clone())),
        };
        if let Some(base) = unlisted {
            let label = match &base {
                DiffTargetBase::UncommittedChanges => "",
                DiffTargetBase::MainBranch(name)
                | DiffTargetBase::Branch(name)
                | DiffTargetBase::Revision(name) => display_revision(name),
            };
            targets.push(target(
                label,
                None,
                DiffTargetSection::WorkingTree,
                base.clone(),
            ));
        }
    }
    targets.extend(listed);
    targets
}

/// Everything the diff selector shows for a repository.
pub struct DiffSelectorContent {
    pub targets: Vec<DiffTarget>,
    /// The current comparison base, as shown on the trigger button.
    pub base_label: String,
    /// Whether the working directory is compared with anything but its uncommitted changes.
    pub has_custom_base: bool,
    pub comparison: RevisionComparison,
    /// Whether the repository can be compared with revisions other than its local branches.
    pub offers_revisions: bool,
    /// Whether the revisions are still being listed.
    pub is_loading_revisions: bool,
}

const BUTTON_LABEL_MAX_WIDTH: f32 = 320.;
const RESET_TOOLTIP: &str = "Back to uncommitted changes";
const MENU_OFFSET_Y: f32 = 4.;
const BUTTON_CORNER_RADIUS: f32 = 4.;
const BUTTON_VERTICAL_PADDING: f32 = 5.;
const BUTTON_HORIZONTAL_PADDING: f32 = 8.;

pub struct DiffSelector {
    menu: ViewHandle<CodeReviewDiffMenu>,
    menu_open: bool,
    trigger_mouse_state: MouseStateHandle,
    reset_mouse_state: MouseStateHandle,
    /// Cached label for the trigger button: the current comparison base.
    trigger_label: String,
    has_custom_base: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DiffSelectorAction {
    Toggle,
    /// Go back to comparing with the uncommitted changes.
    Reset,
}

#[derive(Clone, Debug)]
pub enum DiffSelectorEvent {
    SelectMode(DiffMode),
    /// The menu was opened, which is when its lazily listed rows are needed.
    Opened,
    /// Typed text was picked as the comparison base and has to be checked first.
    ResolveRevision {
        rev: String,
        comparison: RevisionComparison,
    },
}

pub fn init(app: &mut AppContext) {
    app.register_fixed_bindings([FixedBinding::new(
        "enter",
        DiffSelectorAction::Toggle,
        id!(DiffSelector::ui_name()),
    )]);
}

impl DiffSelector {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let menu = ctx.add_typed_action_view(CodeReviewDiffMenu::new);

        ctx.subscribe_to_view(&menu, |me, _, event, ctx| match event {
            CodeReviewDiffMenuEvent::Select(mode) => {
                me.menu_open = false;
                ctx.emit(DiffSelectorEvent::SelectMode(mode.clone()));
                ctx.notify();
            }
            CodeReviewDiffMenuEvent::ResolveRevision { rev, comparison } => {
                ctx.emit(DiffSelectorEvent::ResolveRevision {
                    rev: rev.clone(),
                    comparison: *comparison,
                });
            }
            CodeReviewDiffMenuEvent::Close => {
                me.menu_open = false;
                ctx.notify();
            }
        });

        Self {
            menu,
            menu_open: false,
            trigger_mouse_state: MouseStateHandle::default(),
            reset_mouse_state: MouseStateHandle::default(),
            trigger_label: String::new(),
            has_custom_base: false,
        }
    }

    pub fn toggle(&mut self, ctx: &mut ViewContext<Self>) {
        if self.menu_open {
            self.close(ctx);
        } else {
            self.menu_open = true;
            self.menu.update(ctx, |menu, ctx| menu.reset(ctx));
            ctx.focus(&self.menu);
            ctx.emit(DiffSelectorEvent::Opened);
            ctx.notify();
        }
    }

    /// Reports that the typed revision `rev` cannot be compared with. The menu stays open.
    pub fn show_revision_error(&mut self, rev: &str, message: String, ctx: &mut ViewContext<Self>) {
        self.menu.update(ctx, |menu, ctx| {
            menu.show_revision_error(rev, message, ctx);
        });
    }

    pub fn close(&mut self, ctx: &mut ViewContext<Self>) {
        if self.menu_open {
            self.menu_open = false;
            ctx.notify();
        }
    }

    /// Replace the menu rows and the comparison base shown on the trigger button.
    pub fn set_content(&mut self, content: DiffSelectorContent, ctx: &mut ViewContext<Self>) {
        let DiffSelectorContent {
            targets,
            base_label,
            has_custom_base,
            comparison,
            offers_revisions,
            is_loading_revisions,
        } = content;
        self.trigger_label = base_label;
        self.has_custom_base = has_custom_base;
        self.menu.update(ctx, |menu, ctx| {
            menu.set_content(
                targets,
                comparison,
                offers_revisions,
                is_loading_revisions,
                ctx,
            );
        });
        ctx.notify();
    }

    fn render_reset_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let tooltip = appearance
            .ui_builder()
            .tool_tip(RESET_TOOLTIP.to_string())
            .build()
            .finish();
        icon_button_with_color(
            appearance,
            Icon::X,
            false,
            self.reset_mouse_state.clone(),
            blended_colors::text_sub(theme, theme.background()).into(),
        )
        .with_tooltip(move || tooltip)
        .build()
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(DiffSelectorAction::Reset))
        .with_cursor(Cursor::PointingHand)
        .finish()
    }
}

impl Entity for DiffSelector {
    type Event = DiffSelectorEvent;
}

impl TypedActionView for DiffSelector {
    type Action = DiffSelectorAction;

    fn handle_action(&mut self, action: &DiffSelectorAction, ctx: &mut ViewContext<Self>) {
        match action {
            DiffSelectorAction::Toggle => self.toggle(ctx),
            DiffSelectorAction::Reset => {
                self.close(ctx);
                ctx.emit(DiffSelectorEvent::SelectMode(DiffMode::Head));
            }
        }
    }
}

impl View for DiffSelector {
    fn ui_name() -> &'static str {
        "CodeReviewDiffSelector"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() && self.menu_open {
            ctx.focus(&self.menu);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let text_color = theme.main_text_color(theme.background()).into_solid();
        let font_family = appearance.ui_font_family();
        let font_size = appearance.ui_font_size();

        let label = if self.trigger_label.is_empty() {
            "Uncommitted changes".to_string()
        } else {
            self.trigger_label.clone()
        };

        // Build the icon+text row by hand: `with_text_and_icon_label` wraps
        // the text in `Shrinkable<flex=1>`, which makes the button grow to
        // fill available width and paints an oversized hover rectangle even
        // for short labels.
        let icon = ConstrainedBox::new(
            Icon::SwitchHorizontal01
                .to_warpui_icon(Fill::Solid(text_color))
                .finish(),
        )
        .with_width(15.)
        .with_height(15.)
        .finish();

        // Truncate long branch names. Capping the text directly avoids
        // double-constraining padding from an outer wrapper.
        let label_text = ConstrainedBox::new(
            Text::new_inline(label, font_family, font_size)
                .with_color(text_color)
                .with_style(Properties::default().weight(Weight::Semibold))
                .with_clip(ClipConfig::ellipsis())
                .finish(),
        )
        .with_max_width(BUTTON_LABEL_MAX_WIDTH)
        .finish();

        let custom_label = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(icon)
            .with_child(Container::new(label_text).with_margin_left(6.).finish())
            .finish();

        // Shared hover + active styles so the fill persists when the pointer
        // moves into the menu's search input.
        let hover_or_active_styles = UiComponentStyles {
            background: Some(theme.surface_2().into()),
            border_radius: Some(CornerRadius::with_all(Radius::Pixels(BUTTON_CORNER_RADIUS))),
            ..Default::default()
        };

        let mut button = appearance
            .ui_builder()
            .button(ButtonVariant::Text, self.trigger_mouse_state.clone())
            .with_custom_label(custom_label)
            .with_style(UiComponentStyles {
                font_family_id: Some(font_family),
                font_size: Some(font_size),
                font_color: Some(text_color),
                font_weight: Some(Weight::Semibold),
                padding: Some(Coords {
                    top: BUTTON_VERTICAL_PADDING,
                    bottom: BUTTON_VERTICAL_PADDING,
                    left: BUTTON_HORIZONTAL_PADDING,
                    right: BUTTON_HORIZONTAL_PADDING,
                }),
                border_radius: Some(CornerRadius::with_all(Radius::Pixels(BUTTON_CORNER_RADIUS))),
                ..Default::default()
            })
            .with_hovered_styles(hover_or_active_styles)
            .with_active_styles(hover_or_active_styles);

        if self.menu_open {
            button = button.active();
        }

        let trigger = button
            .build()
            .with_cursor(Cursor::PointingHand)
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(DiffSelectorAction::Toggle);
            })
            .finish();

        let mut stack = Stack::new().with_child(trigger);

        if self.menu_open {
            stack.add_positioned_overlay_child(
                ChildView::new(&self.menu).finish(),
                OffsetPositioning::offset_from_parent(
                    vec2f(0., MENU_OFFSET_Y),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::BottomLeft,
                    ChildAnchor::TopLeft,
                ),
            );
        }

        if !self.has_custom_base {
            return Container::new(stack.finish()).finish();
        }
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(stack.finish())
            .with_child(self.render_reset_button(appearance))
            .finish()
    }
}

#[cfg(test)]
#[path = "diff_selector_tests.rs"]
mod tests;
