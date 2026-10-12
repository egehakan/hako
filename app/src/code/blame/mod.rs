//! Who last changed each line of the files open in code editors, read from `git blame`.

mod format;
mod model;
mod porcelain;

pub use format::annotation_text;
pub use model::{BlameEvent, BlameFile, BlameLookup, BlameModel, BlameResult, RepoInfo};
pub use porcelain::BlameCommit;
