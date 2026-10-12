//! Stand-in for the replace of global search in builds without local files to replace in.

#![allow(dead_code)]

use std::path::PathBuf;

use project_replace::apply::{FileTarget, Progress, ReplaceReport};
use project_replace::engine::SearchSpec;
use project_replace::replacement::ReplaceSpec;
use project_replace::scope::ScopeGlobs;
use warpui::{Entity, ModelContext};

pub struct ReplaceJob {
    pub search: SearchSpec,
    pub replace: ReplaceSpec,
    pub roots: Vec<PathBuf>,
    pub scope: ScopeGlobs,
    pub files: Vec<FileTarget>,
}

pub enum ProjectReplaceEvent {
    Progress,
    Finished(ReplaceReport),
    Failed(String),
}

pub struct ProjectReplace {}

impl Entity for ProjectReplace {
    type Event = ProjectReplaceEvent;
}

impl ProjectReplace {
    pub fn new() -> Self {
        ProjectReplace {}
    }

    pub fn is_running(&self) -> bool {
        false
    }

    pub fn progress(&self) -> Option<Progress> {
        None
    }

    pub fn cancel(&mut self, _ctx: &mut ModelContext<Self>) {}

    pub fn start(&mut self, _job: ReplaceJob, ctx: &mut ModelContext<Self>) {
        ctx.emit(ProjectReplaceEvent::Failed(
            "Replacing needs access to local files.".to_string(),
        ));
    }
}

impl Default for ProjectReplace {
    fn default() -> Self {
        Self::new()
    }
}
