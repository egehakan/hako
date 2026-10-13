//! The desktop side of Hako remote access: the wire protocol shared with the relay, the
//! outbound connection that carries it, the bookkeeping of a terminal's output on its way out,
//! and the file methods. Nothing here knows about the app's windows or views.

pub mod backoff;
pub mod boundary;
pub mod connection;
pub mod files;
mod git_status;
pub mod output_frame;
pub mod output_stream;
pub mod prompt_lines;
pub mod protocol;

pub use connection::{
    ConfigError, Connection, ConnectionConfig, ConnectionState, Incoming, Responder, agent_endpoint,
};
