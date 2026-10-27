//! A page's cover ([`roostr_core::cover`]): the banner above the icon and
//! title (the icon overlaps its bottom edge), its actions - desktop: on
//! hover; phone: the ⋯ menu - Reposition, the image chooser behind Add
//! cover / Change cover, and the generate panel that renders the page's
//! `cover_generation` session.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::cover::{self, Action, CoverAction, Generation, OpenAiCredential, Status};
use roostr_core::create::{AddFile, add_failure_text};
use roostr_core::nav::{Route, navigate};
use roostr_core::types::Object;
use roostr_core::{Glyph, StoreEvent, theme, ui};
use roostr_editor::PickedFile;

use super::ObjectPage;

/// Banner heights: desktop, phone.
const BANNER_H: f32 = 200.;
const BANNER_H_PHONE: f32 = 140.;

/// How far the page icon reaches up into the cover: desktop, phone.
pub(super) const ICON_OVERLAP: f32 = 44.;
pub(super) const ICON_OVERLAP_PHONE: f32 = 30.;

const WRITING: &str = "Writing the idea…";
const PAINTING: &str = "Painting… (~30 s)";
const WAITING: &str = "Waiting for the computer that serves this space's OpenAI credential…";

/// The cover's state on one page.
#[derive(Default)]
pub(super) struct CoverUi {
    /// The banner's File and its image (`None` while loading or unreadable).
    banner: Option<(String, Option<Arc<Image>>)>,
    /// Generated tries' images by File id (`None` while loading or unreadable).
    thumbs: HashMap<String, Option<Arc<Image>>>,
    /// The pointer is over the icon and title (desktop: shows Add cover).
    head_hovered: bool,
    /// The pointer is over the banner (desktop: shows its actions).
    banner_hovered: bool,
    reposition: Option<Reposition>,
    chooser: Option<Chooser>,
    panel: Option<Panel>,
    /// The session the person closed, until its fields are gone: a session
    /// that is still (or again) on the page opens the panel by itself.
    dismissed: Option<i64>,
    /// Where the banner was painted and its image's size, for dragging.
    geometry: Rc<Cell<Geometry>>,
}

#[derive(Clone, Copy, Default)]
struct Geometry {
    bounds: Bounds<Pixels>,
    image: Size<f32>,
}

impl Geometry {
    /// How much taller the image is than the banner once it fills its width.
    fn overflow(self) -> f32 {
        if self.image.width <= 0. || self.image.height <= 0. {
            return 0.;
        }
        let shown = f32::from(self.bounds.size.width) * self.image.height / self.image.width;
        (shown - f32::from(self.bounds.size.height)).max(0.)
    }
}

/// Reposition: the focus being dragged, unsaved.
struct Reposition {
    y: f32,
    /// Pointer y where the drag began, and the focus then.
    drag: Option<(Pixels, f32)>,
}

/// Add cover / Change cover: choose, paste or generate an image.
#[derive(Default)]
struct Chooser {
    adding: bool,
    error: String,
}

/// The generate panel.
struct Panel {
    prompt: Entity<TextareaState>,
    /// What the box was last filled with: the person edited it when it
    /// holds anything else.
    synced: String,
    /// The session prompt last shown.
    shown_prompt: String,
    /// The try previewed (and used by Use this).
    selected: Option<String>,
    /// How many tries the session had last time: a new one is selected.
    seen_tries: usize,
    key: KeyState,
    error: String,
}

/// The space's OpenAI credential, as far as the panel knows.
#[derive(Clone, PartialEq)]
enum KeyState {
    Checking,
    /// No OpenAI credential in the space.
    Missing,
    /// The credential, and whether its key is filled in.
    Found(OpenAiCredential),
}

/// The banner image positioned like CSS `object-fit: cover` with
/// `object-position: 50% (y * 100)%`.
fn cover_bounds(bounds: Bounds<Pixels>, image: Size<f32>, y: f32) -> Bounds<Pixels> {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let scale = (w / image.width).max(h / image.height);
    let size = size(px(image.width * scale), px(image.height * scale));
    Bounds {
        origin: point(
            bounds.origin.x + (bounds.size.width - size.width) / 2.,
            bounds.origin.y + (bounds.size.height - size.height) * y,
        ),
        size,
    }
}

/// The session's `at`: the newest of its answer and its pending ask.
fn session_at(object: &Object) -> Option<i64> {
    let generation = cover::generation(&object.fields).map(|g| g.at);
    let request = cover::pending_request(&object.fields);
    generation.into_iter().chain(request).max()
}

/// A button laid over the cover image: light, so it reads on any picture.
fn pill(id: &'static str, label: &'static str, cx: &App) -> Button {
    Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(hsla(0., 0., 1., 0.88))
                .foreground(hsla(0., 0., 0.15, 1.))
                .hover(hsla(0., 0., 1., 1.))
                .active(hsla(0., 0., 0.94, 1.)),
        )
        .rounded(px(6.))
        .h(px(26.))
        .px(px(9.))
        .accessibility_label(label)
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
}

/// `button` findable by `id` (tests, the inspector).
fn tagged(id: &'static str, button: impl IntoElement) -> Div {
    div().debug_selector(move || id.into()).child(button)
}

impl ObjectPage {
    /// Bring the cover in line with the page: its image, the panel's session.
    pub(super) fn sync_cover(
        &mut self,
        object: &Object,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let file = cover::can_have_cover(object)
            .then(|| cover::cover_file(&object.fields))
            .flatten();
        if file.is_none() {
            self.cover.reposition = None;
        }
        if self.cover.banner.as_ref().map(|(f, _)| f) != file.as_ref() {
            self.cover.banner = file.clone().map(|f| (f, None));
            if let Some(file) = file {
                let load = roostr_editor::file_image(&file, cx);
                cx.spawn(async move |this, cx| {
                    let image = load.await.ok();
                    let _ = this.update(cx, |this, cx| {
                        if let Some((shown, slot)) = &mut this.cover.banner
                            && *shown == file
                        {
                            *slot = image;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        let session = cover::can_have_cover(object)
            .then(|| session_at(object))
            .flatten();
        if session.is_none() {
            self.cover.dismissed = None;
        } else if self.cover.panel.is_none() && session != self.cover.dismissed {
            self.open_generate(window, cx);
        }
        let Some(panel) = &mut self.cover.panel else {
            return;
        };
        let generation = cover::generation(&object.fields).unwrap_or_default();
        // A new prompt from the art director fills the box unless the person is editing it.
        if generation.prompt != panel.shown_prompt {
            let text = panel.prompt.read(cx).value().to_string();
            if text == panel.synced || text == generation.prompt {
                panel.prompt.update(cx, |p, cx| {
                    p.set_value(generation.prompt.clone(), window, cx)
                });
                panel.synced = generation.prompt.clone();
            }
            panel.shown_prompt = generation.prompt.clone();
        }
        if generation.tries.len() != panel.seen_tries {
            panel.seen_tries = generation.tries.len();
            panel.selected = generation.tries.last().map(|t| t.file.clone());
        }
        let missing: Vec<String> = generation
            .tries
            .iter()
            .map(|t| t.file.clone())
            .filter(|f| !self.cover.thumbs.contains_key(f))
            .collect();
        for file in missing {
            self.cover.thumbs.insert(file.clone(), None);
            let load = roostr_editor::file_image(&file, cx);
            cx.spawn(async move |this, cx| {
                let image = load.await.ok();
                let _ = this.update(cx, |this, cx| {
                    this.cover.thumbs.insert(file, image);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// A key pasted into the credential (or a refresh) can lift the needs-key state.
    pub(super) fn cover_store_event(&mut self, event: &StoreEvent, cx: &mut Context<Self>) {
        let Some(panel) = &self.cover.panel else {
            return;
        };
        let recheck = match (event, &panel.key) {
            (StoreEvent::Refreshed, _) => true,
            (StoreEvent::Committed(id), KeyState::Found(c)) => *id == c.id,
            _ => false,
        };
        if recheck {
            self.check_key(cx);
        }
    }

    /// What the phone's ⋯ menu asked of this page's cover.
    pub(super) fn on_cover_ask(
        &mut self,
        action: CoverAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            CoverAction::Choose => self.open_chooser(cx),
            CoverAction::Reposition => self.start_reposition(cx),
            CoverAction::Remove => self.remove_cover(cx),
            CoverAction::Generate => self.open_generate(window, cx),
        }
    }

    fn open_chooser(&mut self, cx: &mut Context<Self>) {
        self.cover.chooser = Some(Chooser::default());
        cx.notify();
    }

    fn pick_cover(&mut self, cx: &mut Context<Self>) {
        let pick = roostr_editor::pick_file(cx);
        cx.spawn(async move |this, cx| {
            if let Some(file) = pick.await {
                let _ = this.update(cx, |this, cx| this.add_cover(file, cx));
            }
        })
        .detach();
    }

    fn paste_cover(&mut self, cx: &mut Context<Self>) {
        let image = cx.read_from_clipboard().and_then(|item| {
            item.entries().iter().find_map(|e| match e {
                ClipboardEntry::Image(image) => Some(roostr_editor::pasted_image(image)),
                _ => None,
            })
        });
        match image {
            Some(file) => self.add_cover(file, cx),
            None => {
                if let Some(chooser) = &mut self.cover.chooser {
                    chooser.error = "There's no image on the clipboard - copy one first.".into();
                }
                cx.notify();
            }
        }
    }

    /// Add `file` to the page's space the usual way (a File object), then make it the cover.
    fn add_cover(&mut self, file: PickedFile, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        let Some(chooser) = &mut self.cover.chooser else {
            return;
        };
        if !file.mime.starts_with("image/") {
            chooser.error = format!(
                "{} isn't an image - choose a PNG, JPEG, GIF or WebP.",
                file.name
            );
            cx.notify();
            return;
        }
        chooser.adding = true;
        chooser.error.clear();
        cx.notify();
        let AddFile(add) = cx.global::<AddFile>().clone();
        let name = file.name.clone();
        let upload = add(
            file.name,
            file.mime,
            file.bytes,
            cover::space_of(&object),
            cx,
        );
        let page = object.id.clone();
        cx.spawn(async move |this, cx| {
            let result = match upload.await {
                Ok(id) => cx
                    .update(|cx| cover::set_cover(&page, &id, cx))
                    .await
                    .map_err(|e| format!("{e:#}")),
                Err(e) => Err(add_failure_text(&e, &name)),
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.cover.chooser = None,
                    Err(e) => {
                        if let Some(chooser) = &mut this.cover.chooser {
                            chooser.adding = false;
                            chooser.error = e;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_reposition(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        if cover::cover_file(&object.fields).is_some() {
            self.cover.reposition = Some(Reposition {
                y: cover::cover_y(&object.fields),
                drag: None,
            });
            cx.notify();
        }
    }

    fn save_reposition(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.cover.reposition.take() {
            cover::set_y(&self.id, r.y, cx).detach_and_log_err(cx);
        }
        cx.notify();
    }

    fn remove_cover(&mut self, cx: &mut Context<Self>) {
        self.cover.reposition = None;
        cover::remove(&self.id, cx).detach_and_log_err(cx);
        cx.notify();
    }

    fn open_generate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cover.chooser = None;
        if self.cover.panel.is_some() {
            return;
        }
        let prompt = self
            .object(cx)
            .and_then(|o| cover::generation(&o.fields))
            .map(|g| g.prompt)
            .unwrap_or_default();
        let state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder("Leave empty and the idea is written from this page")
        });
        state.update(cx, |s, cx| s.set_value(prompt.clone(), window, cx));
        self.cover.panel = Some(Panel {
            prompt: state,
            synced: prompt.clone(),
            shown_prompt: prompt,
            selected: None,
            seen_tries: 0,
            key: KeyState::Checking,
            error: String::new(),
        });
        self.check_key(cx);
        if let Some(object) = self.object(cx) {
            self.sync_cover(&object, window, cx);
        }
        cx.notify();
    }

    /// Look up the space's OpenAI credential and whether it has a key.
    fn check_key(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        let lookup = cover::openai_credential(&cover::space_of(&object), cx);
        cx.spawn(async move |this, cx| {
            let found = lookup.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(panel) = &mut this.cover.panel {
                    match found {
                        Ok(Some(c)) => panel.key = KeyState::Found(c),
                        Ok(None) => panel.key = KeyState::Missing,
                        Err(e) => panel.error = format!("{e:#}"),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Close the panel: the tries nobody used vanish, the session's fields go.
    fn close_generate(&mut self, kept: Option<String>, cx: &mut Context<Self>) {
        self.cover.panel = None;
        if let Some(object) = self.object(cx) {
            self.cover.dismissed = session_at(&object);
            if let Some(file) = &kept {
                cover::set_cover(&object.id, file, cx).detach_and_log_err(cx);
            }
            cover::end_session(&object, kept.as_deref(), cx).detach_and_log_err(cx);
        }
        cx.notify();
    }

    fn ask_cover(&mut self, action: Action, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.cover.panel else {
            return;
        };
        let text = panel.prompt.read(cx).value().trim().to_string();
        let prompt = match action {
            // The art director writes the idea unless the person wrote their own.
            Action::Generate if text != panel.synced.trim() => text,
            Action::Generate | Action::NewIdea => String::new(),
            Action::Reroll => text,
        };
        panel.error.clear();
        let ask = cover::request(&self.id, action, &prompt, cx);
        cx.spawn(async move |this, cx| {
            if let Err(e) = ask.await {
                let _ = this.update(cx, |this, cx| {
                    if let Some(panel) = &mut this.cover.panel {
                        panel.error = format!("{e:#}");
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn select_try(
        &mut self,
        file: String,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = &mut self.cover.panel else {
            return;
        };
        panel.selected = Some(file);
        panel
            .prompt
            .update(cx, |p, cx| p.set_value(prompt.clone(), window, cx));
        panel.synced = prompt;
        cx.notify();
    }

    // ----------------------------------------------------------------- render

    /// The banner, when the page has a cover.
    pub(super) fn render_cover_banner(
        &mut self,
        object: &Object,
        mobile: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (_, image) = self.cover.banner.as_ref()?;
        let image = image.clone();
        let repositioning = self.cover.reposition.as_ref();
        let y = repositioning.map_or_else(|| cover::cover_y(&object.fields), |r| r.y);
        let dragging = repositioning.is_some_and(|r| r.drag.is_some());
        let geometry = self.cover.geometry.clone();
        let radius = if mobile { px(0.) } else { theme::radius::md() };
        let painter = canvas(
            move |bounds, window, cx| {
                let render = image.and_then(|image| image.use_render_image(window, cx));
                let size = render.as_ref().map_or_else(Size::default, |r| {
                    let s = r.size(0);
                    size(s.width.0 as f32, s.height.0 as f32)
                });
                geometry.set(Geometry {
                    bounds,
                    image: size,
                });
                render.map(|r| (r, size))
            },
            move |bounds, render, window, _| {
                if let Some((render, size)) = render {
                    let _ = window.paint_image(
                        bounds,
                        cover_bounds(bounds, size, y),
                        Corners::all(radius),
                        render,
                        0,
                        false,
                    );
                }
            },
        )
        .absolute()
        .inset_0();
        let actions = (!mobile && self.cover.banner_hovered && self.cover.reposition.is_none())
            .then(|| {
                div()
                    .absolute()
                    .right(px(12.))
                    .bottom(px(12.))
                    .flex()
                    .gap(px(6.))
                    .child(tagged(
                        "cover-change",
                        pill("cover-change", "Change cover", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.open_chooser(cx))),
                    ))
                    .child(tagged(
                        "cover-generate",
                        pill("cover-generate", "Generate", cx).on_click(
                            cx.listener(|this, _, window, cx| this.open_generate(window, cx)),
                        ),
                    ))
                    .child(tagged(
                        "cover-reposition",
                        pill("cover-reposition", "Reposition", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.start_reposition(cx))),
                    ))
                    .child(tagged(
                        "cover-remove",
                        pill("cover-remove", "Remove", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.remove_cover(cx))),
                    ))
            });
        let reposition = self.cover.reposition.is_some().then(|| {
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .bg(hsla(0., 0., 0., 0.45))
                        .text_size(px(12.))
                        .text_color(theme::white())
                        .child("Drag the image to reposition"),
                )
                .child(
                    div()
                        .absolute()
                        .right(px(12.))
                        .bottom(px(12.))
                        .flex()
                        .gap(px(6.))
                        .child(tagged(
                            "cover-save",
                            pill("cover-save", "Save position", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.save_reposition(cx))),
                        ))
                        .child(tagged(
                            "cover-cancel",
                            pill("cover-cancel", "Cancel", cx).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.cover.reposition = None;
                                    cx.notify();
                                },
                            )),
                        )),
                )
        });
        let banner = div()
            .id("object-cover")
            .debug_selector(|| "object-cover".into())
            .relative()
            .w_full()
            .h(px(if mobile { BANNER_H_PHONE } else { BANNER_H }))
            .flex_none()
            .overflow_hidden()
            .rounded(radius)
            .bg(theme::hover())
            .when(mobile, |d| d.mt(px(-24.)))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.cover.banner_hovered = *hovered;
                cx.notify();
            }))
            .when(self.cover.reposition.is_some(), |d| {
                d.cursor(if dragging {
                    CursorStyle::ClosedHand
                } else {
                    CursorStyle::OpenHand
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, e: &MouseDownEvent, _, cx| {
                        if let Some(r) = &mut this.cover.reposition {
                            r.drag = Some((e.position.y, r.y));
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                    let overflow = this.cover.geometry.get().overflow();
                    if let Some(r) = &mut this.cover.reposition
                        && let Some((start, from)) = r.drag
                        && e.pressed_button == Some(MouseButton::Left)
                        && overflow > 0.
                    {
                        r.y = (from - f32::from(e.position.y - start) / overflow).clamp(0., 1.);
                        cx.notify();
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        if let Some(r) = &mut this.cover.reposition {
                            r.drag = None;
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        if let Some(r) = &mut this.cover.reposition {
                            r.drag = None;
                            cx.notify();
                        }
                    }),
                )
            })
            .child(painter)
            .children(actions)
            .children(reposition);
        Some(banner.into_any_element())
    }

    /// Hover tracking for the icon and title: a page without a cover offers Add cover there.
    pub(super) fn cover_head_hover(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        cx.listener(|this, hovered: &bool, _, cx| {
            if this.cover.head_hovered != *hovered {
                this.cover.head_hovered = *hovered;
                cx.notify();
            }
        })
    }

    /// Desktop, no cover, pointer over the icon and title: Add cover.
    pub(super) fn render_add_cover(
        &self,
        object: &Object,
        mobile: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let offer = !mobile
            && self.cover.head_hovered
            && self.cover.banner.is_none()
            && self.cover.chooser.is_none()
            && cover::can_have_cover(object);
        offer.then(|| {
            div()
                .absolute()
                .top(px(4.))
                .right_0()
                .child(tagged(
                    "cover-add",
                    Button::new("cover-add")
                        .ghost()
                        .h(px(26.))
                        .px(px(8.))
                        .text_color(theme::muted())
                        .accessibility_label("Add cover")
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(13.))
                                .child(Icon::new(Glyph::Plus).size(px(14.)))
                                .child("Add cover"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.open_chooser(cx))),
                ))
                .into_any_element()
        })
    }

    /// The chooser and the generate panel, under the title.
    pub(super) fn render_cover_cards(
        &mut self,
        object: &Object,
        mobile: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .when(mobile, |d| d.px(px(16.)))
            .children(self.render_chooser(object, cx))
            .children(self.render_panel(object, cx))
    }

    fn render_chooser(&self, object: &Object, cx: &mut Context<Self>) -> Option<Div> {
        let chooser = self.cover.chooser.as_ref()?;
        let title = if cover::cover_file(&object.fields).is_some() {
            "Change cover"
        } else {
            "Add cover"
        };
        let disabled = chooser.adding;
        Some(
            card("cover-chooser")
                .child(card_title(
                    title,
                    "cover-chooser-close",
                    cx.listener(|this, _, _, cx| {
                        this.cover.chooser = None;
                        cx.notify();
                    }),
                ))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .when(roostr_editor::CAN_PICK_FILES, |d| {
                            d.child(tagged(
                                "cover-pick",
                                ui::disabled(
                                    ui::outline_button("cover-pick", "Choose an image…"),
                                    disabled,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.pick_cover(cx))),
                            ))
                        })
                        .child(tagged(
                            "cover-paste",
                            ui::disabled(
                                ui::outline_button("cover-paste", "Paste image"),
                                disabled,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.paste_cover(cx))),
                        ))
                        .child(tagged(
                            "cover-chooser-generate",
                            ui::disabled(
                                ui::outline_button("cover-chooser-generate", "Generate…"),
                                disabled,
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_generate(window, cx)),
                            ),
                        )),
                )
                .when(chooser.adding, |d| d.child(small_note("Adding the image…")))
                .when(!chooser.error.is_empty(), |d| {
                    d.child(small_note(chooser.error.clone()).text_color(theme::red()))
                }),
        )
    }

    fn render_panel(&self, object: &Object, cx: &mut Context<Self>) -> Option<Div> {
        let panel = self.cover.panel.as_ref()?;
        let generation: Option<Generation> = cover::generation(&object.fields);
        let pending = cover::pending_request(&object.fields);
        let running = generation.as_ref().is_some_and(Generation::running);
        // The ask is waiting for its computer until a generation answers it.
        let waiting = pending.is_some_and(|at| {
            generation
                .as_ref()
                .is_none_or(|g| g.at != at || !g.running())
        });
        let busy = running || waiting;
        let reported = generation
            .as_ref()
            .filter(|g| g.status == Some(Status::Failed));
        let (needs_key, credential) = match (&panel.key, reported.filter(|g| g.needs_key)) {
            (_, Some(g)) => (true, Some(g.credential.clone()).filter(|c| !c.is_empty())),
            (KeyState::Missing, None) => (true, None),
            (KeyState::Found(c), None) if !c.has_key => (true, Some(c.id.clone())),
            _ => (false, None),
        };
        let ready = matches!(&panel.key, KeyState::Found(c) if c.has_key);
        let can_ask = ready && !busy;
        let prompt_text = panel.prompt.read(cx).value().trim().to_string();
        let tries = generation
            .as_ref()
            .map(|g| g.tries.clone())
            .unwrap_or_default();
        let selected = panel
            .selected
            .clone()
            .filter(|f| tries.iter().any(|t| t.file == *f));
        let progress = if running {
            generation.as_ref().and_then(|g| match g.status {
                Some(Status::Writing) => Some(("cover-progress-writing", WRITING)),
                Some(Status::Painting) => Some(("cover-progress-painting", PAINTING)),
                _ => None,
            })
        } else if waiting {
            Some(("cover-progress-waiting", WAITING))
        } else {
            None
        };
        let preview = selected
            .as_ref()
            .and_then(|f| self.cover.thumbs.get(f).cloned().flatten());
        let thumbs = tries.iter().enumerate().map(|(i, t)| {
            let image = self.cover.thumbs.get(&t.file).cloned().flatten();
            let chosen = selected.as_deref() == Some(t.file.as_str());
            let (file, prompt) = (t.file.clone(), t.prompt.clone());
            div()
                .id(("cover-try", i))
                .debug_selector(move || format!("cover-try-{i}"))
                .w(px(96.))
                .h(px(54.))
                .flex_none()
                .rounded(px(6.))
                .overflow_hidden()
                .border_2()
                .border_color(if chosen {
                    theme::accent()
                } else {
                    transparent_black()
                })
                .bg(theme::hover())
                .cursor_pointer()
                .when_some(image, |d, image| {
                    d.child(
                        img(ImageSource::Image(image))
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_try(file.clone(), prompt.clone(), window, cx)
                }))
        });
        let failed = reported
            .filter(|g| !g.needs_key && !g.error.is_empty())
            .map(|g| g.error.clone());
        let key_text = reported
            .filter(|g| g.needs_key && !g.error.is_empty())
            .map_or_else(|| cover::NEEDS_KEY.to_string(), |g| g.error.clone());
        Some(
            card("cover-panel")
                .child(card_title(
                    "Generate a cover",
                    "cover-panel-close",
                    cx.listener(|this, _, _, cx| this.close_generate(None, cx)),
                ))
                .when(needs_key, |d| {
                    d.child(
                        div()
                            .debug_selector(|| "cover-needs-key".into())
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(6.))
                            .child(small_note(key_text).text_color(theme::fg()))
                            .when_some(credential, |d, id| {
                                d.child(tagged(
                                    "cover-open-credential",
                                    ui::outline_button(
                                        "cover-open-credential",
                                        "Open the OpenAI credential",
                                    )
                                    .on_click(
                                        move |_, _, cx| navigate(Route::Object(id.clone()), cx),
                                    ),
                                ))
                            }),
                    )
                })
                .children(
                    progress.map(|(id, text)| small_note(text).debug_selector(move || id.into())),
                )
                .children(failed.map(|e| {
                    small_note(e)
                        .debug_selector(|| "cover-error".into())
                        .text_color(theme::red())
                }))
                .when(!panel.error.is_empty(), |d| {
                    d.child(small_note(panel.error.clone()).text_color(theme::red()))
                })
                .child(
                    div()
                        .debug_selector(|| "cover-prompt".into())
                        .child(Textarea::new(&panel.prompt).text_size(px(13.))),
                )
                .when_some(preview, |d, image| {
                    d.child(
                        div()
                            .debug_selector(|| "cover-preview".into())
                            .w_full()
                            .h(px(140.))
                            .rounded(px(8.))
                            .overflow_hidden()
                            .child(
                                img(ImageSource::Image(image))
                                    .size_full()
                                    .object_fit(ObjectFit::Cover),
                            ),
                    )
                })
                .when(!tries.is_empty(), |d| {
                    d.child(div().flex().flex_wrap().gap(px(6.)).children(thumbs))
                })
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .child(tagged(
                            "cover-ask-generate",
                            ui::disabled(
                                ui::outline_button("cover-ask-generate", "Generate"),
                                !can_ask,
                            )
                            .on_click(
                                cx.listener(|this, _, _, cx| this.ask_cover(Action::Generate, cx)),
                            ),
                        ))
                        .child(tagged(
                            "cover-ask-reroll",
                            ui::disabled(
                                ui::outline_button("cover-ask-reroll", "Reroll"),
                                !can_ask || prompt_text.is_empty(),
                            )
                            .on_click(
                                cx.listener(|this, _, _, cx| this.ask_cover(Action::Reroll, cx)),
                            ),
                        ))
                        .child(tagged(
                            "cover-ask-new-idea",
                            ui::disabled(
                                ui::outline_button("cover-ask-new-idea", "New idea"),
                                !can_ask || generation.is_none(),
                            )
                            .on_click(
                                cx.listener(|this, _, _, cx| this.ask_cover(Action::NewIdea, cx)),
                            ),
                        ))
                        .child(tagged(
                            "cover-use",
                            ui::disabled(
                                ui::primary_button("cover-use", "Use this"),
                                selected.is_none(),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let kept =
                                        this.cover.panel.as_ref().and_then(|p| p.selected.clone());
                                    this.close_generate(kept, cx)
                                },
                            )),
                        )),
                )
                .child(
                    small_note(cover::COST_NOTE)
                        .debug_selector(|| "cover-cost".into())
                        .text_size(px(11.)),
                ),
        )
    }
}

/// A bordered card in the page's flow.
fn card(id: &'static str) -> Div {
    div()
        .debug_selector(move || id.into())
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(12.))
        .mb(px(12.))
        .rounded(px(10.))
        .border_1()
        .border_color(theme::border())
        .bg(theme::bg())
}

/// A card's heading with its close button.
fn card_title(
    title: &'static str,
    close: &'static str,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::fg())
                .child(title),
        )
        .child(tagged(
            close,
            ui::icon_button(close, Glyph::BadgeX)
                .accessibility_label("Close")
                .tooltip("Close")
                .on_click(on_close),
        ))
}

/// 12px muted text that wraps.
fn small_note(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .line_height(relative(1.4))
        .text_color(theme::muted())
        .child(text.into())
}

#[cfg(test)]
mod tests {
    // Explicit imports: the kit's glob would shadow `#[test]` with GPUI's.
    use gpui_kit::{Bounds, point, px, size};

    use super::{Geometry, cover_bounds};

    #[test]
    fn the_cover_fills_the_banner_and_slides_with_its_focus() {
        let banner = Bounds {
            origin: point(px(10.), px(20.)),
            size: size(px(800.), px(200.)),
        };
        // A square image fills the width: 800 tall, 600 of it hidden.
        let square = size(100., 100.);
        let top = cover_bounds(banner, square, 0.);
        assert_eq!(top.size, size(px(800.), px(800.)));
        assert_eq!(top.origin, point(px(10.), px(20.)));
        assert_eq!(cover_bounds(banner, square, 1.).origin.y, px(20. - 600.));
        assert_eq!(cover_bounds(banner, square, 0.5).origin.y, px(20. - 300.));
        let g = Geometry {
            bounds: banner,
            image: square,
        };
        assert_eq!(g.overflow(), 600.);
        // A very wide image fills the height and centres sideways: nothing to drag.
        let wide = size(1000., 100.);
        let b = cover_bounds(banner, wide, 0.);
        assert_eq!(b.size, size(px(2000.), px(200.)));
        assert_eq!(b.origin.x, px(10. - 600.));
        let g = Geometry {
            bounds: banner,
            image: wide,
        };
        assert_eq!(g.overflow(), 0.);
    }
}
