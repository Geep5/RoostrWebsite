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

Browser protobuf/hash, replay, queries and ordinary mutation planning use the
same Odin sources as the native daemon, compiled to WebAssembly. IndexedDB,
network transport and DOM integration remain platform adapters.

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

## Verification

```sh
bun test scripts/core-abi.test.ts scripts/sync-authority.test.ts scripts/local-transport.test.ts
bun run scripts/parity-codec.ts
bun run scripts/parity-replay-fixtures.ts
bun run scripts/parity-query-fixtures.ts
bun run scripts/parity-mutation-fixtures.ts
```

For a running local daemon, `scripts/parity-replay.ts` and `scripts/parity-query.ts`
compare the real vault using the local service credential. These operator tools
need the sibling harness source and respect `GLON_DATA` / `GLON_API`.

Deploy with `fly deploy --remote-only`. Default production mode is browser-offline;
local machine control always requires explicit pairing.
