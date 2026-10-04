/**
 * negentropy.ts — Negentropy range-based set reconciliation, protocol V1
 * (0x61), byte-exact with the reference implementation
 * (https://github.com/hoytech/negentropy, js/Negentropy.js) that NIP-77
 * carries over NEG-OPEN / NEG-MSG.
 *
 * Item = (timestamp, 32-byte id), sorted by timestamp then id bytes.
 * Fingerprint = first 16 bytes of sha256(sum of ids as little-endian
 * 256-bit integers mod 2^256 || varint(count)).
 *
 * Only the initiator (client) side is used by the engine; the responder
 * side is implemented too so tests can reconcile two in-process peers.
 */

import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";

export const PROTOCOL_VERSION = 0x61;
const ID_SIZE = 32;
const FINGERPRINT_SIZE = 16;
const BUCKETS = 16;
/** The reserved "infinity" timestamp (2^64-1 on the wire, encoded as 0). */
const INFINITY = Number.POSITIVE_INFINITY;

const enum Mode {
	Skip = 0,
	Fingerprint = 1,
	IdList = 2,
}

export interface NegentropyItem {
	timestamp: number;
	id: Uint8Array;
}

interface Bound {
	timestamp: number;
	/** Id prefix: 0..32 bytes. */
	id: Uint8Array;
}

const EMPTY = new Uint8Array(0);

function compareBytes(a: Uint8Array, b: Uint8Array): number {
	const n = Math.min(a.length, b.length);
	for (let i = 0; i < n; i++) if (a[i] !== b[i]) return a[i] - b[i];
	return a.length - b.length;
}

function compareItems(a: Bound, b: Bound): number {
	if (a.timestamp !== b.timestamp) return a.timestamp < b.timestamp ? -1 : 1;
	return compareBytes(a.id, b.id);
}

/** Growable byte buffer. */
class Writer {
	private buf = new Uint8Array(256);
	length = 0;

	private reserve(n: number): void {
		if (this.length + n <= this.buf.length) return;
		const next = new Uint8Array(Math.max(this.buf.length * 2, this.length + n));
		next.set(this.buf.subarray(0, this.length));
		this.buf = next;
	}

	byte(b: number): void {
		this.reserve(1);
		this.buf[this.length++] = b;
	}

	bytes(b: Uint8Array): void {
		this.reserve(b.length);
		this.buf.set(b, this.length);
		this.length += b.length;
	}

	varint(n: number): void {
		if (!Number.isSafeInteger(n) || n < 0) throw new Error(`negentropy: bad varint ${n}`);
		const digits: number[] = [];
		do {
			digits.push(n % 128);
			n = Math.floor(n / 128);
		} while (n > 0);
		for (let i = digits.length - 1; i >= 0; i--) this.byte(i > 0 ? digits[i] | 0x80 : digits[i]);
	}

	view(): Uint8Array {
		return this.buf.subarray(0, this.length);
	}
}

class Reader {
	private pos = 0;
	constructor(private readonly buf: Uint8Array) {}

	get remaining(): number {
		return this.buf.length - this.pos;
	}

	byte(): number {
		if (this.pos >= this.buf.length) throw new Error("negentropy: parse ends prematurely");
		return this.buf[this.pos++];
	}

	bytes(n: number): Uint8Array {
		if (this.remaining < n) throw new Error("negentropy: parse ends prematurely");
		const out = this.buf.subarray(this.pos, this.pos + n);
		this.pos += n;
		return out;
	}

	varint(): number {
		let res = 0;
		for (;;) {
			const b = this.byte();
			res = res * 128 + (b & 0x7f);
			if (!Number.isSafeInteger(res)) throw new Error("negentropy: varint overflow");
			if ((b & 0x80) === 0) return res;
		}
	}
}

/** Sum of ids mod 2^256, little-endian, as eight 32-bit limbs. */
class Accumulator {
	private readonly limbs = new Uint32Array(8);

	add(id: Uint8Array): void {
		const v = new DataView(id.buffer, id.byteOffset, ID_SIZE);
		let carry = 0;
		for (let i = 0; i < 8; i++) {
			const next = this.limbs[i] + v.getUint32(i * 4, true) + carry;
			this.limbs[i] = next >>> 0;
			carry = next > 0xffffffff ? 1 : 0;
		}
	}

	fingerprint(count: number): Uint8Array {
		const w = new Writer();
		const sum = new Uint8Array(ID_SIZE);
		const v = new DataView(sum.buffer);
		for (let i = 0; i < 8; i++) v.setUint32(i * 4, this.limbs[i], true);
		w.bytes(sum);
		w.varint(count);
		return sha256(w.view()).subarray(0, FINGERPRINT_SIZE);
	}
}

/** Sorted, immutable item set (the reference's NegentropyStorageVector). */
export class NegentropyStorage {
	private readonly items: NegentropyItem[] = [];
	private sealed = false;

	insert(timestamp: number, id: string | Uint8Array): void {
		if (this.sealed) throw new Error("negentropy: already sealed");
		if (!Number.isSafeInteger(timestamp) || timestamp < 0) throw new Error(`negentropy: bad timestamp ${timestamp}`);
		const bytes = typeof id === "string" ? hexToBytes(id) : id;
		if (bytes.length !== ID_SIZE) throw new Error("negentropy: bad id size for added item");
		this.items.push({ timestamp, id: bytes });
	}

	seal(): void {
		if (this.sealed) throw new Error("negentropy: already sealed");
		this.sealed = true;
		this.items.sort(compareItems);
		for (let i = 1; i < this.items.length; i++) {
			if (compareItems(this.items[i - 1], this.items[i]) === 0) throw new Error("negentropy: duplicate item inserted");
		}
	}

	size(): number {
		this.checkSealed();
		return this.items.length;
	}

	item(i: number): NegentropyItem {
		return this.items[i];
	}

	/** First index in [begin, end) whose item is not below `bound`. */
	lowerBound(begin: number, end: number, bound: Bound): number {
		this.checkSealed();
		let count = end - begin;
		while (count > 0) {
			const step = count >> 1;
			const it = begin + step;
			if (compareItems(this.items[it], bound) < 0) {
				begin = it + 1;
				count -= step + 1;
			} else count = step;
		}
		return begin;
	}

	fingerprint(begin: number, end: number): Uint8Array {
		this.checkSealed();
		const acc = new Accumulator();
		for (let i = begin; i < end; i++) acc.add(this.items[i].id);
		return acc.fingerprint(end - begin);
	}

	private checkSealed(): void {
		if (!this.sealed) throw new Error("negentropy: not sealed");
	}
}

export interface ReconcileResult {
	/** Next message (hex) to send, or null when the initiator is done. */
	next: string | null;
	/** Ids we hold that the other side lacks (initiator only). */
	haveIds: string[];
	/** Ids the other side holds that we lack (initiator only). */
	needIds: string[];
}

export class Negentropy {
	private initiator = false;
	private lastTimestampIn = 0;
	private lastTimestampOut = 0;

	/** `frameSizeLimit` in bytes (0 = unlimited); hex on the wire doubles it. */
	constructor(private readonly storage: NegentropyStorage, private readonly frameSizeLimit = 0) {
		if (frameSizeLimit !== 0 && frameSizeLimit < 4096) throw new Error("negentropy: frameSizeLimit too small");
	}

	/** The initiator's first message (hex). */
	initiate(): string {
		if (this.initiator) throw new Error("negentropy: already initiated");
		this.initiator = true;
		this.lastTimestampOut = 0;
		const out = new Writer();
		out.byte(PROTOCOL_VERSION);
		this.splitRange(0, this.storage.size(), { timestamp: INFINITY, id: EMPTY }, out);
		return bytesToHex(out.view());
	}

	reconcile(queryHex: string): ReconcileResult {
		const haveIds: string[] = [];
		const needIds: string[] = [];
		const query = new Reader(hexToBytes(queryHex));
		this.lastTimestampIn = this.lastTimestampOut = 0;

		const full = new Writer();
		full.byte(PROTOCOL_VERSION);

		const version = query.byte();
		if (version < 0x60 || version > 0x6f) throw new Error("negentropy: invalid protocol version byte");
		if (version !== PROTOCOL_VERSION) {
			if (this.initiator) throw new Error(`negentropy: unsupported protocol version requested: ${version - 0x60}`);
			return { next: bytesToHex(full.view()), haveIds, needIds };
		}

		const storageSize = this.storage.size();
		let prevBound: Bound = { timestamp: 0, id: EMPTY };
		let prevIndex = 0;
		let skip = false;

		while (query.remaining !== 0) {
			let o = new Writer();
			const doSkip = () => {
				if (!skip) return;
				skip = false;
				this.encodeBound(o, prevBound);
				o.varint(Mode.Skip);
			};

			const currBound = this.decodeBound(query);
			const mode = query.varint();
			const lower = prevIndex;
			let upper = this.storage.lowerBound(prevIndex, storageSize, currBound);

			if (mode === Mode.Skip) {
				skip = true;
			} else if (mode === Mode.Fingerprint) {
				const theirs = query.bytes(FINGERPRINT_SIZE);
				const ours = this.storage.fingerprint(lower, upper);
				if (compareBytes(theirs, ours) !== 0) {
					doSkip();
					this.splitRange(lower, upper, currBound, o);
				} else skip = true;
			} else if (mode === Mode.IdList) {
				const numIds = query.varint();
				const theirs = new Set<string>();
				for (let i = 0; i < numIds; i++) {
					const id = query.bytes(ID_SIZE);
					if (this.initiator) theirs.add(bytesToHex(id));
				}
				if (this.initiator) {
					skip = true;
					for (let i = lower; i < upper; i++) {
						const k = bytesToHex(this.storage.item(i).id);
						if (!theirs.delete(k)) haveIds.push(k);
					}
					for (const k of theirs.keys()) needIds.push(k);
				} else {
					doSkip();
					const ids = new Writer();
					let count = 0;
					let endBound: Bound = currBound;
					for (let i = lower; i < upper; i++) {
						if (this.exceeded(full.length + ids.length)) {
							endBound = this.storage.item(i);
							upper = i; // the remaining range gets its own fingerprint
							break;
						}
						ids.bytes(this.storage.item(i).id);
						count++;
					}
					this.encodeBound(o, endBound);
					o.varint(Mode.IdList);
					o.varint(count);
					o.bytes(ids.view());
					full.bytes(o.view());
					o = new Writer();
				}
			} else {
				throw new Error("negentropy: unexpected mode");
			}

			if (this.exceeded(full.length + o.length)) {
				// Frame full: stop here and fingerprint everything left.
				const remaining = this.storage.fingerprint(upper, storageSize);
				this.encodeBound(full, { timestamp: INFINITY, id: EMPTY });
				full.varint(Mode.Fingerprint);
				full.bytes(remaining);
				break;
			}
			full.bytes(o.view());
			prevIndex = upper;
			prevBound = currBound;
		}

		return {
			next: full.length === 1 && this.initiator ? null : bytesToHex(full.view()),
			haveIds,
			needIds,
		};
	}

	private splitRange(lower: number, upper: number, upperBound: Bound, o: Writer): void {
		const numElems = upper - lower;
		if (numElems < BUCKETS * 2) {
			this.encodeBound(o, upperBound);
			o.varint(Mode.IdList);
			o.varint(numElems);
			for (let i = lower; i < upper; i++) o.bytes(this.storage.item(i).id);
			return;
		}
		const perBucket = Math.floor(numElems / BUCKETS);
		const withExtra = numElems % BUCKETS;
		let curr = lower;
		for (let i = 0; i < BUCKETS; i++) {
			const size = perBucket + (i < withExtra ? 1 : 0);
			const fingerprint = this.storage.fingerprint(curr, curr + size);
			curr += size;
			const next = curr === upper ? upperBound : this.minimalBound(this.storage.item(curr - 1), this.storage.item(curr));
			this.encodeBound(o, next);
			o.varint(Mode.Fingerprint);
			o.bytes(fingerprint);
		}
	}

	private exceeded(n: number): boolean {
		return this.frameSizeLimit !== 0 && n > this.frameSizeLimit - 200;
	}

	private decodeBound(r: Reader): Bound {
		let timestamp = r.varint();
		timestamp = timestamp === 0 ? INFINITY : timestamp - 1;
		if (this.lastTimestampIn === INFINITY || timestamp === INFINITY) {
			this.lastTimestampIn = INFINITY;
			timestamp = INFINITY;
		} else {
			timestamp += this.lastTimestampIn;
			this.lastTimestampIn = timestamp;
		}
		const len = r.varint();
		if (len > ID_SIZE) throw new Error("negentropy: bound key too long");
		return { timestamp, id: r.bytes(len) };
	}

	private encodeBound(o: Writer, bound: Bound): void {
		if (bound.timestamp === INFINITY) {
			this.lastTimestampOut = INFINITY;
			o.varint(0);
		} else {
			const delta = bound.timestamp - this.lastTimestampOut;
			this.lastTimestampOut = bound.timestamp;
			o.varint(delta + 1);
		}
		o.varint(bound.id.length);
		o.bytes(bound.id);
	}

	private minimalBound(prev: NegentropyItem, curr: NegentropyItem): Bound {
		if (curr.timestamp !== prev.timestamp) return { timestamp: curr.timestamp, id: EMPTY };
		let shared = 0;
		while (shared < ID_SIZE && curr.id[shared] === prev.id[shared]) shared++;
		return { timestamp: curr.timestamp, id: curr.id.subarray(0, shared + 1) };
	}
}
