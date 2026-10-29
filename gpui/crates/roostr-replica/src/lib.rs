//! The daemon-less replica (`src/lib/engine/backend.ts`, iOS `RoostrSync`):
//! a [`roostr_core::Backend`] that keeps the vault on this device - the
//! shared Odin engine replays, queries and plans, an on-device change store
//! holds every change, and Nostr relays are the only network.

mod backend;
mod device;
pub mod identity;
pub mod keyring;
pub mod local;
pub mod records;
pub mod rt;
pub mod sync;

pub use backend::{DEVICE_ID, HOLDING, Occurrence, PROFILE, RELAYS, ReplicaBackend, ReplicaParts};
pub use device::DeviceInfo;
pub use roostr_replica_store::{MaybeSend, MaybeSync};
