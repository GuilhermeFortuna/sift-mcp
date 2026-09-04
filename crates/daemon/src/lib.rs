//! Resident daemon: unix-socket server holding models and indexes.

pub mod client;
pub mod codec;
pub mod handshake;
pub mod paths;
pub mod protocol;
#[cfg(feature = "resident")]
pub mod resident;
#[cfg(feature = "resident")]
pub mod server;

pub use client::DaemonClient;
pub use protocol::{
    DaemonError, DaemonStatus, Envelope, IndexMode, IndexPhase, IndexReportWire, MAX_REQUEST_BYTES,
    PROTOCOL_VERSION, Request, Response,
};
#[cfg(feature = "resident")]
pub use server::{BindOutcome, Daemon, DaemonConfig};
