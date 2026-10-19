//! DataView for an object id (query/set, collection or type) plus a picker
//! switching between the vault's set objects.
//!
//! `cargo run -p roostr-sets --example sets -- [object-id]`

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::{Store, api, field_str, theme};
use roostr_sets::{DataView, DataViewKind};
use serde_json::json;

struct Picker {
    sets: Vec<(String, String, String)>,
    current: Option<(String, Entity<DataView>)>,
    scroll: ScrollHandle,
}

impl Picker {
    fn new(
        initial: Option<String>,
        initial_kind: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        start_driver(window, cx);
        let task = api::query(
            json!({"filters": [{"key": "typeKey", "condition": "in", "value": ["query", "set", "collection"]}], "fields": ["name", "channel"]}),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(res) = task.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.sets = res
                    .records
                    .iter()
                    .map(|r| {
                        (
                            r.id.clone(),
                            r.type_key.clone(),
                            field_str(&r.fields, "name").to_string(),
                        )
                    })
                    .collect();
                this.sets.sort_by(|a, b| a.2.cmp(&b.2).then(a.0.cmp(&b.0)));
                if this.current.is_none()
                    && let Some((id, kind, _)) = this.sets.first().cloned()
                {
                    this.open(id, &kind, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
        let mut this = Self {
            sets: Vec::new(),
            current: None,
            scroll: ScrollHandle::new(),
        };
        if let Some(id) = initial {
            let kind = initial_kind.unwrap_or_else(|| {
                Store::global(cx)
                    .read(cx)
                    .summary(&id)
                    .map(|s| s.type_key.clone())
                    .unwrap_or_else(|| "query".into())
            });
            this.open(id, &kind, window, cx);
        }
        this
    }

    fn open(&mut self, id: String, type_key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let kind = match type_key {
            "collection" => DataViewKind::Collection,
            "type" => DataViewKind::Type,
            _ => DataViewKind::Query,
        };
        let view = cx.new(|cx| DataView::new(id.clone(), kind, window, cx));
        self.current = Some((id, view));
        cx.notify();
    }
}

impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self
            .current
            .as_ref()
            .map(|c| c.0.clone())
            .unwrap_or_default();
        let mut bar = div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .px(px(32.))
            .pt(px(12.))
            .pb(px(8.))
            .border_b_1()
            .border_color(theme::border());
        for (id, kind, name) in &self.sets {
            let on = *id == current;
            let (pick, k) = (id.clone(), kind.clone());
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("pick-{id}")))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(999.))
                    .border_1()
                    .border_color(if on { theme::accent() } else { theme::border() })
                    .text_size(px(12.))
                    .cursor_pointer()
                    .child(format!("{name} · {kind}"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open(pick.clone(), &k, window, cx)
                    })),
            );
        }
        let title = self
            .current
            .as_ref()
            .and_then(|(id, _)| Store::global(cx).read(cx).object(id))
            .map(|o| o.name().to_string())
            .unwrap_or_default();
        let view = self.current.as_ref().map(|c| c.1.clone());
        div().size_full().flex().flex_col().child(bar).child(
            div()
                .id("page")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .px(px(32.))
                .pt(px(24.))
                .pb(px(80.))
                // The page around the data view arms its rubber band too (as the object page should).
                .when_some(view, |d, view| {
                    d.on_mouse_down(MouseButton::Left, move |e, window, cx| {
                        view.update(cx, |v, cx| v.page_mouse_down(e, window, cx));
                    })
                })
                .child(
                    div()
                        .pl(px(48.))
                        .pb(px(16.))
                        .text_size(px(34.))
                        .font_weight(FontWeight(750.))
                        .child(title),
                )
                .when_some(self.current.as_ref().map(|c| c.1.clone()), |d, v| {
                    d.child(div().ml(px(48.)).child(v))
                }),
        )
    }
}

/// `SETS_DRIVE=1`: read input commands from stdin and dispatch them into the
/// window as real platform events, so the views can be exercised headlessly
/// (window coordinates in points): `click X Y [cmd|shift]`, `rclick X Y`,
/// `move X Y`, `drag X1 Y1 X2 Y2`, `key <keystroke>`, `type <text>`.
fn drive(line: &str, window: &mut Window, cx: &mut App) {
    eprintln!("drive: {line}");
    let parts: Vec<&str> = line.split_whitespace().collect();
    let num = |i: usize| {
        parts
            .get(i)
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(0.)
    };
    let at = |i: usize| point(px(num(i)), px(num(i + 1)));
    let mods = |s: Option<&&str>| Modifiers {
        platform: s == Some(&"cmd"),
        shift: s == Some(&"shift"),
        ..Default::default()
    };
    let press = |pos: Point<Pixels>,
                 button: MouseButton,
                 modifiers: Modifiers,
                 window: &mut Window,
                 cx: &mut App| {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: pos,
                pressed_button: None,
                modifiers,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position: pos,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position: pos,
                modifiers,
                click_count: 1,
            }),
            cx,
        );
    };
    match parts.first().copied() {
        Some("click") => press(at(1), MouseButton::Left, mods(parts.get(3)), window, cx),
        Some("rclick") => press(at(1), MouseButton::Right, Modifiers::default(), window, cx),
        Some("move") => {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: at(1),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
        }
        Some("drag") => {
            let (from, to) = (at(1), at(3));
            let m = Modifiers::default();
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: from,
                    pressed_button: None,
                    modifiers: m,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: from,
                    modifiers: m,
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            for step in 1..=12 {
                let t = step as f32 / 12.;
                let p = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: p,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: m,
                    }),
                    cx,
                );
            }
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: to,
                    modifiers: m,
                    click_count: 1,
                }),
                cx,
            );
        }
        Some("key") => {
            if let Ok(k) = Keystroke::parse(parts.get(1).unwrap_or(&"")) {
                window.dispatch_keystroke(k, cx);
            }
        }
        Some("type") => {
            let text = line
                .trim_start()
                .strip_prefix("type")
                .unwrap_or("")
                .trim_start();
            for ch in text.chars() {
                let key = if ch == ' ' {
                    "space".to_string()
                } else {
                    ch.to_string()
                };
                if let Ok(mut k) = Keystroke::parse(&key) {
                    k.key_char = Some(ch.to_string());
                    window.dispatch_keystroke(k, cx);
                }
            }
        }
        _ => {}
    }
    window.refresh();
}

fn start_driver(window: &mut Window, cx: &mut Context<Picker>) {
    if std::env::var_os("SETS_DRIVE").is_none() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    cx.spawn_in(window, async move |_, cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(30))
                .await;
            while let Ok(line) = rx.try_recv() {
                let _ = cx.update(|window, cx| drive(&line, window, cx));
            }
        }
    })
    .detach();
}

fn main() {
    roostr_core::dev::run_example("Sets", |args, window, cx| {
        roostr_properties::init(cx);
        roostr_sets::init(cx);
        let initial = args.first().cloned();
        // Optional second arg: the object's type key (types are not in the summaries).
        let kind = args.get(1).cloned();
        cx.new(|cx| Picker::new(initial, kind, window, cx)).into()
    });
}
