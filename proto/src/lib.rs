pub mod paths;
pub mod protocol;
pub mod sync;
pub mod tcp;
pub mod transport;

// Re-export the most commonly used types at crate root.
pub use protocol::{DualieMessage, ClipboardText, FileChunk, SyncEntry, PROTOCOL_VERSION};
pub use sync::{ConflictRecord, SyncDecision, SyncPair, reconcile};
pub use tcp::{TcpPeer, TcpPeerReader, TcpPeerWriter};
pub use transport::{PeerTransport, decode_frame, encode_frame};
