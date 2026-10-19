//! Nostr for the daemon-less replica: what the Svelte browser mode
//! (`src/lib/engine/{keys,negentropy,sync,spacekeys,profile}.ts`,
//! `src/lib/{blossom,invite}.ts`) and the iOS app
//! (`RoostrSync/{Nostr,GiftWrap,SpaceKeyring,Negentropy,Relay}.swift`) keep
//! on the host side of the Odin engine, byte-compatible with both.
//!
//! - [`keys`]: secret parsing (`nsec1…`/hex), x-only pubkeys, BIP-340
//!   signing, ECDH, the author id; [`bech32`] for NIP-19.
//! - [`Event`]: NIP-01 ids and verification, as nostr-tools computes them.
//! - [`nip44`] (v2 payloads) and [`nip59`] (seal + gift wrap, space invites
//!   and join requests). The engine's `wire` method has its own NIP-44 for
//!   the change stream; these serve the host's gift wraps, which must work
//!   without an engine (the public join page).
//! - [`spacekeys`]: the space keyring file and its ensure/rotate/import rules.
//! - [`negentropy`]: NIP-77 range-based set reconciliation (protocol V1).
//! - [`relay`]: the websocket client (NIP-01, NIP-77), live subscriptions
//!   with the hosts' reconnect backoff, publish pacing, multi-relay helpers.
//! - [`nip11`], [`blossom`], [`profile`]: relay information documents,
//!   Blossom discovery and upload authorization, the kind-0 profile.
//!
//! Every future runs on any executor (GPUI's, a test's `block_on`): native
//! sockets use `async-io`, the browser uses `web_sys::WebSocket`. Futures
//! and streams are `Send` on native and need not be in the browser.

pub mod bech32;
pub mod blossom;
mod event;
pub mod filter;
pub mod keys;
pub mod kinds;
pub mod negentropy;
pub mod nip11;
pub mod nip44;
pub mod nip59;
pub mod profile;
mod random;
pub mod relay;
mod shared;
pub mod spacekeys;
mod time;

pub use event::{Event, EventTemplate, event_id};
pub use filter::Filter;
pub use keys::{KeyError, Keys};
pub use relay::{Relay, RelayError};
pub use time::{now_s, sleep, timeout};

/// The relays a fresh identity syncs through (`DEFAULT_RELAYS` in sync.ts).
pub const DEFAULT_RELAYS: &[&str] = &["wss://roostr-relay.fly.dev"];
