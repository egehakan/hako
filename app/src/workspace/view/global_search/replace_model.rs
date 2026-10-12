//! Runs a replace across the files of a project for global search.
//!
//! Files that are not open are rewritten in the background. Files open in an editor are edited
//! through their buffer on the main thread, so that the editor shows the change and whatever it
//! holds unsaved is kept.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use instant::Instant;
use project_replace::apply::{
    AppliedBufferEdits, DEFAULT_MAX_FILE_BYTES, FileTarget, OpenBuffer, Progress, ReplaceError,
    ReplaceOutcome, ReplaceReport, ReplaceRequest, replace_in_files,
};
use project_replace::engine::SearchSpec;
use project_replace::journal::{Journal, JournalLimits};
use project_replace::replacement::ReplaceSpec;
use project_replace::report::{SkipReason, SkippedFile};
use project_replace::scope::ScopeGlobs;
use warp_util::file::FileId;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

use crate::code::global_buffer_model::{BufferTextEditError, GlobalBufferModel};

/// Directory, below the app's state directory, that holds the copies kept for undoing.
const JOURNAL_DIRECTORY: &str = "project_replace_journal";

/// How often the progress of a run is reported while it goes through its files.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Set while a replace or an undo runs anywhere in the app. Two of them at once could each
/// change a file the other is about to check.
static RUN_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// What to replace, with what, and where.
pub struct ReplaceJob {
    pub search: SearchSpec,
    pub replace: ReplaceSpec,
    /// The local folders that were searched.
    pub roots: Vec<PathBuf>,
    pub scope: ScopeGlobs,
    pub files: Vec<FileTarget>,
}

pub enum ProjectReplaceEvent {
    /// A run started, got further, or was asked to stop.
    Progress,
    Finished(ReplaceReport),
    /// The run could not start or stopped before changing anything.
    Failed(String),
}

enum RunState {
    Idle,
    Replacing {
        progress: Option<Progress>,
        cancel: Arc<AtomicBool>,
    },
}

/// A file that was open in an editor when a run started.
struct OpenFile {
    file_id: FileId,
    has_unsaved_changes: bool,
}

pub struct ProjectReplace {
    state: RunState,
}

impl Entity for ProjectReplace {
    type Event = ProjectReplaceEvent;
}

impl ProjectReplace {
    pub fn new() -> Self {
        Self {
            state: RunState::Idle,
        }
    }

    pub fn is_running(&self) -> bool {
        match self.state {
            RunState::Idle => false,
            RunState::Replacing { .. } => true,
        }
    }

    /// How far the current run got, once it reported anything.
    pub fn progress(&self) -> Option<Progress> {
        match &self.state {
            RunState::Idle => None,
            RunState::Replacing { progress, .. } => *progress,
        }
    }

    /// Stops the current run before its next file. The files it already changed stay changed and
    /// can be undone.
    pub fn cancel(&mut self, ctx: &mut ModelContext<Self>) {
        match &self.state {
            RunState::Idle => {}
            RunState::Replacing { cancel, .. } => {
                cancel.store(true, Ordering::Relaxed);
                ctx.emit(ProjectReplaceEvent::Progress);
            }
        }
    }

    pub fn start(&mut self, job: ReplaceJob, ctx: &mut ModelContext<Self>) {
        let Some(journal) = journal() else {
            ctx.emit(ProjectReplaceEvent::Failed(
                "There is nowhere to keep the copies needed to undo a replace.".to_string(),
            ));
            return;
        };
        if RUN_IN_PROGRESS.swap(true, Ordering::SeqCst) {
            ctx.emit(ProjectReplaceEvent::Failed(
                "Another replace is still running.".to_string(),
            ));
            return;
        }

        let file_names: HashSet<&OsStr> = job
            .files
            .iter()
            .filter_map(|target| target.path.file_name())
            .collect();
        let (open_buffers, open_files) = open_buffers_named(&file_names, ctx);
        let request = ReplaceRequest {
            search: job.search,
            replace: job.replace,
            roots: job.roots,
            scope: job.scope,
            files: job.files,
            open_buffers,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let (progress_tx, progress_rx) = async_channel::unbounded();
        ctx.spawn_stream_local(progress_rx, Self::handle_progress, |_, _| {});
        ctx.spawn(
            {
                let cancel = cancel.clone();
                async move {
                    let last_report = Cell::new(None::<Instant>);
                    let report_progress = |progress: Progress| {
                        let is_due = last_report
                            .get()
                            .is_none_or(|last| last.elapsed() >= PROGRESS_INTERVAL);
                        if is_due || progress.done == progress.total {
                            last_report.set(Some(Instant::now()));
                            let _ = progress_tx.try_send(progress);
                        }
                    };
                    let result = replace_in_files(&request, &journal, &report_progress, &cancel);
                    (request, result)
                }
            },
            move |me, (request, result), ctx| {
                me.handle_files_replaced(request, result, open_files, ctx);
            },
        );

        self.state = RunState::Replacing {
            progress: None,
            cancel,
        };
        ctx.emit(ProjectReplaceEvent::Progress);
    }

    fn handle_progress(&mut self, latest: Progress, ctx: &mut ModelContext<Self>) {
        match &mut self.state {
            RunState::Idle => {}
            RunState::Replacing { progress, .. } => {
                *progress = Some(latest);
                ctx.emit(ProjectReplaceEvent::Progress);
            }
        }
    }

    /// Applies the edits of the files that are open in an editor, now that the files that are not
    /// have been rewritten, then closes the journal of the run.
    fn handle_files_replaced(
        &mut self,
        request: ReplaceRequest,
        result: Result<ReplaceOutcome, ReplaceError>,
        open_files: HashMap<PathBuf, OpenFile>,
        ctx: &mut ModelContext<Self>,
    ) {
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(err) => {
                self.finish_run();
                ctx.emit(ProjectReplaceEvent::Failed(err.to_string()));
                return;
            }
        };

        let mut applied = Vec::new();
        let mut skipped = Vec::new();
        for buffer_edits in &outcome.buffer_edits {
            let open_buffer = request
                .open_buffers
                .iter()
                .find(|buffer| buffer.path == buffer_edits.buffer_path);
            let (Some(open_buffer), Some(open_file)) =
                (open_buffer, open_files.get(&buffer_edits.buffer_path))
            else {
                continue;
            };

            let edits = buffer_edits
                .edits
                .iter()
                .map(|edit| (edit.range.clone(), edit.replacement.clone()))
                .collect();
            // A buffer that was in step with its file is saved, so that it stays in step. One
            // with unsaved changes keeps them unsaved, along with the replacement.
            let save = !open_file.has_unsaved_changes;
            let edited = GlobalBufferModel::handle(ctx).update(ctx, |model, ctx| {
                model.apply_text_edits(open_file.file_id, &open_buffer.text, edits, save, ctx)
            });
            match edited {
                Ok(text_after) => applied.push(AppliedBufferEdits {
                    path: buffer_edits.path.clone(),
                    text_before: open_buffer.text.clone(),
                    text_after,
                    occurrences: buffer_edits.edits.len(),
                }),
                Err(BufferTextEditError::NotLoaded | BufferTextEditError::TextChanged) => {
                    skipped.push(SkippedFile {
                        path: buffer_edits.path.clone(),
                        reason: SkipReason::EditedWhileReplacing,
                    });
                }
            }
        }

        ctx.spawn(
            async move { outcome.finish(applied, skipped) },
            |me, report, ctx| {
                me.finish_run();
                ctx.emit(ProjectReplaceEvent::Finished(report));
            },
        );
    }

    fn finish_run(&mut self) {
        self.state = RunState::Idle;
        RUN_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

impl Default for ProjectReplace {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ProjectReplace {
    /// A run whose model is gone never gets to report back, so it is told to stop and no longer
    /// holds up other runs.
    fn drop(&mut self) {
        match &self.state {
            RunState::Idle => {}
            RunState::Replacing { cancel, .. } => {
                cancel.store(true, Ordering::Relaxed);
                RUN_IN_PROGRESS.store(false, Ordering::SeqCst);
            }
        }
    }
}

fn journal() -> Option<Journal> {
    let state_directory = warp_core::paths::state_dir();
    // Without a state directory the journal would land wherever the app happens to run from.
    if state_directory.as_os_str().is_empty() {
        return None;
    }
    Some(Journal::new(
        state_directory.join(JOURNAL_DIRECTORY),
        JournalLimits::default(),
    ))
}

/// The text of every local file open in an editor whose name is one of `file_names`, along with
/// what is needed to edit it. Going by name keeps the text of unrelated editors from being
/// copied, while still finding a file that is open under another path to it.
fn open_buffers_named(
    file_names: &HashSet<&OsStr>,
    ctx: &AppContext,
) -> (Vec<OpenBuffer>, HashMap<PathBuf, OpenFile>) {
    let mut open_buffers = Vec::new();
    let mut open_files = HashMap::new();
    for loaded in GlobalBufferModel::as_ref(ctx).loaded_local_buffers(ctx) {
        let is_named = loaded
            .path
            .file_name()
            .is_some_and(|name| file_names.contains(name));
        if !is_named {
            continue;
        }
        open_buffers.push(OpenBuffer {
            path: loaded.path.clone(),
            text: loaded.buffer.as_ref(ctx).text().into_string(),
            has_unsaved_changes: loaded.has_unsaved_changes,
        });
        open_files.insert(
            loaded.path,
            OpenFile {
                file_id: loaded.file_id,
                has_unsaved_changes: loaded.has_unsaved_changes,
            },
        );
    }
    (open_buffers, open_files)
}
