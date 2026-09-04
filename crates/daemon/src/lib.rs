//! Resident daemon: unix-socket server holding models and indexes.

pub mod client;
pub mod codec;
pub mod handshake;
pub mod paths;
pub mod protocol;
pub mod resident;
pub mod server;

pub use client::DaemonClient;
pub use protocol::{
    DaemonError, DaemonStatus, Envelope, IndexMode, IndexReportWire, MAX_REQUEST_BYTES,
    PROTOCOL_VERSION, Request, Response,
};
pub use server::{BindOutcome, Daemon, DaemonConfig};
