//! A File object shown inside a page (`FileBlock.svelte`): images inline,
//! PDFs as a card that opens the system viewer (GPUI has no PDF view), the
//! caption linking to the File. The bytes come from Blossom when the File
//! names a blob, else from this computer's harness. Images never grow past
//! the column and resize by their bottom-right corner; the width (percent
//! of the column) is kept on the block as `width`.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use gpui_kit::component::alert::Alert;
use gpui_kit::component::attachment::{
    Attachment, AttachmentContent, AttachmentDescription, AttachmentMedia, AttachmentStatus,
    AttachmentTitle,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::motion::{self, HoverMotion};
use roostr_core::types::BlockContent;
use roostr_core::{Route, Store, theme};

use crate::editor::{Editor, custom_content};

const MIN_WIDTH: f32 = 10.;

pub struct FileBlock {
    block_id: String,
    meta: BTreeMap<String, String>,
    read_only: bool,
    editor: WeakEntity<Editor>,
    image: Option<Arc<Image>>,
    dims: Option<(u32, u32)>,
    bytes: Option<Arc<Vec<u8>>>,
    error: String,
    attempt: u64,
    loaded_for: String,
    /// The width while a drag is in progress; `None` = the saved one.
    drag_width: Option<f32>,
    /// (frame left, column width) at drag start.
    drag: Option<(Pixels, Pixels)>,
    column: Pixels,
    _load: Option<Task<()>>,
}

impl FileBlock {
    pub fn new(
        block_id: String,
        meta: BTreeMap<String, String>,
        read_only: bool,
        editor: WeakEntity<Editor>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            block_id,
            meta,
            read_only,
            editor,
            image: None,
            dims: None,
            bytes: None,
            error: String::new(),
            attempt: 0,
            loaded_for: String::new(),
            drag_width: None,
            drag: None,
            column: px(0.),
            _load: None,
        };
        this.load(cx);
        this
    }

    pub fn set_meta(
        &mut self,
        meta: BTreeMap<String, String>,
        read_only: bool,
        cx: &mut Context<Self>,
    ) {
        self.read_only = read_only;
        if meta != self.meta {
            self.meta = meta;
            self.load(cx);
            cx.notify();
        }
    }

    fn mime(&self) -> &str {
        self.meta.get("mime").map(String::as_str).unwrap_or("")
    }

    fn inline(&self) -> bool {
        self.mime().starts_with("image/") || self.mime() == "application/pdf"
    }

    fn name(&self, cx: &App) -> String {
        let id = self.meta.get("fileId").cloned().unwrap_or_default();
        Store::global(cx)
            .read(cx)
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

    fn load(&mut self, cx: &mut Context<Self>) {
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
        let blob = crate::files::file_block_blob(&self.meta, cx);
        let mime = self.mime().to_string();
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = blob.await;
            let decoded = cx
                .background_spawn(async move {
                    let (bytes, fetched) = result?;
                    let kind = if mime.is_empty() { fetched } else { mime };
                    let dims = shown_dimensions(&bytes);
                    anyhow::Ok((bytes, kind, dims))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match decoded {
                    Ok((bytes, kind, dims)) => {
                        if let Some(format) = ImageFormat::from_mime_type(&kind) {
                            this.image = Some(Arc::new(Image::from_bytes(format, bytes.clone())));
                        }
                        this.dims = dims;
                        this.bytes = Some(Arc::new(bytes));
                    }
                    Err(err) => this.error = crate::files::view_failure_text(&err),
                }
                cx.notify();
            });
        }));
    }

    fn end_resize(&mut self, cx: &mut Context<Self>) {
        self.drag = None;
        let Some(width) = self.drag_width else { return };
        if Some(width) == self.saved_width() {
            self.drag_width = None;
            cx.notify();
            return;
        }
        let mut meta = self.meta.clone();
        meta.insert("width".into(), format!("{width}"));
        let pairs: Vec<(String, String)> = meta.into_iter().collect();
        let refs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let content: BlockContent = custom_content("file", &refs);
        let block = self.block_id.clone();
        let editor = self.editor.clone();
        cx.spawn(async move |this, cx| {
            let Ok(task) = editor.update(cx, |e, cx| e.w_block_update(&block, content, cx)) else {
                return;
            };
            let _ = task.await;
            let Ok(refresh) = editor.update(cx, |e, cx| e.refresh(cx)) else {
                return;
            };
            refresh.await;
            let _ = this.update(cx, |this, cx| {
                this.drag_width = None;
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for FileBlock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.name(cx);
        let inline = self.inline();
        let mime = self.mime().to_string();
        let file_id = self.meta.get("fileId").cloned().unwrap_or_default();
        let entity = cx.entity();
        let column_cell = entity.clone();
        let mut root = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .w_full()
            .min_w_0()
            .relative()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        column_cell.update(cx, |this, cx| {
                            if (this.column - bounds.size.width).abs() > px(0.5) {
                                this.column = bounds.size.width;
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        if inline && !self.error.is_empty() {
            root = root.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        Alert::error("file-error", self.error.clone())
                            .xsmall()
                            .text_size(px(13.))
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        Button::new("file-retry")
                            .label("Retry")
                            .outline()
                            .xsmall()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.attempt += 1;
                                this.load(cx);
                                cx.notify();
                            })),
                    ),
            );
        } else if inline && self.bytes.is_none() {
            root = root.child(
                Attachment::new()
                    .status(AttachmentStatus::Processing)
                    .media(AttachmentMedia::new())
                    .content(
                        AttachmentContent::new().title(
                            AttachmentTitle::new(format!("Loading {name}…"))
                                .font_weight(FontWeight::NORMAL)
                                .text_color(theme::muted()),
                        ),
                    )
                    .w_full()
                    .text_size(px(13.))
                    .rounded(px(8.))
                    .bg(theme::panel())
                    .border_color(theme::border()),
            );
        } else if mime.starts_with("image/") {
            if let Some(image) = self.image.clone() {
                let column = if self.column > px(0.) {
                    self.column
                } else {
                    px(640.)
                };
                let (w, h) = self
                    .dims
                    .map(|(w, h)| (w as f32, h as f32))
                    .unwrap_or((column.into(), f32::from(column) * 0.6));
                let pct = self.drag_width.or(self.saved_width());
                let (dw, dh) = match pct {
                    Some(p) => {
                        let dw = f32::from(column) * p / 100.;
                        (dw, dw * h / w.max(1.))
                    }
                    None => {
                        let mut dw = w.min(column.into());
                        let mut dh = dw * h / w.max(1.);
                        let max_h = f32::from(window.viewport_size().height) * 0.7;
                        if dh > max_h {
                            dh = max_h;
                            dw = dh * w / h.max(1.);
                        }
                        (dw, dh)
                    }
                };
                let resizing = self.drag_width.is_some();
                let drag_entity = entity.clone();
                // `.img-frame:hover .resize` (`transition: opacity 0.12s`); a
                // resize keeps it shown.
                let (reveal, hover) = if self.read_only {
                    (0., None)
                } else {
                    let (t, state) = motion::hover("img-resize-reveal", motion::FADE, window, cx);
                    (t, Some(state))
                };
                root = root.child(
                    div()
                        .id("img-frame")
                        .when_some(hover, |d, state| {
                            d.on_hover(move |hovered: &bool, _, cx| {
                                HoverMotion::set(&state, *hovered, cx)
                            })
                        })
                        .relative()
                        .w(px(dw))
                        .h(px(dh))
                        .child(img(ImageSource::Image(image)).size_full().rounded(px(8.)))
                        .when(resizing, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .top(px(-4.))
                                    .left(px(-4.))
                                    .right(px(-4.))
                                    .bottom(px(-4.))
                                    .border_2()
                                    .border_color(theme::accent())
                                    .rounded(px(8.)),
                            )
                        })
                        .when(!self.read_only, |d| {
                            d.child(
                                div()
                                    .id("img-resize")
                                    .absolute()
                                    .right(px(-5.))
                                    .bottom(px(-5.))
                                    .size(px(14.))
                                    .rounded_full()
                                    .border_2()
                                    .border_color(theme::bg())
                                    .bg(theme::accent())
                                    .cursor(CursorStyle::ResizeUpLeftDownRight)
                                    .opacity(if resizing { 1. } else { reveal })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            let left = ev.position.x - px(dw);
                                            this.drag = Some((left, column));
                                            cx.notify();
                                        }),
                                    ),
                            )
                        })
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |_, _, window, _| {
                                    // Registered every frame: a press and release can land
                                    // before the next paint.
                                    let e = drag_entity.clone();
                                    window.on_mouse_event(move |ev: &MouseMoveEvent, _, _, cx| {
                                        e.update(cx, |this, cx| {
                                            if let Some((left, col)) = this.drag {
                                                let pct = ((ev.position.x - left) / col * 100.)
                                                    .round()
                                                    .clamp(MIN_WIDTH, 100.);
                                                this.drag_width = Some(pct);
                                                cx.notify();
                                            }
                                        });
                                    });
                                    let e = drag_entity.clone();
                                    window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                                        e.update(cx, |this, cx| {
                                            if this.drag.is_some() {
                                                this.end_resize(cx);
                                            }
                                        });
                                    });
                                },
                            )
                            .absolute()
                            .size_0(),
                        ),
                );
            }
        } else if mime == "application/pdf" {
            let bytes = self.bytes.clone();
            let open_name = name.clone();
            root = root.child(
                // The press must not reach the editor (block selection/drag).
                div()
                    .w_full()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Attachment::new()
                            .id("pdf-card")
                            .on_click(move |_, _, cx| {
                                if let Some(bytes) = bytes.clone() {
                                    crate::files::open_externally(bytes, &open_name, cx)
                                        .detach_and_log_err(cx);
                                }
                            })
                            .media(AttachmentMedia::new().child(Icon::new(IconName::FileText)))
                            .content(
                                AttachmentContent::new()
                                    .title(
                                        AttachmentTitle::new(name.clone())
                                            .font_weight(FontWeight::NORMAL),
                                    )
                                    .description(
                                        AttachmentDescription::new(
                                            "PDF · Open in the system viewer",
                                        )
                                        .text_size(px(11.)),
                                    ),
                            )
                            .w_full()
                            .h_auto()
                            .gap(px(10.))
                            .px(px(14.))
                            .py(px(12.))
                            .rounded(px(8.))
                            .text_size(px(13.)),
                    ),
            );
        }
        root.child(
            Button::new("file-caption")
                .text()
                .accessibility_label(format!("📎 {name}"))
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .text_size(px(12.))
                        .child(format!("📎 {name}")),
                )
                .text_color(theme::muted())
                .self_start()
                .max_w_full()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    roostr_core::nav::navigate(Route::Object(file_id.clone()), cx)
                }),
        )
    }
}

/// The size an image shows at: its pixel size turned by its EXIF
/// orientation, as GPUI draws it (and a browser's `<img>` did). A phone
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
