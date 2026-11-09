//! Block editor, file pages and file blocks: the native port of the web
//! app's `Editor.svelte` (with BlockNode, BlockMenu, SlashMenu, LinkPicker,
//! TableBlock, FileBlock, RelationBlock) and `File.svelte`, drawn with Rune.
//!
//! The app calls [`register`] once, [`open`] / [`close`] as object pages
//! come and go, and draws an open body with [`view`]; File objects use
//! [`open_file`] / [`file_view`]. See `local://roostr-editor-api.md`.

/// The desktop block gutter (drag handle rail) left of every block. The
/// object page shifts a document left by it, so the text lines up under
/// the header's arrows and the handles hang in the margin.
pub const GUTTER_W: f32 = 48.;

mod block_menu;
mod codelint;
mod doc;
mod editor;
mod embed;
mod embed_view;
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
#[cfg(test)]
mod tests;
mod text;
mod toggles;
mod write_queue;

use std::rc::Rc;

use roostr_core::State;
use roostr_core::create::AddFile;

pub use editor::Editors;
pub use file_view::{FileViews, close_file, file_view, open_file};
pub use files::{
    CAN_PICK_FILES, PickedFile, Uploaded, file_image, pasted_image, pasted_png, pick_file,
    upload_file,
};
pub use render::view;

/// Set the editor up for the app: re-read open bodies when the store says
/// they changed, follow the keyboard between blocks, make picked files into
/// File objects (`s.add_file`), and start loading the spelling dictionary.
pub fn register(s: &mut State) {
    s.on_store(editor::on_store);
    s.on_poll(keys::on_poll);
    s.add_file = Some(AddFile(Rc::new(|name, mime, bytes, space, s| {
        let upload = upload_file(PickedFile { name, mime, bytes }, space, s);
        s.rt.spawn(async move |_| Ok(upload.await?.id))
    })));
    spellcheck::ensure_loaded(s);
}

/// Open `object_id`'s body (once; later calls keep the open one): it loads
/// and [`view`] draws it. `read_only` shows the body and never edits it.
pub fn open(s: &mut State, object_id: &str, read_only: bool) {
    if is_open(s, object_id) {
        return;
    }
    let editor = editor::Editor::new(object_id.to_string(), read_only, s);
    s.slot::<Editors>()
        .map
        .insert(object_id.to_string(), editor);
}

/// Whether `object_id`'s body is open.
pub fn is_open(s: &State, object_id: &str) -> bool {
    editor::editor(s, object_id).is_some()
}

/// Close `object_id`'s body: unsaved typing is saved, queued writes still land.
pub fn close(s: &mut State, object_id: &str) {
    if let Some(editor) = s.slot::<Editors>().map.remove(object_id) {
        editor.close(s);
    }
}

/// Put the caret at the start of `object_id`'s body (Enter in the page
/// title): its first line when that is empty, else a new line above it.
pub fn focus_start(s: &mut State, object_id: &str) {
    editor::with_editor(s, object_id, |e, s| e.focus_start(s));
}
