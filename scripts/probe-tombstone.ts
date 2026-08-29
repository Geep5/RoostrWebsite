import { SimplePool } from "nostr-tools";
import { getPublicKey } from "nostr-tools";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import { readFileSync } from "node:fs";

const sk = hexToBytes(JSON.parse(readFileSync(process.env.HOME + "/.glon/nostr.json", "utf8")).privkey);
const pk = getPublicKey(sk);
const objectId = "5579a9b8-701f-413e-a659-673119d56f43";
const h = bytesToHex(sha256(new Uint8Array([...sk, ...new TextEncoder().encode(objectId)]))).slice(0, 16);
const pool = new SimplePool();
const relays = ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.nostr.band"];
const evs = await pool.querySync(relays, { kinds: [1078], authors: [pk], "#h": [h] });
console.log("events for object:", evs.length, evs.map(e => ({ at: e.created_at, id: e.id.slice(0, 8) })));
pool.close(relays);
process.exit(0);
