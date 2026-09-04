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
    ClientRole, DaemonError, DaemonStatus, Envelope, EventCursor, IndexMode, IndexPhase,
    IndexProgressSnapshot, IndexReportWire, LastIndexCompletion, Lifecycle, MAX_REQUEST_BYTES,
    OBSERVER_CLIENT, Observation, PROTOCOL_VERSION, PROTOCOL_VERSION_V1, Request, RequestEvent,
    ResourceSnapshot, Response,
};
#[cfg(feature = "resident")]
pub use server::{BindOutcome, Daemon, DaemonConfig};
