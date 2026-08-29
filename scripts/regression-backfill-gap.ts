/**
 * regression-backfill-gap.ts — READ-ONLY regression for the backfill pager.
 *
 * Object 5579a9b8-701f-413e-a659-673119d56f43 has two changes on the
 * relays (create 809d78b8…, delete df512097…). The old merged-relay pager
 * (until = min of the merged page) skipped the delete. This runs the real
 * browser backfill path (fake-indexeddb store, real proto decode) against
 * the live relays and asserts BOTH change ids land in the store.
 *
 *   bun run scripts/regression-backfill-gap.ts
 */

import { hexToBytes } from "@noble/hashes/utils.js";
import { proto } from "../src/lib/engine/proto";
import { ChangeStore } from "../src/lib/engine/store";
import { RelaySync, DEFAULT_RELAYS } from "../src/lib/engine/sync";

const OBJECT_ID = "5579a9b8-701f-413e-a659-673119d56f43";
const EXPECTED_PREFIXES = ["809d78b8", "df512097"];

const identity = (await Bun.file(`${process.env.HOME}/.glon/nostr.json`).json()) as { privkey: string };
const sk = hexToBytes(identity.privkey);

const store = new ChangeStore();
await store.open();

const sync = new RelaySync(
	sk,
	DEFAULT_RELAYS,
	store,
	{
		onObjects: () => {},
		onStatus: (s) => console.log(`[status] ${s.phase}${s.detail ? `: ${s.detail}` : ""}`),
	},
	{ decode: (b) => proto.decodeChange(b) },
);

await sync.start(); // full backfill from cursor 0 — publish() never called
sync.stop();

const changes = await store.changesFor(OBJECT_ID);
const ids = changes.map((c) => c.id);
console.log(`\nobject ${OBJECT_ID}: ${changes.length} change(s)`);
for (const id of ids) console.log(`  ${id}`);
console.log(`events=${sync.stats.events} decryptFail=${sync.stats.decryptFailures} decodeFail=${sync.stats.decodeFailures} imported=${sync.stats.imported}`);

let failed = false;
for (const prefix of EXPECTED_PREFIXES) {
	if (ids.some((id) => id.startsWith(prefix))) {
		console.log(`PASS: change ${prefix}… present`);
	} else {
		failed = true;
		console.error(`FAIL: change ${prefix}… MISSING`);
	}
}
console.log(failed ? "\nRESULT: FAIL" : "\nRESULT: PASS");
process.exit(failed ? 1 : 0);
