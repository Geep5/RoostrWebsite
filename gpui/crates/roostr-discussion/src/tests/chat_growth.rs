//! The chat's message list is a Rune growth stack: a message that arrives once the chat is
//! showing eases its height open (a reply at the streaming pace while its words fade in, a
//! sent line in one quick glide) and the pinned end follows; a sent line landing as the real
//! message keeps its row. Under Reduce Motion rows land at once and only the words' fade
//! stays.

use std::time::Duration;

use roostr_core::PaneTab;
use roostr_core::nav::Route;
use rune::runtime::ElementInfo;
use rune::{Key, Mods};

use super::{Harness, Vault, chat_block, object, shown, with_discussion};
use crate::discussion::Place;

const ME: &str = "me";
const REPLY: &str = "Here is the plan for the launch: we ship the beta to the first cohort on \
                     Monday, gather their notes through the week, fix what they find and open \
                     the doors to everyone the Monday after, with the announcement ready.";

/// The live (not exiting) element keyed `key`.
fn info(h: &Harness, key: &str) -> Option<ElementInfo> {
    h.rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key) && !e.exiting)
}

/// The chat pane open on `page-1`, whose thread already has a dozen messages.
fn open_chat() -> (std::sync::Arc<Vault>, Harness) {
    let messages = (1..=12)
        .map(|n| chat_block(&format!("m{n}"), "peer", 1_000 + n, &format!("Message {n}")))
        .collect();
    let vault = Vault::new(
        vec![with_discussion(
            object("page-1", "page", "Launch plan"),
            messages,
        )],
        ME,
    );
    let mut h = Harness::new(vault.clone(), (1280., 800.));
    h.update(|s| {
        let pane = &mut s.pane;
        pane.set_open(PaneTab::Chat, true, &mut s.ui);
    });
    h.navigate(Route::Object("page-1".into()));
    assert!(h.has("msg-m12"), "the chat shows its thread");
    (vault, h)
}

/// Chat line `id` lands in `page-1`'s thread in the vault, and the daemon says so.
fn line_lands(vault: &Vault, id: &str, author: &str, ts: i64, text: &str) {
    vault.update("page-1", |o| {
        let mut blocks = serde_json::to_value(&o.blocks).expect("blocks");
        let list = blocks.as_array_mut().expect("block list");
        list[0]["childrenIds"]
            .as_array_mut()
            .expect("thread children")
            .push(id.into());
        list.push(chat_block(id, author, ts, text));
        o.blocks = serde_json::from_value(blocks).expect("blocks");
    });
    vault.commit("page-1");
}

/// A peer's reply `id` arrives.
fn reply_arrives(vault: &Vault, id: &str) {
    line_lands(vault, id, "peer", 5_000, REPLY);
}

fn arriving(h: &Harness) -> bool {
    shown(h, Place::Pane).is_some_and(|d| d.arriving)
}

#[test]
fn a_reply_arriving_at_the_end_grows_in_while_its_words_fade_in() {
    let (vault, mut h) = open_chat();
    reply_arrives(&vault, "r1");
    h.frames_until("the reply mounting", |h| h.has("msg-r1"));

    let row = info(&h, "msg-r1").expect("the reply's row");
    assert!(
        row.rect[3] < row.target[3] - 1.0,
        "the reply starts below its height: {} of {}",
        row.rect[3],
        row.target[3]
    );
    assert!(arriving(&h), "the reply's row streams");
    let line = info(&h, "md-r1-0-0").expect("the reply's text");
    assert!(line.revealing, "the reply's words fade in");

    // Its height only rises while it streams.
    let mut last = row.rect[3];
    for _ in 0..30 {
        h.frame();
        let now = info(&h, "msg-r1").expect("the reply's row").rect[3];
        assert!(
            now >= last - 0.01,
            "the reply's row shrank: {last} then {now}"
        );
        last = now;
    }
    assert!(
        last > row.rect[3],
        "the reply's row grew: {} to {last}",
        row.rect[3]
    );

    // Once its words are in, it stops streaming and lands at its full height, pinned to the
    // end of the list.
    h.advance(Duration::from_secs(3));
    assert!(!arriving(&h), "the reply has finished streaming");
    let row = info(&h, "msg-r1").expect("the reply's row");
    assert!(
        (row.rect[3] - row.target[3]).abs() < 0.5,
        "the reply reaches its height: {} of {}",
        row.rect[3],
        row.target[3]
    );
    assert!(!info(&h, "md-r1-0-0").expect("the reply's text").revealing);
    let list = h.rect("chat-messages").expect("the message list");
    let below = list[1] + list[3] - (row.rect[1] + row.rect[3]);
    assert!(
        (-0.5..8.0).contains(&below),
        "the list stays at its end: {below}pt under the reply"
    );
}

#[test]
fn a_sent_line_glides_open_and_keeps_its_row_when_it_lands() {
    let (vault, mut h) = open_chat();
    h.type_text("Ship it Monday");
    h.key(Key::Enter, Mods::default());
    let sending = |h: &Harness| {
        h.rt.elements().into_iter().find(|e| {
            !e.exiting
                && e.key
                    .as_deref()
                    .is_some_and(|k| k.starts_with("msg-sending-"))
        })
    };
    let row = sending(&h).expect("the sent line shows at once");
    let key = row.key.clone().expect("keyed");
    assert!(
        row.rect[3] < row.target[3] - 1.0,
        "the sent line starts below its height: {} of {}",
        row.rect[3],
        row.target[3]
    );
    assert!(!arriving(&h), "your own line arrives whole, not streaming");
    for _ in 0..40 {
        h.frame();
    }
    let row = info(&h, &key).expect("the sent line");
    assert!(
        (row.rect[3] - row.target[3]).abs() < 0.5,
        "the sent line glides to its height: {} of {}",
        row.rect[3],
        row.target[3]
    );

    // The object carries the real line: the same row shows it, at its height, with no
    // second row growing in.
    let now = chrono::Utc::now().timestamp_millis();
    line_lands(&vault, "real-1", ME, now, "Ship it Monday");
    h.frames_until("the line landing", |h| {
        shown(h, Place::Pane).is_some_and(|d| d.outgoing.is_none())
    });
    h.frame();
    assert!(!h.has("msg-real-1"), "no second row for the landed line");
    let row = info(&h, &key).expect("the landed line keeps its row");
    assert!(
        (row.rect[3] - row.target[3]).abs() < 0.5,
        "the landed line neither closes nor regrows: {} of {}",
        row.rect[3],
        row.target[3]
    );
}

#[test]
fn under_reduce_motion_a_reply_lands_at_once_and_only_fades() {
    let (vault, mut h) = open_chat();
    h.rt.set_system_reduced_motion(true);
    h.frame();
    reply_arrives(&vault, "r1");
    h.frames_until("the reply mounting", |h| h.has("msg-r1"));

    let row = info(&h, "msg-r1").expect("the reply's row");
    assert!(
        (row.rect[3] - row.target[3]).abs() < 0.5,
        "the reply lands at its height: {} of {}",
        row.rect[3],
        row.target[3]
    );
    assert!(
        info(&h, "md-r1-0-0").expect("the reply's text").revealing,
        "its words still fade in"
    );
    // Rune's reduced fade (0.15 s), every word at once.
    for _ in 0..15 {
        h.frame();
    }
    assert!(!info(&h, "md-r1-0-0").expect("the reply's text").revealing);
}

/// Frames of the chat's message list, tiled into one PNG.
struct Strip {
    gpu: rune::render::Gpu,
    renderer: rune::render::Renderer,
    tiles: Vec<Vec<u8>>,
    tile: (u32, u32),
}

/// The window's pixel size and scale (the harness draws 1280x800 at 2x).
const WINDOW: (u32, u32) = (2560, 1600);
const SCALE: f32 = 2.0;
/// The bottom of the message list each tile shows, in points.
const TILE_H: f32 = 320.0;

impl Strip {
    fn new() -> Strip {
        let gpu = rune::render::Gpu::headless();
        let renderer = rune::render::Renderer::new(&gpu.device, rune::render::OFFSCREEN_FORMAT);
        Strip {
            gpu,
            renderer,
            tiles: Vec::new(),
            tile: (0, 0),
        }
    }

    /// The last frame, cropped to the bottom of the message list.
    fn shoot(&mut self, h: &mut Harness) {
        let (w, hh) = WINDOW;
        let target = rune::render::Offscreen::new(&self.gpu.device, w, hh);
        self.renderer.set_srgb_blend(h.rt.srgb_blend());
        self.renderer.prepare(
            &self.gpu.device,
            &self.gpu.queue,
            &mut h.rt.text.atlas,
            &h.rt.instances,
            WINDOW,
        );
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.renderer
            .draw(&mut encoder, &target.view, h.rt.clear_color());
        target.copy(&mut encoder);
        self.gpu.queue.submit([encoder.finish()]);
        let px = target.read(&self.gpu.device);
        let list = h.rect("chat-messages").expect("the message list");
        let x0 = (list[0] * SCALE) as u32;
        let y1 = ((list[1] + list[3]) * SCALE) as u32;
        let (cw, ch) = ((list[2] * SCALE) as u32, (TILE_H * SCALE) as u32);
        let y0 = y1 - ch;
        let mut tile = Vec::with_capacity((cw * ch * 4) as usize);
        for y in y0..y1 {
            let row = ((y * w + x0) * 4) as usize;
            tile.extend_from_slice(&px[row..row + (cw * 4) as usize]);
        }
        self.tile = (cw, ch);
        self.tiles.push(tile);
    }

    /// The tiles in rows of `cols`, 8px apart on grey.
    fn write(&self, path: &str, cols: usize) {
        const GAP: u32 = 8;
        let (cw, ch) = self.tile;
        let cols = cols.min(self.tiles.len()) as u32;
        let rows = (self.tiles.len() as u32).div_ceil(cols);
        let (sw, sh) = (cols * cw + (cols - 1) * GAP, rows * ch + (rows - 1) * GAP);
        let mut img = [128u8, 128, 128, 255].repeat((sw * sh) as usize);
        for (i, tile) in self.tiles.iter().enumerate() {
            let (tx, ty) = (
                (i as u32 % cols) * (cw + GAP),
                (i as u32 / cols) * (ch + GAP),
            );
            for y in 0..ch {
                let dst = (((ty + y) * sw + tx) * 4) as usize;
                let src = (y * cw * 4) as usize;
                img[dst..dst + (cw * 4) as usize]
                    .copy_from_slice(&tile[src..src + (cw * 4) as usize]);
            }
        }
        rune::render::write_png(std::path::Path::new(path), sw, sh, &img).expect("write the strip");
    }
}

/// Visual proof, not a check: frame strips of a reply growing in with its words fading
/// (`/tmp/roostr-motion/chat-growth.png`) and of a sent line gliding open
/// (`/tmp/roostr-motion/chat-growth-sent.png`), the bottom of the chat pane at 60 fps.
#[test]
#[ignore = "writes frame strips to /tmp/roostr-motion; needs a GPU"]
fn chat_growth_strip() {
    let (vault, mut h) = open_chat();
    let mut strip = Strip::new();
    strip.shoot(&mut h);
    reply_arrives(&vault, "r1");
    h.frames_until("the reply mounting", |h| h.has("msg-r1"));
    let mut at = 0;
    for frame in [0, 3, 6, 9, 12, 16, 20, 26, 34] {
        while at < frame {
            h.frame();
            at += 1;
        }
        strip.shoot(&mut h);
    }
    h.advance(Duration::from_secs(3));
    strip.shoot(&mut h);
    strip.write("/tmp/roostr-motion/chat-growth.png", 6);

    strip.tiles.clear();
    h.type_text("Ship it Monday, and tell the first cohort tonight.");
    strip.shoot(&mut h);
    h.key(Key::Enter, Mods::default());
    strip.shoot(&mut h);
    for _ in 0..8 {
        for _ in 0..2 {
            h.frame();
        }
        strip.shoot(&mut h);
    }
    h.settle();
    strip.shoot(&mut h);
    strip.write("/tmp/roostr-motion/chat-growth-sent.png", 6);
}
