import { SimplePool, getPublicKey, nip44 } from "nostr-tools";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import { readFileSync } from "node:fs";
import { proto } from "../src/lib/engine/proto";

const sk = hexToBytes(JSON.parse(readFileSync(process.env.HOME + "/.glon/nostr.json", "utf8")).privkey);
const pk = getPublicKey(sk);
const ck = nip44.getConversationKey(sk, pk);
const objectId = "5579a9b8-701f-413e-a659-673119d56f43";
const h = bytesToHex(sha256(new Uint8Array([...sk, ...new TextEncoder().encode(objectId)]))).slice(0, 16);
const pool = new SimplePool();
const relays = ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.nostr.band"];
const evs = await pool.querySync(relays, { kinds: [1078], authors: [pk], "#h": [h] });
for (const e of evs) {
  try {
    const b64 = nip44.decrypt(e.content, ck);
    const chunk = e.tags.find((t) => t[0] === "c");
    if (chunk) { console.log(e.created_at, "CHUNK", chunk.slice(1)); continue; }
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const c = proto.decodeChange(bytes);
    console.log(e.created_at, c?.id.slice(0, 12), JSON.stringify(c?.ops.map(o => Object.keys(o))));
  } catch (err) {
    console.log(e.created_at, "ERR", String(err).slice(0, 80));
  }
}
pool.close(relays);
process.exit(0);
