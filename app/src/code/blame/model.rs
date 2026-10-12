//! Computes and caches the blame of files open in code editors.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::{Either, select};
use parking_lot::Mutex;
use warp_util::content_version::ContentVersion;
use warp_util::git::{run_git_command, run_git_command_streaming};
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

use super::format::GitIdentity;
use super::porcelain::{BlameParser, FileBlame};

/// Larger buffers are not blamed: git would take long enough to be felt while typing.
const MAX_BLAME_BYTES: usize = 2 * 1024 * 1024;
const MAX_BLAME_LINES: usize = 50_000;
/// How long git gets for one file. What it attributed until then is kept.
const BLAME_TIMEOUT: Duration = Duration::from_secs(8);
const CACHED_FILES: usize = 32;
/// Bounds for the small lookup tables, which are emptied when they reach it.
const MAX_REMEMBERED_PATHS: usize = 512;
const MAX_REMEMBERED_MESSAGES: usize = 256;

/// A file in the work tree of a git repository.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlameFile {
    pub repo_root: PathBuf,
    /// Relative to `repo_root` and with forward slashes, the way git names the file.
    pub relative_path: String,
}

/// Everything the blame of a file depends on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlameKey {
    pub file: BlameFile,
    /// The commit `HEAD` pointed to.
    pub head: String,
    /// The version of the buffer whose text was blamed.
    pub version: ContentVersion,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BlameResult {
    /// Lines git had not reached when it ran out of time have no commit.
    Blamed(FileBlame),
    /// Git has no history for the file: it is untracked or ignored.
    NotTracked,
    TooLarge,
    /// Git did not attribute a single line in the time it was given.
    TimedOut,
    Failed,
}

/// The blames last used, at most one per file.
pub(super) struct BlameCache {
    /// Most recently used first.
    entries: VecDeque<(BlameKey, Arc<BlameResult>)>,
    capacity: usize,
}

impl BlameCache {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
        }
    }

    pub(super) fn get(&mut self, key: &BlameKey) -> Option<Arc<BlameResult>> {
        let index = self.entries.iter().position(|(cached, _)| cached == key)?;
        let entry = self.entries.remove(index)?;
        let result = entry.1.clone();
        self.entries.push_front(entry);
        Some(result)
    }

    /// Stores `result`, replacing what was cached for another version of the same file, and
    /// drops the file used longest ago when there are too many.
    pub(super) fn insert(&mut self, key: BlameKey, result: Arc<BlameResult>) {
        self.entries.retain(|(cached, _)| cached.file != key.file);
        self.entries.push_front((key, result));
        self.entries.truncate(self.capacity);
    }

    /// Drops what was computed for `repo_root` against a commit other than `head`.
    pub(super) fn retain_head(&mut self, repo_root: &Path, head: Option<&str>) {
        self.entries.retain(|(cached, _)| {
            cached.file.repo_root != repo_root || Some(cached.head.as_str()) == head
        });
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Something that stops a computation under way.
pub(super) trait AbortJob {
    fn abort(&self);
}

impl AbortJob for SpawnedFutureHandle {
    fn abort(&self) {
        SpawnedFutureHandle::abort(self);
    }
}

/// The blames being computed, at most one per file.
pub(super) struct BlameJobs<H> {
    running: HashMap<BlameFile, (BlameKey, H)>,
}

impl<H: AbortJob> BlameJobs<H> {
    pub(super) fn new() -> Self {
        Self {
            running: HashMap::new(),
        }
    }

    /// Whether a computation for `key` still has to be started. One that is running for another
    /// version of the file, or against another `HEAD`, is stopped: its result would describe
    /// text that is no longer in the buffer.
    pub(super) fn needs_start(&mut self, key: &BlameKey) -> bool {
        match self.running.get(&key.file) {
            Some((running, _)) if running == key => false,
            Some(_) => {
                if let Some((_, stale)) = self.running.remove(&key.file) {
                    stale.abort();
                }
                true
            }
            None => true,
        }
    }

    pub(super) fn started(&mut self, key: BlameKey, handle: H) {
        self.running.insert(key.file.clone(), (key, handle));
    }

    /// Forgets the computation for `key`, which has ended. Returns `false` when it had been
    /// replaced in the meantime, in which case its result must not be used.
    pub(super) fn finished(&mut self, key: &BlameKey) -> bool {
        let is_current = self
            .running
            .get(&key.file)
            .is_some_and(|(running, _)| running == key);
        if is_current {
            self.running.remove(&key.file);
        }
        is_current
    }

    /// Stops everything running for `repo_root`.
    pub(super) fn abort_repo(&mut self, repo_root: &Path) {
        self.running.retain(|file, (_, handle)| {
            let stale = file.repo_root == repo_root;
            if stale {
                handle.abort();
            }
            !stale
        });
    }
}

/// What a repository contributes to how blame is shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoInfo {
    /// `None` while the repository has no commits.
    pub head: Option<String>,
    pub identity: GitIdentity,
    /// The address of the `origin` remote.
    pub origin_url: Option<String>,
}

#[derive(Default)]
struct RepoState {
    /// `None` until it has been read for the first time.
    info: Option<RepoInfo>,
    is_refreshing: bool,
    /// A refresh was asked for while one was running, whose answer may predate the change.
    refresh_again: bool,
}

enum Resolution<T> {
    Pending,
    Known(T),
}

pub enum BlameEvent {
    /// A lookup may now answer differently: a computation ended or a repository changed.
    Updated,
}

pub enum BlameLookup {
    /// The answer for the buffer as it is now.
    Ready(Arc<BlameResult>),
    /// Being worked out. [`BlameEvent::Updated`] follows.
    Pending,
    /// The file is not in the work tree of a repository that has commits.
    NoRepository,
}

pub struct BlameModel {
    cache: BlameCache,
    jobs: BlameJobs<SpawnedFutureHandle>,
    /// Where each path that was asked about lives; `None` for a path outside any work tree.
    files: HashMap<PathBuf, Resolution<Option<BlameFile>>>,
    repos: HashMap<PathBuf, RepoState>,
    /// Full commit messages by repository and commit; `None` when git could not print one.
    messages: HashMap<(PathBuf, String), Resolution<Option<Arc<str>>>>,
}

impl Entity for BlameModel {
    type Event = BlameEvent;
}

impl SingletonEntity for BlameModel {}

impl Default for BlameModel {
    fn default() -> Self {
        Self::new()
    }
}

impl BlameModel {
    pub fn new() -> Self {
        Self {
            cache: BlameCache::new(CACHED_FILES),
            jobs: BlameJobs::new(),
            files: HashMap::new(),
            repos: HashMap::new(),
            messages: HashMap::new(),
        }
    }

    /// The blame of the file at `path` whose buffer is at `version`. `contents` is only called
    /// when the text has to be handed to git. A lookup for an answer that is already known does
    /// no work, which is what makes asking on every cursor movement free.
    pub fn lookup(
        &mut self,
        path: &Path,
        version: ContentVersion,
        contents: impl FnOnce(&AppContext) -> String,
        ctx: &mut ModelContext<Self>,
    ) -> BlameLookup {
        let file = match self.files.get(path) {
            Some(Resolution::Known(Some(file))) => file.clone(),
            Some(Resolution::Known(None)) => return BlameLookup::NoRepository,
            Some(Resolution::Pending) => return BlameLookup::Pending,
            None => {
                self.resolve_file(path.to_path_buf(), ctx);
                return BlameLookup::Pending;
            }
        };
        let head = match self.repos.get(&file.repo_root).map(|repo| &repo.info) {
            Some(Some(info)) => match &info.head {
                Some(head) => head.clone(),
                None => return BlameLookup::NoRepository,
            },
            Some(None) => return BlameLookup::Pending,
            None => {
                self.refresh_repo(&file.repo_root, ctx);
                return BlameLookup::Pending;
            }
        };

        let key = BlameKey {
            file,
            head,
            version,
        };
        if let Some(result) = self.cache.get(&key) {
            return BlameLookup::Ready(result);
        }
        if !self.jobs.needs_start(&key) {
            return BlameLookup::Pending;
        }

        let contents = contents(ctx);
        if exceeds_blame_limits(&contents) {
            let result = Arc::new(BlameResult::TooLarge);
            self.cache.insert(key, result.clone());
            return BlameLookup::Ready(result);
        }

        let job_key = key.clone();
        let handle = ctx.spawn(
            compute_blame(key.file.clone(), contents, BLAME_TIMEOUT),
            move |me, result, ctx| {
                if me.jobs.finished(&job_key) {
                    me.cache.insert(job_key, Arc::new(result));
                    ctx.emit(BlameEvent::Updated);
                }
            },
        );
        self.jobs.started(key, handle);
        BlameLookup::Pending
    }

    /// The repository file `path` was found to be, once a lookup has resolved it.
    pub fn file_for_path(&self, path: &Path) -> Option<&BlameFile> {
        match self.files.get(path)? {
            Resolution::Known(file) => file.as_ref(),
            Resolution::Pending => None,
        }
    }

    pub fn repo_info(&self, repo_root: &Path) -> Option<&RepoInfo> {
        self.repos.get(repo_root)?.info.as_ref()
    }

    /// Reads `HEAD` and the configuration of `repo_root` again. Blames computed against another
    /// commit are dropped when `HEAD` turns out to have moved.
    pub fn refresh_repo(&mut self, repo_root: &Path, ctx: &mut ModelContext<Self>) {
        let state = self.repos.entry(repo_root.to_path_buf()).or_default();
        if state.is_refreshing {
            state.refresh_again = true;
            return;
        }
        state.is_refreshing = true;

        let root = repo_root.to_path_buf();
        ctx.spawn(load_repo_info(root.clone()), move |me, info, ctx| {
            me.repo_info_loaded(root, info, ctx);
        });
    }

    fn repo_info_loaded(
        &mut self,
        repo_root: PathBuf,
        info: RepoInfo,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(state) = self.repos.get_mut(&repo_root) else {
            return;
        };
        state.is_refreshing = false;
        let refresh_again = std::mem::take(&mut state.refresh_again);
        let changed = state.info.as_ref() != Some(&info);
        let head_changed = state.info.as_ref().map(|current| &current.head) != Some(&info.head);
        let head = info.head.clone();
        state.info = Some(info);

        if head_changed {
            self.jobs.abort_repo(&repo_root);
            self.cache.retain_head(&repo_root, head.as_deref());
        }
        if refresh_again {
            self.refresh_repo(&repo_root, ctx);
        }
        if changed {
            ctx.emit(BlameEvent::Updated);
        }
    }

    /// The whole message of commit `sha`, or `None` until git has printed it. Asking starts the
    /// read; [`BlameEvent::Updated`] follows.
    pub fn commit_message(
        &mut self,
        repo_root: &Path,
        sha: &str,
        ctx: &mut ModelContext<Self>,
    ) -> Option<Arc<str>> {
        let key = (repo_root.to_path_buf(), sha.to_string());
        match self.messages.get(&key) {
            Some(Resolution::Known(message)) => return message.clone(),
            Some(Resolution::Pending) => return None,
            None => {}
        }
        if self.messages.len() >= MAX_REMEMBERED_MESSAGES {
            self.messages.clear();
        }
        self.messages.insert(key.clone(), Resolution::Pending);

        let root = repo_root.to_path_buf();
        let sha = sha.to_string();
        ctx.spawn(
            async move {
                run_git_command(&root, &["show", "-s", "--format=%B", sha.as_str()])
                    .await
                    .ok()
            },
            move |me, message, ctx| {
                let message = message
                    .map(|message| Arc::<str>::from(message.trim_end()))
                    .filter(|message| !message.is_empty());
                me.messages.insert(key, Resolution::Known(message));
                ctx.emit(BlameEvent::Updated);
            },
        );
        None
    }

    fn resolve_file(&mut self, path: PathBuf, ctx: &mut ModelContext<Self>) {
        if self.files.len() >= MAX_REMEMBERED_PATHS {
            self.files.clear();
        }
        self.files.insert(path.clone(), Resolution::Pending);
        ctx.spawn(locate_in_repository(path.clone()), move |me, file, ctx| {
            me.files.insert(path, Resolution::Known(file));
            ctx.emit(BlameEvent::Updated);
        });
    }
}

fn exceeds_blame_limits(contents: &str) -> bool {
    contents.len() > MAX_BLAME_BYTES
        || contents.bytes().filter(|byte| *byte == b'\n').count() > MAX_BLAME_LINES
}

/// Finds the work tree `path` is in by asking git from the file's directory, which also covers
/// linked work trees, submodules and repositories nested in another one.
async fn locate_in_repository(path: PathBuf) -> Option<BlameFile> {
    let directory = path.parent()?;
    let file_name = path.file_name()?.to_str()?;
    let output = run_git_command(
        directory,
        &["rev-parse", "--show-toplevel", "--show-prefix"],
    )
    .await
    .ok()?;
    let mut lines = output.lines();
    let repo_root = PathBuf::from(lines.next().filter(|root| !root.is_empty())?);
    // The directory relative to the root, empty at the root and ending in a slash below it.
    let prefix = lines.next().unwrap_or_default();
    Some(BlameFile {
        repo_root,
        relative_path: format!("{prefix}{file_name}"),
    })
}

async fn load_repo_info(repo_root: PathBuf) -> RepoInfo {
    let head = run_git_command(&repo_root, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .await
        .ok()
        .map(|output| output.trim().to_string())
        .filter(|head| !head.is_empty());
    // Exits with an error when none of the keys is set, which reads as an empty configuration.
    let config = run_git_command(
        &repo_root,
        &[
            "config",
            "--get-regexp",
            r"^(user\.name|user\.email|remote\.origin\.url)$",
        ],
    )
    .await
    .unwrap_or_default();
    let (identity, origin_url) = parse_repo_config(&config);

    RepoInfo {
        head,
        identity,
        origin_url,
    }
}

/// Reads the output of `git config --get-regexp`: one `key value` per line, in the order git
/// reads its configuration files, so that a later line overrides an earlier one.
pub(super) fn parse_repo_config(output: &str) -> (GitIdentity, Option<String>) {
    let mut identity = GitIdentity::default();
    let mut origin_url = None;
    for line in output.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        let value = Some(value.trim().to_string()).filter(|value| !value.is_empty());
        match key {
            "user.name" => identity.name = value,
            "user.email" => identity.email = value,
            "remote.origin.url" => origin_url = value,
            _ => {}
        }
    }
    (identity, origin_url)
}

/// Blames `contents` as the text of `file`, so that lines changed in the buffer but not yet
/// committed, or not even saved, are reported as uncommitted by git itself.
///
/// Neither whitespace changes are skipped (`-w`) nor moved or copied lines followed (`-M`,
/// `-C`): the result then matches plain `git blame`, and following moves makes git several times
/// slower on large files.
pub(super) async fn compute_blame(
    file: BlameFile,
    contents: String,
    timeout: Duration,
) -> BlameResult {
    let started = Instant::now();
    let parser = Mutex::new(BlameParser::default());
    let args = [
        "blame",
        "--incremental",
        "--contents",
        "-",
        "--",
        file.relative_path.as_str(),
    ];
    let finished = {
        let blame =
            run_git_command_streaming(&file.repo_root, &args, Some(contents.as_bytes()), |chunk| {
                parser.lock().feed(chunk)
            });
        let timer = Timer::after(timeout);
        futures::pin_mut!(blame);
        futures::pin_mut!(timer);
        match select(blame, timer).await {
            Either::Left((result, _)) => Some(result),
            // Leaving this block drops the command, which kills git.
            Either::Right(_) => None,
        }
    };
    let blame = parser.into_inner().finish();
    let blamed_lines = blame.blamed_line_count();
    let timed_out = finished.is_none();

    let result = match finished {
        Some(Ok(())) => BlameResult::Blamed(blame),
        Some(Err(error)) => {
            let message = error.to_string();
            log::debug!("git blame of {} failed: {message}", file.relative_path);
            if message.contains("no such path") {
                BlameResult::NotTracked
            } else {
                BlameResult::Failed
            }
        }
        None if blamed_lines == 0 => BlameResult::TimedOut,
        None => BlameResult::Blamed(blame),
    };
    log::info!(
        "git blame of {} ({} bytes): {blamed_lines} lines attributed in {} ms{}",
        file.relative_path,
        contents.len(),
        started.elapsed().as_millis(),
        if timed_out {
            ", stopped at the time limit"
        } else {
            ""
        },
    );
    result
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
