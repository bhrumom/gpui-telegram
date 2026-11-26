//! Pure-Rust multi-agent orchestration DAG, generation-fenced dispatches,
//! typed mailbox routing, review/merge gates, multi-host federation,
//! Unified Runtime RPC Protocol v3, transcript-bound E2EE v2 (X25519 +
//! HKDF-SHA256 96-byte key schedule + ChaCha20-Poly1305 + Ed25519),
//! atomic snapshots & protected secrets, diff annotations, Design Mode,
//! Plugin V2 manifests, 8-check hibernation policy, and ComputerUse guards.

pub mod diff_design_plugins_guards;
pub mod orchestration;
pub mod rpc_e2ee_vault;

pub use diff_design_plugins_guards::*;
pub use orchestration::*;
pub use rpc_e2ee_vault::*;
