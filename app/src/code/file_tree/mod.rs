//! File picker component for rendering expandable folder structures.

pub mod snapshot;

#[cfg_attr(not(feature = "local_fs"), allow(dead_code, unused_imports))]
mod file_ops;
#[cfg_attr(not(feature = "local_fs"), allow(dead_code, unused_imports))]
mod trash;
#[cfg_attr(not(feature = "local_fs"), allow(dead_code, unused_imports))]
mod undo;
#[cfg_attr(not(feature = "local_fs"), allow(dead_code, unused_imports))]
mod view;

pub use view::*;
