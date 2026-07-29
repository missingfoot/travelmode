//! travelmode-core: shared types and the IPC protocol used between the
//! daemon (`travelmoded`) and its clients (CLI, GUI).

pub mod ipc;
pub mod types;

pub use ipc::{read_frame, write_frame, IpcError};
pub use types::*;
