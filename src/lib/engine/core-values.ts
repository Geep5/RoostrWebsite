/** Protobuf maps carry insertion order, unlike Odin's JSON object maps.
 * Only schema map slots become ordered pairs; user map keys are never interpreted
 * as schema properties. This is transport encoding, not a second domain codec.
 */
type RecordValue = Record<string, unknown>;

function record(value: unknown): value is RecordValue {
	return value !== null && typeof value === "object" && !Array.isArray(value) && !(value instanceof Uint8Array);
}

function mapSlot(value: unknown, unpack: boolean, strings: boolean): unknown {
	if (unpack) {
		if (!Array.isArray(value)) return value;
		return Object.fromEntries(value.map(([key, item]: [string, unknown]) => [key, strings ? item : transform(item, true)]));
	}
	if (!record(value)) return value;
	return Object.entries(value).map(([key, item]) => [key, strings ? item : transform(item, false)]);
}

function transform(value: unknown, unpack: boolean, slot = ""): unknown {
	if (value instanceof Uint8Array) {
		let binary = "";
		for (const byte of value) binary += String.fromCharCode(byte);
		return btoa(binary);
	}
	if (Array.isArray(value)) return value.map((item) => transform(item, unpack, slot));
	if (!record(value)) return value;
	const out: RecordValue = {};
	for (const [key, item] of Object.entries(value)) {
		if (key === "entries") {
			// ValueMap's only field. Its keys are opaque, so do not recurse into
			// the dictionary itself (a legitimate map key can also be "entries").
			out[key] = mapSlot(item, unpack, false);
		} else if (key === "meta") {
			out[key] = mapSlot(item, unpack, true);
		} else if (key === "fields") {
			// Blocks use a ValueMap wrapper; objects/snapshots use a direct map.
			const block = slot === "block" || slot === "blocks" || "childrenIds" in value;
			out[key] = block ? transform(item, unpack) : mapSlot(item, unpack, false);
		} else {
			Object.defineProperty(out, key, { value: transform(item, unpack, key), enumerable: true, configurable: true, writable: true });
		}
	}
	return out;
}

export function packCoreValueMaps(value: unknown): unknown {
	return transform(value, false);
}

export function unpackCoreValueMaps<T>(value: unknown): T {
	return transform(value, true) as T;
}
