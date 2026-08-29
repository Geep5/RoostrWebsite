/**
 * Proto layer — Change codec + content addressing (implements ProtoApi).
 *
 * Decoded changes are fully JSON-safe: change id / parent ids are hex
 * strings, every nested bytes field (custom.data, snapshot.content,
 * bytesValue) is a base64 string — the exact representation protobufjs
 * fromObject() accepts back on encode, so decode → encode roundtrips
 * byte-for-byte and content addresses stay stable.
 */
import protobuf from "protobufjs";
import { sha256 } from "@noble/hashes/sha2.js";
import { PROTO_TEXT } from "./proto-text";
import type { ChangeJSON, ProtoApi } from "./contracts";

// ── Schema ──────────────────────────────────────────────────────────

const root = protobuf.parse(PROTO_TEXT).root;
const ChangeType = root.lookupType("glon.Change");

/**
 * defaults:true fills proto3 zero values (matching the Odin server's
 * always-present JSON keys); oneof members appear only when set, so
 * every OpJSON / Value has exactly one member. bytes:String yields
 * base64 for all bytes fields.
 */
const DECODE_OPTS = { bytes: String, longs: Number, defaults: true } as const;

// ── Hex helpers ─────────────────────────────────────────────────────

const HEX = "0123456789abcdef";

function bytesToHex(b: Uint8Array): string {
	let out = "";
	for (let i = 0; i < b.length; i++) out += HEX[b[i] >> 4] + HEX[b[i] & 15];
	return out;
}

function hexToBytes(hex: string): Uint8Array {
	const out = new Uint8Array(hex.length >> 1);
	for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
	return out;
}

function base64ToBytes(b64: string): Uint8Array {
	const bin = atob(b64);
	const out = new Uint8Array(bin.length);
	for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
	return out;
}

// ── Codec ───────────────────────────────────────────────────────────

interface ChangeWire extends Omit<ChangeJSON, "id" | "parentIds"> {
	id: string; // base64 on the wire object
	parentIds: string[];
	snapshot?: unknown;
}

export function decodeChange(bytes: Uint8Array): ChangeJSON | null {
	try {
		const msg = ChangeType.decode(bytes);
		const obj = ChangeType.toObject(msg, DECODE_OPTS) as unknown as ChangeWire;
		const change: ChangeJSON = {
			id: bytesToHex(base64ToBytes(obj.id ?? "")),
			objectId: obj.objectId ?? "",
			parentIds: (obj.parentIds ?? []).map((p) => bytesToHex(base64ToBytes(p))),
			ops: obj.ops ?? [],
			timestamp: obj.timestamp ?? 0,
			author: obj.author ?? "",
		};
		if (obj.snapshot != null) change.snapshot = obj.snapshot;
		return change;
	} catch {
		return null;
	}
}

/** Wire-object with the given id bytes; ops/snapshot pass through fromObject. */
function encodeWithId(change: ChangeJSON, id: Uint8Array): Uint8Array {
	const obj = {
		id,
		objectId: change.objectId,
		parentIds: change.parentIds.map(hexToBytes),
		ops: change.ops,
		timestamp: change.timestamp,
		author: change.author,
		...(change.snapshot != null ? { snapshot: change.snapshot } : {}),
	};
	const msg = ChangeType.fromObject(obj);
	return ChangeType.encode(msg).finish();
}

/**
 * Hash preimage: the change encoded with the id field zeroed, emitted as
 * `0a 00` (tag + zero length). Every glon writer hashes this exact form;
 * protobufjs 8 skips empty proto3 bytes fields, so the prefix is added
 * by hand.
 */
function encodeForHashing(change: ChangeJSON): Uint8Array {
	const body = encodeWithId(change, new Uint8Array(0));
	const out = new Uint8Array(body.length + 2);
	out[0] = 0x0a;
	out[1] = 0x00;
	out.set(body, 2);
	return out;
}

/** Content address: sha256 hex of the encoding with id zeroed. */
export function changeId(change: ChangeJSON): string {
	return bytesToHex(sha256(encodeForHashing(change)));
}

/** Canonical bytes with the (recomputed) content-address id set. */
export function encodeChange(change: ChangeJSON): Uint8Array {
	return encodeWithId(change, sha256(encodeForHashing(change)));
}

export const proto: ProtoApi = { decodeChange, encodeChange, changeId };
