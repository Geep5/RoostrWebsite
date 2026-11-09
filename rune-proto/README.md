# Roostr on Rune (prototype)

A look-and-feel prototype of Roostr on [Rune](https://github.com/anomalyco/rune), separate
from the GPUI app: a sidebar of spaces and their pins, a page as editable blocks, and a
query or collection as Roostr's table: the view bar, property icons in the header, bold
iconed names, linked objects as pills, tags in their colours, checkboxes, the view's column
widths (sideways scroll when wider than the page), and its filters and sorts.

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
block removes it, and a checkbox toggles. In a table, click a header to sort (again to reverse)
and double-click a row to open it. ⇧⌘B hides the sidebar.

Not here: saving, agents and chat, the view bar's and add-column buttons (drawn, not wired),
column resizing, covers, and images.
