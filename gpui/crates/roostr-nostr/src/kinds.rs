//! Event kinds the Roostr hosts publish and read (sync.ts, wire.odin).

/// NIP-01 profile metadata.
pub const METADATA: u32 = 0;
/// NIP-09 deletion; on a space stream, the owner's h-deletion of the space.
pub const DELETION: u32 = 5;
/// NIP-59 seal.
pub const SEAL: u32 = 13;
/// One sealed change (or one part of a chunked change).
pub const CHANGE: u32 = 1078;
/// One sealed checkpoint part per object.
pub const CHECKPOINT: u32 = 1079;
/// Parts 1..n-1 of a base; part 0 rides in the [`BASE`] event.
pub const BASE_PART: u32 = 1080;
/// NIP-59 gift wrap.
pub const GIFT_WRAP: u32 = 1059;
/// BUD-11 Blossom authorization.
pub const BLOSSOM_AUTH: u32 = 24242;
/// Gift-wrapped rumor carrying a space key to a member.
pub const SPACE_INVITE_RUMOR: u32 = 24891;
/// Gift-wrapped rumor asking a space owner to join.
pub const JOIN_REQUEST_RUMOR: u32 = 24892;
/// NIP-98 HTTP authorization.
pub const HTTP_AUTH: u32 = 27235;
/// The relay write-allowlist an owner publishes (`d` = [`ALLOWLIST_D`]).
pub const ALLOWLIST: u32 = 30100;
/// A base: addressable, part 0 of the object's compacted state.
pub const BASE: u32 = 31078;

/// The allowlist event's `d` tag.
pub const ALLOWLIST_D: &str = "roostr-allowlist";
/// Every DAG kind a sync filter pulls.
pub const DAG: [u32; 4] = [CHANGE, CHECKPOINT, BASE, BASE_PART];
/// A shared space's `#h` filter: its DAG kinds plus h-deletions.
pub const SPACE: [u32; 5] = [CHANGE, CHECKPOINT, BASE, BASE_PART, DELETION];
