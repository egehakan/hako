//! The desktop side of Hako remote access: the wire protocol shared with the relay, the
//! outbound connection that carries it, the bookkeeping of a terminal's output on its way out,
//! the file, transfer and source control methods, and the Mac browser. Nothing here knows about
//! the app's windows or views.

pub mod archive;
pub mod backoff;
pub mod boundary;
pub mod browser;
mod browser_engine;
pub mod browser_input;
pub mod browser_sessions;
pub mod cdp;
pub mod chrome;
pub mod connection;
pub mod extract;
pub mod file_ops;
pub mod files;
pub mod git;
pub mod git_graph;
pub mod git_history;
mod git_history_format;
pub mod git_merge;
pub mod git_run;
mod git_status;
#[cfg(test)]
mod git_test_support;
pub mod notify;
pub mod output_frame;
pub mod output_stream;
pub mod previews;
pub mod prompt_lines;
pub mod protocol;
pub mod rename;
pub mod streams;
pub mod transfers;
pub mod upload_names;
pub mod uploads_area;

pub use browser_engine::BrowserConfig;
pub use connection::{
    ConfigError, Connection, ConnectionConfig, ConnectionState, Incoming, Responder, agent_endpoint,
};
