//! Editor example: `cargo run -p roostr-editor --example editor -- [object-id] [--readonly]`.
//!
//! Opens the object's body in the block editor (default: the seeded
//! "Editor showcase" page with every block style). A File object id opens
//! its File page instead. `--readonly` shows the body without editing.
//!
//! `EDITOR_DRIVE=1` reads input commands from stdin and dispatches them into
//! the window as platform events (window coordinates in points), so the
//! editor can be exercised without the shared OS focus:
//! `click X Y [cmd|shift|alt] [count]`, `rclick X Y`, `move X Y`,
//! `drag X1 Y1 X2 Y2`, `scroll X Y DY`, `key <keystroke>`, `type <text>`,
//! `drop X Y <path>` (a file dragged in from Finder), `clip <text>`,
//! `clipimg <path>` (a PNG on the clipboard).

use std::path::PathBuf;

use gpui_kit::*;
use roostr_core::{Store, theme};
use roostr_editor::{Editor, FileView};

struct Page {
    object_id: String,
    body: AnyView,
    scroll: ScrollHandle,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = Store::global(cx)
            .read(cx)
            .object(&self.object_id)
            .map(|o| o.name().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Untitled".into());
        div().size_full().child(
            div()
                .id("page-scroll")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(
                    div()
                        .max_w(px(920.))
                        .mx_auto()
                        .px(px(32.))
                        .pt(px(24.))
                        .pb(px(80.))
                        .child(
                            div()
                                .ml(px(48.))
                                .mb(px(12.))
                                .text_size(px(34.))
                                .font_weight(FontWeight(750.))
                                .text_color(theme::fg())
                                .child(name),
                        )
                        .child(self.body.clone()),
                ),
        )
    }
}

fn mouse(window: &mut Window, cx: &mut App, event: PlatformInput) {
    window.dispatch_event(event, cx);
}

fn press(
    pos: Point<Pixels>,
    button: MouseButton,
    modifiers: Modifiers,
    count: usize,
    window: &mut Window,
    cx: &mut App,
) {
    mouse(
        window,
        cx,
        PlatformInput::MouseMove(MouseMoveEvent {
            position: pos,
            pressed_button: None,
            modifiers,
        }),
    );
    mouse(
        window,
        cx,
        PlatformInput::MouseDown(MouseDownEvent {
            button,
            position: pos,
            modifiers,
            click_count: count,
            first_mouse: false,
        }),
    );
    mouse(
        window,
        cx,
        PlatformInput::MouseUp(MouseUpEvent {
            button,
            position: pos,
            modifiers,
            click_count: count,
        }),
    );
}

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
    let rest = |n: usize| {
        let mut s = line.trim_start();
        for _ in 0..n {
            s = s.split_once(' ').map_or("", |(_, r)| r).trim_start();
        }
        s.to_string()
    };
    match parts.first().copied() {
        Some("click") => {
            let modifiers = Modifiers {
                platform: parts.get(3) == Some(&"cmd"),
                shift: parts.get(3) == Some(&"shift"),
                alt: parts.get(3) == Some(&"alt"),
                ..Default::default()
            };
            let count = parts.get(4).and_then(|c| c.parse().ok()).unwrap_or(1);
            press(at(1), MouseButton::Left, modifiers, count, window, cx);
        }
        Some("rclick") => press(
            at(1),
            MouseButton::Right,
            Modifiers::default(),
            1,
            window,
            cx,
        ),
        Some("move") => mouse(
            window,
            cx,
            PlatformInput::MouseMove(MouseMoveEvent {
                position: at(1),
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
        ),
        Some("drag") => {
            let (from, to) = (at(1), at(3));
            let m = Modifiers::default();
            mouse(
                window,
                cx,
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: from,
                    pressed_button: None,
                    modifiers: m,
                }),
            );
            mouse(
                window,
                cx,
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: from,
                    modifiers: m,
                    click_count: 1,
                    first_mouse: false,
                }),
            );
            for step in 1..=16 {
                let t = step as f32 / 16.;
                let p = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
                mouse(
                    window,
                    cx,
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: p,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: m,
                    }),
                );
            }
            mouse(
                window,
                cx,
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: to,
                    modifiers: m,
                    click_count: 1,
                }),
            );
        }
        // A drag in pieces, so the pointer can rest at an edge between moves.
        Some("down") => mouse(
            window,
            cx,
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: at(1),
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
        ),
        Some("hold") => mouse(
            window,
            cx,
            PlatformInput::MouseMove(MouseMoveEvent {
                position: at(1),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
        ),
        Some("up") => mouse(
            window,
            cx,
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: at(1),
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
        ),
        Some("scroll") => mouse(
            window,
            cx,
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: at(1),
                delta: ScrollDelta::Pixels(point(px(0.), px(num(3)))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            }),
        ),
        Some("key") => {
            if let Ok(k) = Keystroke::parse(parts.get(1).unwrap_or(&"")) {
                window.dispatch_keystroke(k, cx);
            }
        }
        Some("type") => {
            for ch in rest(1).chars() {
                let key = if ch == ' ' {
                    "space".to_string()
                } else {
                    ch.to_lowercase().to_string()
                };
                if let Ok(mut k) = Keystroke::parse(&key) {
                    k.modifiers.shift = ch.is_uppercase();
                    k.key_char = Some(ch.to_string());
                    window.dispatch_keystroke(k, cx);
                }
            }
        }
        Some("drop") => {
            let pos = at(1);
            let paths = ExternalPaths(vec![PathBuf::from(rest(3))].into());
            mouse(
                window,
                cx,
                PlatformInput::FileDrop(FileDropEvent::Entered {
                    position: pos,
                    paths,
                }),
            );
            mouse(
                window,
                cx,
                PlatformInput::FileDrop(FileDropEvent::Pending { position: pos }),
            );
            mouse(
                window,
                cx,
                PlatformInput::FileDrop(FileDropEvent::Submit { position: pos }),
            );
        }
        // An OS file drag that hovers, then leaves the window without dropping.
        Some("filehover") => {
            let pos = at(1);
            let paths = ExternalPaths(vec![PathBuf::from(rest(3))].into());
            mouse(
                window,
                cx,
                PlatformInput::FileDrop(FileDropEvent::Entered {
                    position: pos,
                    paths,
                }),
            );
            mouse(
                window,
                cx,
                PlatformInput::FileDrop(FileDropEvent::Pending { position: pos }),
            );
        }
        Some("fileexit") => mouse(window, cx, PlatformInput::FileDrop(FileDropEvent::Exited)),
        Some("clip") => cx.write_to_clipboard(ClipboardItem::new_string(rest(1))),
        Some("clipimg") => {
            if let Ok(bytes) = std::fs::read(rest(1)) {
                cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                    ImageFormat::Png,
                    bytes,
                )));
            }
        }
        _ => {}
    }
    window.refresh();
}

fn start_driver(window: &mut Window, cx: &mut App) {
    if std::env::var_os("EDITOR_DRIVE").is_none() {
        return;
    }
    // Report navigations (a mention click opens its object).
    let nav = roostr_core::Navigator::global(cx);
    cx.observe(&nav, |nav, cx| eprintln!("NAV {:?}", nav.read(cx).route()))
        .detach();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    window
        .spawn(cx, async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(30))
                    .await;
                while let Ok(line) = rx.try_recv() {
                    // Hover first and let a frame pass, as a real pointer does before it presses.
                    let mut parts = line.split_whitespace();
                    if let (Some("click" | "rclick"), Some(x), Some(y)) =
                        (parts.next(), parts.next(), parts.next())
                    {
                        let _ = cx.update(|window, cx| drive(&format!("move {x} {y}"), window, cx));
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(60))
                            .await;
                    }
                    let _ = cx.update(|window, cx| drive(&line, window, cx));
                }
            }
        })
        .detach();
}

fn main() {
    roostr_core::dev::run_example("Editor", |args, window, cx| {
        roostr_editor::init(cx);
        roostr_properties::init(cx);
        let read_only = args.iter().any(|a| a == "--readonly");
        let object_id = args
            .iter()
            .find(|a| !a.starts_with("--"))
            .cloned()
            .unwrap_or_else(|| "seed-page-showcase".into());
        Store::global(cx).update(cx, |s, cx| s.ensure_object(&object_id, cx));
        let is_file = Store::global(cx)
            .read(cx)
            .summary(&object_id)
            .is_some_and(|s| s.type_key == "file");
        let body: AnyView = if is_file {
            cx.new(|cx| FileView::new(object_id.clone(), window, cx))
                .into()
        } else {
            cx.new(|cx| Editor::new(object_id.clone(), read_only, window, cx))
                .into()
        };
        start_driver(window, cx);
        cx.new(|_| Page {
            object_id,
            body,
            scroll: ScrollHandle::new(),
        })
        .into()
    });
}
