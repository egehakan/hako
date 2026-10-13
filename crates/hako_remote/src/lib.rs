//! The desktop side of Hako remote access: the wire protocol shared with the relay, the
//! outbound connection that carries it, the bookkeeping of a terminal's output on its way out,
//! and the file, transfer and source control methods. Nothing here knows about the app's windows
//! or views.

pub mod backoff;
pub mod boundary;
pub mod connection;
pub mod files;
pub mod git;
pub mod git_graph;
pub mod git_history;
mod git_history_format;
pub mod git_run;
mod git_status;
#[cfg(test)]
mod git_test_support;
pub mod notify;
pub mod output_frame;
pub mod output_stream;
pub mod prompt_lines;
pub mod protocol;
pub mod transfers;
pub mod upload_names;
pub mod uploads_area;

pub use connection::{
    ConfigError, Connection, ConnectionConfig, ConnectionState, Incoming, Responder, agent_endpoint,
};
