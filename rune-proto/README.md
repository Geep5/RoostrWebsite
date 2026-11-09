# Roostr on Rune (prototype)

A look-and-feel prototype of Roostr on [Rune](https://github.com/anomalyco/rune), separate
from the GPUI app: a sidebar of spaces and their pins, a page as editable blocks, and a
query or collection as a sortable table.

It reads the local glon daemon (`~/.glon/api-token`, read-only; edits stay in memory) and
falls back to sample data when none answers.

```sh
cd rune-proto
cargo run --release                         # the local vault
cargo run --release -- space=matcherino open=publishers
cargo run --release -- sample               # the built-in sample
cargo run --release -- --snap page.png --size 1240x820 --theme dark   # render headless
cargo test --release
```

In a page, Return starts a new block (a list item continues the list), Backspace in an empty
block removes it, and a checkbox toggles. In a table, click a header to sort, drag its edge to
resize, and double-click a row to open it. ⇧⌘B hides the sidebar.

Not here: saving, agents and chat, query filters (a query shows every object of its types in
its space), covers, and images.
