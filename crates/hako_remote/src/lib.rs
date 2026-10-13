//! The desktop side of Hako remote access: the wire protocol shared with the relay and the
//! outbound connection that carries it. Nothing here knows about the app's windows or views.

pub mod backoff;
pub mod connection;
pub mod output_frame;
pub mod protocol;

pub use connection::{
    ConfigError, Connection, ConnectionConfig, ConnectionState, Incoming, Responder, agent_endpoint,
};
