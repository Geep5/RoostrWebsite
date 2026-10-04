# Roostr UI

The single Svelte UI for Roostr's browser-offline and native-daemon modes. The
application routes live under `/app`; the root website and `/j` join flow share
this build. The old `glonOdin/app` mirror has been retired.

## Run

```sh
npm install
npm run dev          # browser-owned identity, IndexedDB and Nostr synchronization
npm run dev:local    # paired local Odin daemon; http://127.0.0.1:5190/app
```

Backend selection is explicit: `VITE_ROOSTR_BACKEND=local` selects the local
adapter. Pairing a browser-mode page with a machine grants access to its harness
controls; it does not replace the browser's identity or switch its vault.

Files (pasted/dropped images, `/file`, File pages) need the Roostr running on
the same computer: the bytes live in its harness. In browser mode the first file
action pairs automatically by proof of ownership - the tab signs a one-use,
origin-bound harness challenge with the vault key it already holds, and only the
computer whose identity is that key accepts (see glonOdin README, API). Without
a reachable Roostr the file block says so in place; the iOS app has no Roostr
under it, so files stay unavailable there.

For local mode, start the daemon and independent sync service from the sibling
`glonOdin` repository:

```sh
# glonOdin repository
odin build src -o:speed -out:glon-odin
./glon-odin serve
# separate terminal, glonOdin/harness
bun install
bun run sync
# optional separate agent service
bun run serve
```

Enter the daemon terminal's one-use pairing code in the UI. Sessions are bound
to the requesting Origin and expire after24 hours or a daemon restart. Run
`bun run pair` from `glonOdin/harness` to print a fresh code without restarting.
Bearer tokens travel in request headers, including streamed SSE; they are not
placed in URLs. Browser sessions cannot export the native private key. The
operator-only `glon-odin key-export` command is for explicit private recovery.

## Shared Odin engine

Browser protobuf/hash, replay, queries, ordinary mutation planning and the
relay receive session (chunk reassembly, cursor high-water mark, replay-group
bookkeeping via the `sync` method) use the same Odin sources as the native
daemon, compiled to WebAssembly. IndexedDB, network transport and DOM
integration remain platform adapters.

History sync uses NIP-77 (Negentropy V1, `src/lib/engine/negentropy.ts`): per
relay and stream filter (self `authors:[pk]`, each space `#h`; checkpoints
before changes) the relay events this device holds — the IndexedDB
`relay-events` store — are reconciled against the relay's set, and only the
missing ids are fetched with `REQ {ids}` and imported. History is complete once
every needed id came back and imported cleanly. Relays that answer `NEG-OPEN`
with `NEG-ERR`/`NOTICE` or silence get the older paged `until` walk.

```sh
npm run build:core   # requires Odin and sibling ../glonOdin
npm run verify:core
npm run check
npm run build
```

`build:core` emits `static/engine.wasm` and `static/engine-core.json`. Commit both
after changing the shared sources/runtime. The production build verifies their
hashes and needs no sibling checkout or Odin installation; the static artifacts
are deployable through the existing Docker/Fly pipeline. Do not hand-edit the
manifest to bypass verification.

Browser local changes enter a durable outbox before publication. Exact signed
retry events survive reload. Logout refuses unpublished work unless it is
explicitly exported. Clearing browser storage is not a safe way to discard an
outbox or reset the shared dataset.

## Object exchanges

An object's discussion pane shows exchanges from that object's mailbox. **New
exchange** selects object agents; selecting several creates a group by
delivering to each object's own history. **Reply to all** preserves the
audience; **Private** starts a separate exchange. Delivery and local processing
status are displayed separately. The shared exchange ID is a UI grouping key,
not a shared conversation object.

A Capability page is one catalog skill on one computer: its Status, Error,
Served by, Key and last Check are written by that computer's harness. In a tab
paired with that computer the Status popover stages Install / Check / Switch
off / Switch on / Uninstall as mailbox requests on the capability, and lists
its waiting requests with Approve / Reject; approval runs on that computer.
What the software is and how agents use it lives on its Skill object.

A Credential page (New → Credential) is one login for one service. Keys typed
there are saved on the credential itself (readable by everyone in its space);
Connect / Check / Disconnect run on the computer in its Served by, which opens
the Chrome sign-in window and keeps the status current. A Google account is a
credential with service `google-account` and the address as its Account:
Connect runs `gws auth login` there and signs in through the browser window gws
opens. Agents list the credentials they may use in their Credentials property.

## Verification

```sh
bun test scripts/core-abi.test.ts scripts/sync-authority.test.ts scripts/local-transport.test.ts scripts/negentropy.test.ts scripts/sync-negentropy.test.ts
bun run scripts/parity-codec.ts
bun run scripts/parity-replay-fixtures.ts
bun run scripts/parity-query-fixtures.ts
bun run scripts/parity-mutation-fixtures.ts
bun run scripts/parity-wire.ts
bun run scripts/parity-authority.ts
bun run scripts/parity-serving.ts
```

For a running local daemon, `scripts/parity-replay.ts` and `scripts/parity-query.ts`
compare the real vault using the local service credential. These operator tools
need the sibling harness source and respect `GLON_DATA` / `GLON_API`.

Deploy with `fly deploy --remote-only`. Default production mode is browser-offline;
local machine control always requires explicit pairing.
