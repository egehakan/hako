//! The parts of the source control overview that reach beyond a repository's uncommitted
//! changes: the control that switches its branch, and the graph of its history with the files
//! of the commit that is open in it.

use std::time::{SystemTime, UNIX_EPOCH};

use ::hako_remote::protocol::{
    GitFile, GitFileStatus as CommitFileStatus, GitLogCommit, GitRef, GitRefKind, GitSide,
    GitSwitchKind,
};
use warp_core::ui::color::blend::Blend;
use warp_core::ui::theme::AnsiColorIdentifier;
use warp_core::user_preferences::GetUserPreferences;
use warpui::elements::Border;

use super::*;
use crate::code::blame::relative_time;
use crate::code_review::graph_lanes::{DotStyle, GraphLanes, LANE_WIDTH, LaneColors};
use crate::code_review::nested_repo_history::{
    PageRequest, RepoHistory, load_branches, load_commit, load_page, one_line, switch_branch,
};

/// Where the open state of each repository's graph is kept between runs, by repository root.
const GRAPH_SECTIONS_KEY: &str = "SourceControlGraphSections";

/// The graph column is at most this many lanes wide; lanes further right are left out.
const MAX_GRAPH_LANES: usize = 6;
const GRAPH_COLUMN_GAP: f32 = 4.;
const GRAPH_HEADER_INDENT: f32 = 12.;

/// The next page of a history is asked for once a commit this close to its end is on screen.
const NEXT_PAGE_MARGIN: usize = 10;

const MAX_REF_LABELS: usize = 2;
const REF_LABEL_MAX_WIDTH: f32 = 130.;
const BRANCH_NAME_MAX_WIDTH: f32 = 200.;
const BRANCH_ICON_SIZE: f32 = 12.;
const SIDE_ICON_SIZE: f32 = 12.;
const COMMIT_NOTE_INDENT: f32 = 4.;
const LINE_COUNT_MARGIN: f32 = 6.;

const UNPUSHED_TOOLTIP: &str = "Not pushed yet";
const UNPULLED_TOOLTIP: &str = "Not pulled yet";

pub(super) fn read_graph_sections(app: &AppContext) -> HashMap<PathBuf, bool> {
    app.private_user_preferences()
        .read_value(GRAPH_SECTIONS_KEY)
        .unwrap_or_default()
        .and_then(|sections| serde_json::from_str(&sections).ok())
        .unwrap_or_default()
}

fn write_graph_sections(sections: &HashMap<PathBuf, bool>, app: &AppContext) {
    match serde_json::to_string(sections) {
        Ok(sections) => {
            if let Err(err) = app
                .private_user_preferences()
                .write_value(GRAPH_SECTIONS_KEY, sections)
            {
                log::warn!("Failed to remember which history graphs are open: {err:#}");
            }
        }
        Err(err) => log::warn!("Failed to write down which history graphs are open: {err:#}"),
    }
}

/// Whether a repository's graph is open when nobody has opened or closed it yet. A folder with
/// several repositories stays one compact list until a graph is asked for.
pub(super) fn is_graph_expanded_by_default(repo_count: usize) -> bool {
    repo_count == 1
}

/// The repositories whose history has more to read near the rows on screen.
pub(super) fn repos_wanting_more(
    rows: &[OverviewRow],
    visible: Range<usize>,
    commit_count: impl Fn(usize) -> usize,
) -> Vec<usize> {
    let mut repos = Vec::new();
    for row in rows.iter().take(visible.end).skip(visible.start) {
        let repo_index = match row {
            OverviewRow::Commit {
                repo_index,
                commit_index,
            }
            | OverviewRow::CommitNote {
                repo_index,
                commit_index,
                ..
            }
            | OverviewRow::CommitFile {
                repo_index,
                commit_index,
                ..
            } => (commit_index + NEXT_PAGE_MARGIN >= commit_count(*repo_index))
                .then_some(*repo_index),
            OverviewRow::GraphNote { repo_index } => Some(*repo_index),
            OverviewRow::Repo { .. }
            | OverviewRow::File { .. }
            | OverviewRow::GraphHeader { .. } => None,
        };
        if let Some(repo_index) = repo_index
            && !repos.contains(&repo_index)
        {
            repos.push(repo_index);
        }
    }
    repos
}

pub(super) fn commit_status_letter(status: CommitFileStatus) -> &'static str {
    match status {
        CommitFileStatus::Modified => "M",
        CommitFileStatus::Added => "A",
        CommitFileStatus::Deleted => "D",
        CommitFileStatus::Renamed => "R",
        CommitFileStatus::Untracked => "U",
        CommitFileStatus::Conflicted => "!",
    }
}

fn commit_status_color(status: CommitFileStatus, appearance: &Appearance) -> ColorU {
    match status {
        CommitFileStatus::Added | CommitFileStatus::Untracked => add_color(appearance),
        CommitFileStatus::Modified | CommitFileStatus::Renamed => replace_color(appearance),
        CommitFileStatus::Deleted | CommitFileStatus::Conflicted => remove_color(appearance),
    }
}

fn upstream_color(appearance: &Appearance) -> ColorU {
    AnsiColorIdentifier::Magenta
        .to_ansi_color(&appearance.theme().terminal_colors().normal)
        .into()
}

fn lane_colors(appearance: &Appearance) -> LaneColors {
    let theme = appearance.theme();
    let background = theme.background().into_solid();
    LaneColors {
        current: theme.accent().into_solid(),
        upstream: upstream_color(appearance),
        // Opaque, so that lines look the same where they overlap.
        other: background.blend(&blended_colors::text_disabled(theme, background)),
        background,
    }
}

/// A commit that the checked-out branch does not have yet is drawn as a ring.
fn dot_style(side: GitSide) -> DotStyle {
    match side {
        GitSide::Both | GitSide::Local => DotStyle::Filled,
        GitSide::Remote => DotStyle::Hollow,
    }
}

/// The author of a commit and how long ago it was written.
fn commit_byline(commit: &GitLogCommit, now: i64) -> String {
    let written = relative_time(commit.time / 1000, now);
    if commit.author.is_empty() {
        written
    } else {
        format!("{} \u{b7} {written}", commit.author)
    }
}

fn now_in_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

impl NestedReposView {
    pub(super) fn is_graph_expanded(&self, repo_root: &Path, repo_count: usize) -> bool {
        self.graph_expanded
            .get(repo_root)
            .copied()
            .unwrap_or_else(|| is_graph_expanded_by_default(repo_count))
    }

    pub(super) fn toggle_graph(&mut self, repo_root: &Path, ctx: &mut ViewContext<Self>) {
        let repo_count = self.model.as_ref(ctx).entries().len();
        let is_expanded = self.is_graph_expanded(repo_root, repo_count);
        self.graph_expanded
            .insert(repo_root.to_path_buf(), !is_expanded);
        write_graph_sections(&self.graph_expanded, ctx);
        self.sync_with_model(ctx);
    }

    /// Reads the repository's history again from its newest commit.
    pub(super) fn reload_history(&mut self, repo_root: &Path, ctx: &mut ViewContext<Self>) {
        let history = self.histories.entry(repo_root.to_path_buf()).or_default();
        if let Some(request) = history.begin_reload() {
            self.load_history_page(repo_root, request, ctx);
        }
    }

    fn load_more_history(&mut self, repo_root: &Path, ctx: &mut ViewContext<Self>) {
        let request = self
            .histories
            .get_mut(repo_root)
            .and_then(RepoHistory::begin_next_page);
        if let Some(request) = request {
            self.load_history_page(repo_root, request, ctx);
            ctx.notify();
        }
    }

    fn load_history_page(
        &mut self,
        repo_root: &Path,
        request: PageRequest,
        ctx: &mut ViewContext<Self>,
    ) {
        let repo = repo_root.to_path_buf();
        ctx.spawn(
            async move {
                let result = load_page(&repo, request).await;
                (repo, result)
            },
            move |me, (repo, result), ctx| {
                let Some(history) = me.histories.get_mut(&repo) else {
                    return;
                };
                if history.apply(request, result) {
                    me.reload_history(&repo, ctx);
                }
                me.sync_with_model(ctx);
            },
        );
    }

    /// Reads further into the histories whose end is near the rows on screen.
    pub(super) fn handle_visible_rows(
        &mut self,
        visible: Range<usize>,
        ctx: &mut ViewContext<Self>,
    ) {
        let model = self.model.clone();
        let entries = model.as_ref(ctx).entries();
        let wanting = repos_wanting_more(&self.rows, visible, |repo_index| {
            entries
                .get(repo_index)
                .and_then(|entry| self.histories.get(&entry.repo.root))
                .map_or(0, |history| history.commits().len())
        });
        let roots: Vec<PathBuf> = wanting
            .into_iter()
            .filter_map(|repo_index| Some(entries.get(repo_index)?.repo.root.clone()))
            .collect();
        for root in roots {
            self.load_more_history(&root, ctx);
        }
    }

    pub(super) fn toggle_commit(
        &mut self,
        repo_root: &Path,
        commit: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(history) = self.histories.get_mut(repo_root) else {
            return;
        };
        if history.toggle_commit(commit) {
            let (repo, id) = (repo_root.to_path_buf(), commit.to_owned());
            ctx.spawn(
                async move {
                    let result = load_commit(&repo, &id).await;
                    (repo, id, result)
                },
                |me, (repo, id, result), ctx| {
                    if let Some(history) = me.histories.get_mut(&repo) {
                        history.set_commit_files(&id, result);
                        me.sync_with_model(ctx);
                    }
                },
            );
        }
        self.sync_with_model(ctx);
    }

    pub(super) fn toggle_branch_menu(&mut self, repo_root: &Path, ctx: &mut ViewContext<Self>) {
        if self.branch_menu_repo.as_deref() == Some(repo_root) {
            self.close_branch_menu(ctx);
            return;
        }
        if matches!(
            self.activities.get(repo_root),
            Some(RepoActivity::Running(_))
        ) {
            return;
        }
        self.branch_menu_repo = Some(repo_root.to_path_buf());
        self.branch_menu_version = self
            .entry(repo_root, ctx)
            .map_or(0, |entry| entry.status_version);
        self.branch_menu.update(ctx, |menu, ctx| menu.reset(ctx));
        self.list_branches(repo_root, ctx);
        ctx.focus(&self.branch_menu);
        ctx.notify();
    }

    pub(super) fn close_branch_menu(&mut self, ctx: &mut ViewContext<Self>) {
        if self.branch_menu_repo.take().is_some() {
            ctx.focus_self();
            ctx.notify();
        }
    }

    /// Lists the branches of the repository the branch menu is open for.
    pub(super) fn list_branches(&mut self, repo_root: &Path, ctx: &mut ViewContext<Self>) {
        let repo = repo_root.to_path_buf();
        ctx.spawn(
            async move {
                let result = load_branches(&repo).await;
                (repo, result)
            },
            |me, (repo, result), ctx| {
                if me.branch_menu_repo.as_deref() == Some(repo.as_path()) {
                    me.branch_menu
                        .update(ctx, |menu, ctx| menu.set_branches(result, ctx));
                }
            },
        );
    }

    /// Checks `branch` out in the repository, as `git switch` does when nothing is forced.
    pub(super) fn switch_branch(
        &mut self,
        repo_root: &Path,
        branch: String,
        kind: GitSwitchKind,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.git_dialog.is_some()
            || self.discard_confirmation.is_some()
            || !self.start_activity(repo_root, "Switching\u{2026}", ctx)
        {
            return;
        }
        let path_env = Self::interactive_path(ctx);
        let repo = repo_root.to_path_buf();
        ctx.spawn(
            async move {
                let path_env = path_env.await;
                let result = switch_branch(&repo, &branch, kind, path_env).await;
                (repo, branch, result)
            },
            |me, (repo, branch, result), ctx| {
                let message = match result {
                    Ok(switched) => RowMessage::info(format!(
                        "Switched to {}",
                        switched.current.unwrap_or(branch)
                    )),
                    Err(message) => {
                        let message = one_line(&message);
                        log::warn!(
                            "Switching to {branch} failed in {}: {message}",
                            repo.display()
                        );
                        RowMessage::error(message)
                    }
                };
                me.finish_activity(&repo, message, ctx);
            },
        );
    }

    /// The repository's branch, which opens the list of branches to switch to.
    pub(super) fn render_branch_control(
        &self,
        repo_root: &Path,
        status: &NestedRepoStatus,
        mouse_state: &MouseStateHandle,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let is_open = self.branch_menu_repo.as_deref() == Some(repo_root);
        let label = if status.is_detached {
            format!("{} (detached)", status.branch)
        } else {
            status.branch.clone()
        };
        let font_family = appearance.ui_font_family();
        let font_size = appearance.ui_font_size() * SECONDARY_FONT_SIZE_RATIO;
        let dispatched = NestedReposViewAction::ToggleBranchMenu(repo_root.to_path_buf());

        let control = Hoverable::new(mouse_state.clone(), |mouse_state| {
            let is_emphasized = is_open || mouse_state.is_hovered();
            let color = if is_emphasized {
                blended_colors::text_main(theme, theme.background())
            } else {
                blended_colors::text_sub(theme, theme.background())
            };
            let icon = |icon: Icon| {
                ConstrainedBox::new(icon.to_warpui_icon(color.into()).finish())
                    .with_width(BRANCH_ICON_SIZE)
                    .with_height(BRANCH_ICON_SIZE)
                    .finish()
            };
            let content = Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(icon(Icon::GitBranch))
                .with_child(
                    Container::new(
                        ConstrainedBox::new(
                            Text::new_inline(label, font_family, font_size)
                                .with_color(color)
                                .with_clip(ClipConfig::end())
                                .finish(),
                        )
                        .with_max_width(BRANCH_NAME_MAX_WIDTH)
                        .finish(),
                    )
                    .with_margin_left(4.)
                    .with_margin_right(4.)
                    .finish(),
                )
                .with_child(icon(Icon::ChevronDown))
                .finish();
            let mut container = Container::new(content)
                .with_horizontal_padding(5.)
                .with_vertical_padding(1.)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
            if is_emphasized {
                container = container.with_background(internal_colors::fg_overlay_3(theme));
            }
            container.finish()
        })
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action(dispatched.clone()))
        .with_cursor(Cursor::PointingHand)
        .finish();

        let mut stack = Stack::new().with_child(control);
        if is_open {
            stack.add_positioned_overlay_child(
                ChildView::new(&self.branch_menu).finish(),
                OffsetPositioning::offset_from_parent(
                    vec2f(0., 4.),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::BottomLeft,
                    ChildAnchor::TopLeft,
                ),
            );
        }
        Container::new(stack.finish()).with_margin_left(6.).finish()
    }

    /// The heading of a repository's graph, which opens and closes it.
    pub(super) fn render_graph_header(
        &self,
        entry: &NestedRepoEntry,
        is_expanded: bool,
        mouse_state: &MouseStateHandle,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let chevron = if is_expanded {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        let is_loading = is_expanded
            && self
                .histories
                .get(&entry.repo.root)
                .is_some_and(RepoHistory::is_loading);
        let dispatched = NestedReposViewAction::ToggleGraph(entry.repo.root.clone());

        Hoverable::new(mouse_state.clone(), |mouse_state| {
            let highlight = ItemHighlightState::new(false, mouse_state);
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Container::new(
                        ConstrainedBox::new(
                            chevron
                                .to_warpui_icon(highlight.text_and_icon_color(appearance).into())
                                .finish(),
                        )
                        .with_width(CHEVRON_SIZE)
                        .with_height(CHEVRON_SIZE)
                        .finish(),
                    )
                    .with_margin_left(GRAPH_HEADER_INDENT)
                    .with_margin_right(4.)
                    .finish(),
                )
                .with_child(
                    Text::new_inline(
                        "Graph",
                        appearance.ui_font_family(),
                        appearance.ui_font_size() * SECONDARY_FONT_SIZE_RATIO,
                    )
                    .with_style(Properties::default().weight(Weight::Semibold))
                    .with_color(blended_colors::text_sub(theme, theme.background()))
                    .finish(),
                );
            if is_loading {
                row.add_child(
                    Container::new(
                        Text::new_inline(
                            "Reading\u{2026}",
                            appearance.ui_font_family(),
                            appearance.ui_font_size() * SECONDARY_FONT_SIZE_RATIO,
                        )
                        .with_color(blended_colors::text_disabled(theme, theme.background()))
                        .finish(),
                    )
                    .with_margin_left(8.)
                    .finish(),
                );
            }
            Self::finish_row(row.finish(), highlight, appearance)
        })
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action(dispatched.clone()))
        .with_cursor(Cursor::PointingHand)
        .finish()
    }

    /// The graph column of a row, as wide as the widest row of the repository's history needs.
    fn render_lanes(lanes: GraphLanes, lane_count: usize) -> Box<dyn Element> {
        let width = LANE_WIDTH * lane_count.clamp(1, MAX_GRAPH_LANES) as f32;
        Container::new(
            ConstrainedBox::new(lanes.finish())
                .with_width(width)
                .with_height(ROW_HEIGHT)
                .finish(),
        )
        .with_margin_left(GRAPH_HEADER_INDENT)
        .with_margin_right(GRAPH_COLUMN_GAP)
        .finish()
    }

    /// The lanes that pass a row under the commit at `commit_index`.
    fn render_passing_lanes(
        history: &RepoHistory,
        commit_index: usize,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let passing = history
            .rows()
            .get(commit_index)
            .map_or(&[][..], |row| row.after.as_slice());
        Self::render_lanes(
            GraphLanes::passing(passing, lane_colors(appearance), MAX_GRAPH_LANES),
            history.lane_count(),
        )
    }

    fn render_ref_label(reference: &GitRef, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text = |color: ColorU| {
            ConstrainedBox::new(
                Text::new_inline(
                    reference.name.clone(),
                    appearance.ui_font_family(),
                    appearance.ui_font_size() * SECONDARY_FONT_SIZE_RATIO,
                )
                .with_color(color)
                .with_clip(ClipConfig::end())
                .finish(),
            )
            .with_max_width(REF_LABEL_MAX_WIDTH)
            .finish()
        };
        let main_text_color = blended_colors::text_main(theme, theme.background());
        let label = match reference.kind {
            GitRefKind::Head => {
                let accent = theme.accent();
                Container::new(text(theme.main_text_color(accent).into_solid()))
                    .with_background(accent)
            }
            GitRefKind::Remote => {
                let mut tint = upstream_color(appearance);
                tint.a = 70;
                Container::new(text(main_text_color)).with_background_color(tint)
            }
            GitRefKind::Local => Container::new(text(main_text_color))
                .with_background(internal_colors::fg_overlay_3(theme)),
            GitRefKind::Tag => {
                Container::new(text(blended_colors::text_sub(theme, theme.background())))
                    .with_border(Border::all(1.).with_border_color(theme.outline().into()))
            }
        };
        label
            .with_horizontal_padding(5.)
            .with_vertical_padding(1.)
            .with_margin_left(6.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .finish()
    }

    /// The mark of a commit that only one of the branch and its upstream has, and what the mark
    /// says when it is pointed at.
    fn render_side_mark(
        side: GitSide,
        appearance: &Appearance,
    ) -> Option<(Box<dyn Element>, &'static str)> {
        let (icon, color, tooltip) = match side {
            GitSide::Both => return None,
            GitSide::Local => (
                Icon::ArrowUp,
                appearance.theme().accent().into_solid(),
                UNPUSHED_TOOLTIP,
            ),
            GitSide::Remote => (
                Icon::ArrowDown,
                upstream_color(appearance),
                UNPULLED_TOOLTIP,
            ),
        };
        let mark = Container::new(
            ConstrainedBox::new(icon.to_warpui_icon(color.into()).finish())
                .with_width(SIDE_ICON_SIZE)
                .with_height(SIDE_ICON_SIZE)
                .finish(),
        )
        .with_margin_left(6.)
        .finish();
        Some((mark, tooltip))
    }

    pub(super) fn render_commit_row(
        &self,
        entry: &NestedRepoEntry,
        commit_index: usize,
        appearance: &Appearance,
    ) -> Option<Box<dyn Element>> {
        let theme = appearance.theme();
        let history = self.histories.get(&entry.repo.root)?;
        let commit = history.commits().get(commit_index)?;
        let row = history.rows().get(commit_index)?;
        let mouse_state = self
            .mouse_states
            .get(&entry.repo.root)?
            .commits
            .get(commit_index)?;
        let is_open = history.open().is_some_and(|open| open.id == commit.id);
        let font_family = appearance.ui_font_family();
        let font_size = appearance.ui_font_size();
        let secondary_font_size = font_size * SECONDARY_FONT_SIZE_RATIO;
        let sub_text_color = blended_colors::text_sub(theme, theme.background());
        // What is still to be pulled is not in the working tree, and reads as further away.
        let subject_color = match commit.side {
            GitSide::Both | GitSide::Local => blended_colors::text_main(theme, theme.background()),
            GitSide::Remote => sub_text_color,
        };

        let lanes = Self::render_lanes(
            GraphLanes::commit(
                row,
                dot_style(commit.side),
                lane_colors(appearance),
                MAX_GRAPH_LANES,
            ),
            history.lane_count(),
        );
        let mut content = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(lanes)
            .with_child(
                Shrinkable::new(
                    3.,
                    Text::new_inline(commit.subject.clone(), font_family, font_size)
                        .with_color(subject_color)
                        .with_clip(ClipConfig::end())
                        .finish(),
                )
                .finish(),
            );
        for reference in commit.refs.iter().take(MAX_REF_LABELS) {
            content.add_child(Self::render_ref_label(reference, appearance));
        }
        if commit.refs.len() > MAX_REF_LABELS {
            content.add_child(
                Container::new(
                    Text::new_inline(
                        format!("+{}", commit.refs.len() - MAX_REF_LABELS),
                        font_family,
                        secondary_font_size,
                    )
                    .with_color(sub_text_color)
                    .finish(),
                )
                .with_margin_left(4.)
                .finish(),
            );
        }
        content.add_child(
            Shrinkable::new(
                1.,
                Container::new(
                    Text::new_inline(
                        commit_byline(commit, now_in_seconds()),
                        font_family,
                        secondary_font_size,
                    )
                    .with_color(sub_text_color)
                    .with_clip(ClipConfig::end())
                    .finish(),
                )
                .with_margin_left(8.)
                .finish(),
            )
            .finish(),
        );
        let side_mark = Self::render_side_mark(commit.side, appearance);
        let dispatched = NestedReposViewAction::ToggleCommit {
            repo_root: entry.repo.root.clone(),
            commit: commit.id.clone(),
        };

        Some(
            Hoverable::new(mouse_state.clone(), |mouse_state| {
                let highlight = ItemHighlightState::new(is_open, mouse_state);
                let mut row = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Shrinkable::new(1., Clipped::new(content.finish()).finish()).finish(),
                    );
                if let Some((side_mark, tooltip)) = side_mark {
                    let side_mark = if mouse_state.is_hovered() {
                        let mut stack = Stack::new().with_child(side_mark);
                        stack.add_positioned_overlay_child(
                            appearance
                                .ui_builder()
                                .tool_tip(tooltip.to_string())
                                .build()
                                .finish(),
                            OffsetPositioning::offset_from_parent(
                                vec2f(0., 4.),
                                ParentOffsetBounds::WindowByPosition,
                                ParentAnchor::BottomRight,
                                ChildAnchor::TopRight,
                            ),
                        );
                        stack.finish()
                    } else {
                        side_mark
                    };
                    row.add_child(side_mark);
                }
                Self::finish_row(row.finish(), highlight, appearance)
            })
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(dispatched.clone()))
            .with_cursor(Cursor::PointingHand)
            .finish(),
        )
    }

    /// A line of text under the open commit: who wrote it and when, and the rest of its
    /// message.
    pub(super) fn render_commit_note_row(
        &self,
        entry: &NestedRepoEntry,
        commit_index: usize,
        line: usize,
        appearance: &Appearance,
    ) -> Option<Box<dyn Element>> {
        let theme = appearance.theme();
        let history = self.histories.get(&entry.repo.root)?;
        let note = history.open()?.notes().into_iter().nth(line)?;
        let row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Self::render_passing_lanes(
                history,
                commit_index,
                appearance,
            ))
            .with_child(
                Shrinkable::new(
                    1.,
                    Container::new(
                        Text::new_inline(
                            note,
                            appearance.ui_font_family(),
                            appearance.ui_font_size() * SECONDARY_FONT_SIZE_RATIO,
                        )
                        .with_color(blended_colors::text_sub(theme, theme.background()))
                        .with_clip(ClipConfig::end())
                        .finish(),
                    )
                    .with_margin_left(COMMIT_NOTE_INDENT)
                    .finish(),
                )
                .finish(),
            )
            .finish();
        Some(Self::finish_row(
            Clipped::new(row).finish(),
            ItemHighlightState::None,
            appearance,
        ))
    }

    fn render_line_counts(file: &GitFile, appearance: &Appearance) -> Box<dyn Element> {
        let font_family = appearance.ui_font_family();
        let font_size = appearance.ui_font_size() * SECONDARY_FONT_SIZE_RATIO;
        let mut counts = Flex::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);
        let count = |text: String, color: ColorU| {
            Container::new(
                Text::new_inline(text, font_family, font_size)
                    .with_color(color)
                    .finish(),
            )
            .with_margin_left(LINE_COUNT_MARGIN)
            .finish()
        };
        if file.binary {
            let theme = appearance.theme();
            counts.add_child(count(
                "binary".to_owned(),
                blended_colors::text_sub(theme, theme.background()),
            ));
        }
        if let Some(additions) = file.additions {
            counts.add_child(count(format!("+{additions}"), add_color(appearance)));
        }
        if let Some(deletions) = file.deletions {
            counts.add_child(count(
                format!("\u{2212}{deletions}"),
                remove_color(appearance),
            ));
        }
        counts.finish()
    }

    /// A file that the open commit changed, which opens its diff in that commit.
    pub(super) fn render_commit_file_row(
        &self,
        entry: &NestedRepoEntry,
        commit_index: usize,
        file_index: usize,
        appearance: &Appearance,
    ) -> Option<Box<dyn Element>> {
        let theme = appearance.theme();
        let history = self.histories.get(&entry.repo.root)?;
        let open = history.open()?;
        let file = open.files().get(file_index)?;
        let mouse_state = self
            .mouse_states
            .get(&entry.repo.root)?
            .commit_files
            .get(file_index)?;
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
        let is_selected = self.selected_file.as_ref().is_some_and(|selected| {
            *selected
                == Selection::Commit {
                    repo_root: entry.repo.root.clone(),
                    commit: open.id.clone(),
                    path: file.path.clone(),
                }
        });
        let dispatched = NestedReposViewAction::SelectCommitFile {
            repo_root: entry.repo.root.clone(),
            commit: open.id.clone(),
            path: file.path.clone(),
        };

        let mut content = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Self::render_passing_lanes(
                history,
                commit_index,
                appearance,
            ))
            .with_child(
                Container::new(
                    ConstrainedBox::new(
                        Text::new_inline(commit_status_letter(file.status), font_family, font_size)
                            .with_style(Properties::default().weight(Weight::Semibold))
                            .with_color(commit_status_color(file.status, appearance))
                            .finish(),
                    )
                    .with_width(STATUS_LETTER_WIDTH)
                    .finish(),
                )
                .with_margin_left(COMMIT_NOTE_INDENT)
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
            content.add_child(
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
        let line_counts = Self::render_line_counts(file, appearance);

        Some(
            Hoverable::new(mouse_state.clone(), |mouse_state| {
                let highlight = ItemHighlightState::new(is_selected, mouse_state);
                let row = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Shrinkable::new(1., Clipped::new(content.finish()).finish()).finish(),
                    )
                    .with_child(line_counts)
                    .finish();
                Self::finish_row(row, highlight, appearance)
            })
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(dispatched.clone()))
            .with_cursor(Cursor::PointingHand)
            .finish(),
        )
    }

    /// The line after a repository's last commit: that more is being read, or why there is
    /// nothing to show.
    pub(super) fn render_graph_note_row(
        &self,
        entry: &NestedRepoEntry,
        appearance: &Appearance,
    ) -> Option<Box<dyn Element>> {
        let theme = appearance.theme();
        let history = self.histories.get(&entry.repo.root)?;
        let note = history.note()?;
        let lanes = match history.commits().len().checked_sub(1) {
            Some(last) if history.has_more() => {
                Self::render_passing_lanes(history, last, appearance)
            }
            Some(_) | None => Self::render_lanes(
                GraphLanes::passing(&[], lane_colors(appearance), MAX_GRAPH_LANES),
                history.lane_count(),
            ),
        };
        let row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(lanes)
            .with_child(
                Shrinkable::new(
                    1.,
                    Text::new_inline(
                        note,
                        appearance.ui_font_family(),
                        appearance.ui_font_size() * SECONDARY_FONT_SIZE_RATIO,
                    )
                    .with_color(blended_colors::text_sub(theme, theme.background()))
                    .with_clip(ClipConfig::end())
                    .finish(),
                )
                .finish(),
            )
            .finish();
        Some(Self::finish_row(
            Clipped::new(row).finish(),
            ItemHighlightState::None,
            appearance,
        ))
    }
}

#[cfg(test)]
#[path = "nested_repos_graph_tests.rs"]
mod tests;
