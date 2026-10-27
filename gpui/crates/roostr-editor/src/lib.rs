//! Block editor, file pages and file blocks: the native port of the web
//! app's `Editor.svelte` (with BlockNode, BlockMenu, SlashMenu, LinkPicker,
//! TableBlock, FileBlock, RelationBlock) and `File.svelte`.

/// The desktop block gutter (drag handle rail) left of every block. The
/// object page shifts a document left by it, so the text lines up under
/// the header's arrows and the handles hang in the margin.
pub const GUTTER_W: f32 = 48.;

mod block_menu;
mod codelint;
mod doc;
mod editor;
mod embed;
mod file_block;
mod file_view;
mod files;
mod keys;
mod link_picker;
mod look;
mod marks;
mod ops;
mod render;
mod slash_menu;
mod spell;
mod spellcheck;
mod text;
mod toggles;
mod write_queue;

use gpui_kit::{App, KeyBinding, PromptLevel, actions};
use roostr_core::ui::secondary_binding;

pub use editor::Editor;
pub use file_view::FileView;
pub use files::{CAN_PICK_FILES, PickedFile, file_image, pasted_image, pick_file};

actions!(
    roostr_editor,
    [
        Enter,
        ShiftEnter,
        Tab,
        ShiftTab,
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteLineLeft,
        DeleteWordRight,
        DeleteLineRight,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        LineStart,
        LineEnd,
        SelectLineStart,
        SelectLineEnd,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
        Up,
        Down,
        SelectUp,
        SelectDown,
        SelectAll,
        Bold,
        Italic,
        Underline,
        InlineCode,
        Escape,
        Undo,
        Redo,
        Copy,
        Cut,
        Paste,
        CharacterPalette,
    ]
);

const CONTEXT: &str = "RoostrEditor";

/// Register the editor's keybindings (the web editor's keydown handling).
pub fn init(cx: &mut App) {
    // New → File adds through the same upload as file blocks.
    cx.set_global(roostr_core::create::AddFile(std::rc::Rc::new(
        |name, mime, bytes, space_id, cx| {
            let upload = files::upload_file(files::PickedFile { name, mime, bytes }, space_id, cx);
            cx.spawn(async move |_| upload.await.map(|uploaded| uploaded.id))
        },
    )));
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("enter", Enter, c),
        KeyBinding::new("shift-enter", ShiftEnter, c),
        KeyBinding::new("tab", Tab, c),
        KeyBinding::new("shift-tab", ShiftTab, c),
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("alt-backspace", DeleteWordLeft, c),
        KeyBinding::new("alt-delete", DeleteWordRight, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("home", LineStart, c),
        KeyBinding::new("end", LineEnd, c),
        KeyBinding::new("shift-home", SelectLineStart, c),
        KeyBinding::new("shift-end", SelectLineEnd, c),
        KeyBinding::new("escape", Escape, c),
        secondary_binding("secondary-a", SelectAll, c, cx),
        secondary_binding("secondary-b", Bold, c, cx),
        secondary_binding("secondary-i", Italic, c, cx),
        secondary_binding("secondary-u", Underline, c, cx),
        secondary_binding("secondary-e", InlineCode, c, cx),
        secondary_binding("secondary-z", Undo, c, cx),
        secondary_binding("secondary-shift-z", Redo, c, cx),
        KeyBinding::new("ctrl-y", Redo, c),
        secondary_binding("secondary-c", Copy, c, cx),
        secondary_binding("secondary-x", Cut, c, cx),
        secondary_binding("secondary-v", Paste, c, cx),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("alt-left", WordLeft, c),
        KeyBinding::new("alt-right", WordRight, c),
        KeyBinding::new("shift-alt-left", SelectWordLeft, c),
        KeyBinding::new("shift-alt-right", SelectWordRight, c),
        KeyBinding::new("cmd-left", LineStart, c),
        KeyBinding::new("cmd-right", LineEnd, c),
        KeyBinding::new("shift-cmd-left", SelectLineStart, c),
        KeyBinding::new("shift-cmd-right", SelectLineEnd, c),
        KeyBinding::new("cmd-up", DocStart, c),
        KeyBinding::new("cmd-down", DocEnd, c),
        KeyBinding::new("shift-cmd-up", SelectDocStart, c),
        KeyBinding::new("shift-cmd-down", SelectDocEnd, c),
        KeyBinding::new("cmd-backspace", DeleteLineLeft, c),
        KeyBinding::new("cmd-delete", DeleteLineRight, c),
        KeyBinding::new("ctrl-a", LineStart, c),
        KeyBinding::new("ctrl-e", LineEnd, c),
        KeyBinding::new("ctrl-cmd-space", CharacterPalette, c),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-left", WordLeft, c),
        KeyBinding::new("ctrl-right", WordRight, c),
        KeyBinding::new("shift-ctrl-left", SelectWordLeft, c),
        KeyBinding::new("shift-ctrl-right", SelectWordRight, c),
        KeyBinding::new("ctrl-home", DocStart, c),
        KeyBinding::new("ctrl-end", DocEnd, c),
        KeyBinding::new("shift-ctrl-home", SelectDocStart, c),
        KeyBinding::new("shift-ctrl-end", SelectDocEnd, c),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, c),
        KeyBinding::new("ctrl-delete", DeleteWordRight, c),
    ]);
}

/// The web's `alert(text)`: a warning dialog on the active window.
pub(crate) fn alert(text: String, cx: &mut App) {
    let Some(window) = cx.active_window() else {
        log::warn!("{text}");
        return;
    };
    let _ = window.update(cx, |_, window, cx| {
        // The answer is not needed: the dialog only informs.
        drop(window.prompt(PromptLevel::Warning, &text, None, &["OK"], cx));
    });
}
