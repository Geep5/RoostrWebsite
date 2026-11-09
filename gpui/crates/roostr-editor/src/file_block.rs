//! A File object shown inside a page (`FileBlock.svelte`): images inline,
//! PDFs as a card that opens the system viewer, the caption linking to the
//! File. The bytes come from Blossom when the File names a blob, else from
//! this computer's harness. Images never grow past the column and resize by
//! their bottom-right corner; the width (percent of the column) is kept on
//! the block as `width`.
//!
//! The editor keeps one [`FileBlock`] per file block in `Editor::file_blocks`
//! ([`Editor::sync_file_blocks`]); it holds the loaded bytes and the resize.

use std::collections::{BTreeMap, HashSet};
use std::io::Cursor;
use std::sync::Arc;

use roostr_core::State;
use roostr_core::nav::{Route, navigate};
use roostr_core::types::BlockContent;
use roostr_rt::Task;
use roostr_ui::{Alert, Button, Glyph, Images, KitSize, Theme, motion, ty};
use rune::{Drag, El, Phase, SurfaceFit, Tx, col, el, row, text};

use crate::editor::{Editor, custom_content, update_editor, with_editor};
use crate::file_view::{drawable_image, loading_card, open_card, window_height};
use crate::files;

const MIN_WIDTH: f32 = 10.;

/// The column width before the block's first layout.
const FALLBACK_COLUMN: f32 = 640.;

/// One file block's loaded bytes and resize state.
pub(crate) struct FileBlock {
    meta: BTreeMap<String, String>,
    read_only: bool,
    /// The decoded image is in [`Images`] under this uri.
    image: Option<String>,
    dims: Option<(u32, u32)>,
    bytes: Option<Arc<Vec<u8>>>,
    error: String,
    attempt: u64,
    loaded_for: String,
    /// The width while a drag is in progress; `None` = the saved one.
    drag_width: Option<f32>,
    /// (image width, column width) at drag start.
    drag: Option<(f32, f32)>,
    _load: Option<Task<()>>,
}

impl FileBlock {
    fn new(meta: BTreeMap<String, String>, read_only: bool) -> Self {
        Self {
            meta,
            read_only,
            image: None,
            dims: None,
            bytes: None,
            error: String::new(),
            attempt: 0,
            loaded_for: String::new(),
            drag_width: None,
            drag: None,
            _load: None,
        }
    }

    fn mime(&self) -> &str {
        self.meta.get("mime").map(String::as_str).unwrap_or("")
    }

    fn inline(&self) -> bool {
        self.mime().starts_with("image/") || self.mime() == "application/pdf"
    }

    fn name(&self, s: &State) -> String {
        let id = self.meta.get("fileId").cloned().unwrap_or_default();
        s.store
            .summary(&id)
            .map(|s| s.name.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| self.meta.get("name").cloned().filter(|n| !n.is_empty()))
            .unwrap_or_else(|| "File".into())
    }

    fn saved_width(&self) -> Option<f32> {
        let w: f32 = self
            .meta
            .get("width")
            .and_then(|w| w.parse().ok())
            .unwrap_or(0.);
        (w > 0.).then(|| w.clamp(MIN_WIDTH, 100.))
    }

    /// Fetch the bytes of an inline file (again after a retry or a new hash).
    fn load(&mut self, object_id: &str, block_id: &str, s: &mut State) {
        let hash = self.meta.get("hash").cloned().unwrap_or_default();
        let key = format!(
            "{}|{}|{}",
            hash,
            self.meta.get("fileId").cloned().unwrap_or_default(),
            self.attempt
        );
        if !self.inline() || hash.is_empty() || key == self.loaded_for {
            return;
        }
        self.loaded_for = key;
        self.error.clear();
        self.image = None;
        self.bytes = None;
        let blob = files::file_block_blob(&self.meta, s);
        let mime = self.mime().to_string();
        let uri = format!("roostr-file-hash:{hash}");
        let (oid, bid) = (object_id.to_string(), block_id.to_string());
        self._load = Some(s.rt.spawn(async move |cx| {
            let result = blob.await;
            let decoded = cx
                .background(async move {
                    let (bytes, fetched) = result?;
                    let kind = if mime.is_empty() { fetched } else { mime };
                    let dims = shown_dimensions(&bytes);
                    anyhow::Ok((bytes, kind, dims))
                })
                .await;
            update_editor(&cx, &oid, |e, s| {
                let Some(fb) = e.file_blocks.get_mut(&bid) else {
                    return;
                };
                match decoded {
                    Ok((bytes, kind, dims)) => {
                        if drawable_image(&kind)
                            && let Some(images) = s.get::<Images>()
                        {
                            images.insert(&uri, bytes.clone());
                            fb.image = Some(uri);
                        }
                        fb.dims = dims;
                        fb.bytes = Some(Arc::new(bytes));
                    }
                    Err(err) => fb.error = files::view_failure_text(&err),
                }
            });
        }));
    }
}

impl Editor {
    /// Keep one [`FileBlock`] per file block (they hold the loaded bytes):
    /// new blocks start loading, changed ones load again, gone ones drop.
    pub(crate) fn sync_file_blocks(&mut self, s: &mut State) {
        let mut live = HashSet::new();
        let read_only = self.read_only;
        for b in &self.blocks {
            let Some(custom) = &b.content.custom else {
                continue;
            };
            if custom.content_type != "file" {
                continue;
            }
            live.insert(b.id.clone());
            let meta = custom.meta.clone().unwrap_or_default();
            match self.file_blocks.get_mut(&b.id) {
                Some(fb) => {
                    fb.read_only = read_only;
                    if meta != fb.meta {
                        fb.meta = meta;
                        fb.load(&self.object_id, &b.id, s);
                    }
                }
                None => {
                    let mut fb = FileBlock::new(meta, read_only);
                    fb.load(&self.object_id, &b.id, s);
                    self.file_blocks.insert(b.id.clone(), fb);
                }
            }
        }
        self.file_blocks.retain(|id, _| live.contains(id));
    }

    /// Put an image block on the clipboard (`copyImageBlock`): its loaded
    /// bytes, as PNG.
    pub(crate) fn copy_image_block(&self, block_id: &str, tx: &mut Tx) -> anyhow::Result<()> {
        let bytes = self
            .file_blocks
            .get(block_id)
            .and_then(|fb| fb.bytes.clone())
            .ok_or_else(|| anyhow::anyhow!("the image hasn't loaded yet"))?;
        files::copy_image(&bytes, tx)
    }

    /// The resize handle's gesture: `width` is the image's drawn width and
    /// `column` the column's at the press.
    fn file_resize_drag(
        &mut self,
        block_id: &str,
        d: &Drag,
        width: f32,
        column: f32,
        s: &mut State,
    ) {
        let Some(fb) = self.file_blocks.get_mut(block_id) else {
            return;
        };
        match d.phase {
            Phase::Down => fb.drag = Some((width, column)),
            Phase::Move => {
                if let Some((start, col)) = fb.drag {
                    let pct = ((start + d.dx) / col.max(1.) * 100.)
                        .round()
                        .clamp(MIN_WIDTH, 100.);
                    fb.drag_width = Some(pct);
                }
            }
            Phase::Up => {
                if d.cancelled() {
                    fb.drag = None;
                    fb.drag_width = None;
                } else if fb.drag.is_some() {
                    self.end_file_resize(block_id, s);
                }
            }
        }
    }

    /// Save the dragged width on the block, then show the saved one.
    fn end_file_resize(&mut self, block_id: &str, s: &mut State) {
        let Some(fb) = self.file_blocks.get_mut(block_id) else {
            return;
        };
        fb.drag = None;
        let Some(width) = fb.drag_width else { return };
        if Some(width) == fb.saved_width() {
            fb.drag_width = None;
            return;
        }
        let mut meta = fb.meta.clone();
        meta.insert("width".into(), format!("{width}"));
        let refs: Vec<(&str, &str)> = meta.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let content: BlockContent = custom_content("file", &refs);
        let task = self.w_block_update(block_id, content, s);
        let (oid, bid) = (self.object_id.clone(), block_id.to_string());
        s.rt.spawn(async move |cx| {
            let _ = task.await;
            let Some(refresh) = update_editor(&cx, &oid, |e, s| e.refresh(s)) else {
                return;
            };
            refresh.await;
            update_editor(&cx, &oid, |e, _| {
                if let Some(fb) = e.file_blocks.get_mut(&bid) {
                    fb.drag_width = None;
                }
            });
        })
        .detach();
    }
}

/// A file block's body (without the gutter/row the editor draws around it).
pub(crate) fn file_block_view(
    s: &State,
    cx: &mut rune::Cx<State>,
    t: &Theme,
    e: &Editor,
    block_id: &str,
    _layer: &mut Vec<El>,
) -> El {
    let root_key = format!("file-{block_id}");
    let Some(fb) = e.file_blocks.get(block_id) else {
        return col().key(root_key).w_full().min_w(0.);
    };
    let name = fb.name(s);
    let inline = fb.inline();
    let mime = fb.mime().to_string();
    let file_id = fb.meta.get("fileId").cloned().unwrap_or_default();
    let oid = e.object_id.clone();
    let column = cx
        .layout_size(&root_key)
        .map(|[w, _]| w)
        .filter(|w| *w > 0.)
        .unwrap_or(FALLBACK_COLUMN);
    let mut root = col().key(root_key).gap(6.).w_full().min_w(0.);
    if inline && !fb.error.is_empty() {
        let (oid, bid) = (oid.clone(), block_id.to_string());
        root = root.child(
            row()
                .items_center()
                .gap(6.)
                .child(
                    el().grow(1.).shrink().min_w(0.).child(
                        Alert::error(format!("file-error-{block_id}"), fb.error.clone())
                            .size(KitSize::XSmall)
                            .text_size(13.)
                            .build(t),
                    ),
                )
                .child(
                    Button::new(format!("file-retry-{block_id}"))
                        .label("Retry")
                        .outline()
                        .xsmall()
                        .on_press(move |s: &mut State| {
                            with_editor(s, &oid, |e, s| {
                                let oid = e.object_id.clone();
                                if let Some(fb) = e.file_blocks.get_mut(&bid) {
                                    fb.attempt += 1;
                                    fb.load(&oid, &bid, s);
                                }
                            });
                        })
                        .build(t),
                ),
        );
    } else if inline && fb.bytes.is_none() {
        root = root.child(loading_card(
            format!("file-loading-{block_id}"),
            format!("Loading {name}…"),
            8.,
            t.panel,
            t,
        ));
    } else if mime.starts_with("image/") {
        let image = fb
            .image
            .as_deref()
            .and_then(|uri| s.get::<Images>()?.image(uri, column));
        if let Some(image) = image {
            let (w, h) = fb
                .dims
                .map(|(w, h)| (w as f32, h as f32))
                .unwrap_or((column, column * 0.6));
            let pct = fb.drag_width.or(fb.saved_width());
            let (dw, dh) = match pct {
                Some(p) => {
                    let dw = column * p / 100.;
                    (dw, dw * h / w.max(1.))
                }
                None => {
                    let mut dw = w.min(column);
                    let mut dh = dw * h / w.max(1.);
                    let max_h = window_height(cx) * 0.7;
                    if dh > max_h {
                        dh = max_h;
                        dw = dh * w / h.max(1.);
                    }
                    (dw, dh)
                }
            };
            let resizing = fb.drag_width.is_some();
            // `.img-frame:hover .resize` (`transition: opacity 0.12s`); a
            // resize keeps it shown.
            let mut frame = el()
                .key(format!("img-frame-{block_id}"))
                .size(dw, dh)
                .child(
                    el().w_full()
                        .h_full()
                        .radius(8.)
                        .pixels_fit(image.pixels(), SurfaceFit::Fit),
                );
            if resizing {
                frame = frame.child(
                    el().pin(Some(-4.), Some(-4.), Some(-4.), Some(-4.))
                        .border(2., t.accent)
                        .radius(8.),
                );
            }
            if !fb.read_only {
                let (oid, bid) = (oid.clone(), block_id.to_string());
                let mut handle = el()
                    .key(format!("img-resize-{block_id}"))
                    .pin(None, None, Some(-5.), Some(-5.))
                    .size(14., 14.)
                    .radius(7.)
                    .border(2., t.bg)
                    .fill(t.accent)
                    .cursor(rune::Cursor::ResizeColumn)
                    .on_drag(move |s: &mut State, d: &mut Drag, _: &mut Tx| {
                        with_editor(s, &oid, |e, s| e.file_resize_drag(&bid, d, dw, column, s));
                    });
                handle = if resizing {
                    handle.opacity(1.)
                } else {
                    handle.group_opacity(0., 1.)
                };
                frame = frame
                    .hover_group()
                    .hover_motion(motion::FADE, motion::FADE)
                    .child(handle);
            }
            root = root.child(frame);
        }
    } else if mime == "application/pdf" {
        let bytes = fb.bytes.clone();
        let open_name = name.clone();
        root = root.child(
            open_card(
                format!("pdf-card-{block_id}"),
                Glyph::FileText,
                name.clone(),
                Some("PDF · Open in the system viewer"),
                8.,
                t,
            )
            .on_press(move |s: &mut State| {
                if let Some(bytes) = bytes.clone() {
                    files::open_externally(bytes, &open_name, s).detach_and_log_err(&s.rt);
                }
            }),
        );
    }
    let caption = format!("📎 {name}");
    root.child(
        el().self_start().max_w(column).child(
            Button::new(format!("file-caption-{block_id}"))
                .text()
                .accessibility_label(caption.clone())
                .child(text(caption).font(ty::ui(12.)).color(t.muted).truncate())
                .text_color(t.muted)
                .on_press(move |s: &mut State| navigate(Route::Object(file_id.clone()), s))
                .build(t),
        ),
    )
}

/// The size an image shows at: its pixel size turned by its EXIF
/// orientation, as Rune draws it (and a browser's `<img>` did). A phone
/// stores a portrait photo sideways, so its pixel size alone would lay it
/// out landscape and the drawn image would spill over the blocks below.
fn shown_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    use image::ImageDecoder as _;
    use image::metadata::Orientation;
    let mut decoder = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let (width, height) = decoder.dimensions();
    let turned = matches!(
        decoder.orientation(),
        Ok(Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH)
    );
    Some(if turned {
        (height, width)
    } else {
        (width, height)
    })
}

#[cfg(test)]
mod tests {
    use super::shown_dimensions;

    /// A 4×2 JPEG, with an EXIF orientation when `orientation` is given.
    fn jpeg(orientation: Option<u8>) -> Vec<u8> {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut out)
            .encode(&[128; 4 * 2 * 3], 4, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        let Some(orientation) = orientation else {
            return out;
        };
        // APP1 "Exif": a big-endian TIFF header and one IFD entry, 0x0112 SHORT.
        let mut app1 = vec![0xFF, 0xE1, 0x00, 0x22];
        app1.extend_from_slice(b"Exif\0\0MM\0\x2A\0\0\0\x08\0\x01");
        app1.extend_from_slice(&[0x01, 0x12, 0x00, 0x03, 0, 0, 0, 1, 0, orientation, 0, 0]);
        app1.extend_from_slice(&[0, 0, 0, 0]);
        out.splice(2..2, app1);
        out
    }

    #[test]
    fn a_sideways_stored_photo_shows_portrait() {
        assert_eq!(shown_dimensions(&jpeg(None)), Some((4, 2)));
        // The camera's portrait photo: stored landscape, shown turned a quarter.
        assert_eq!(shown_dimensions(&jpeg(Some(6))), Some((2, 4)));
        assert_eq!(shown_dimensions(&jpeg(Some(8))), Some((2, 4)));
        // Upside down keeps its shape.
        assert_eq!(shown_dimensions(&jpeg(Some(3))), Some((4, 2)));
        assert_eq!(shown_dimensions(b"not an image"), None);
    }
}
