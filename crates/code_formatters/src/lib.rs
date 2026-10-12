//! Formatting buffer text with the formatters a project already uses (Prettier, Ruff, rustfmt and
//! so on).
//!
//! Nothing here installs or bundles a formatter. A tool is used only when its executable is found
//! and, for tools that format differently per project, the project opts in to it the way that
//! tool's own editor integration would expect.

pub mod config;
pub mod discovery;
mod plan;
mod preference;
mod registry;
#[cfg(all(test, unix))]
mod test_support;

pub use plan::{Arg, Invocation, Selection, select};
pub use preference::Preference;
pub use registry::{Language, Tool};
