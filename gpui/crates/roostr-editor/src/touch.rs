//! A page on a touch screen (the iPhone app), as Anytype's iOS editor has it
//! (anytype-swift `EditorPageController.handleLongPress`, `BlocksSelectionOverlayView`): a drag
//! scrolls - nothing on the page takes the finger - and holding a line selects it, except the
//! line being typed in, whose hold is the system's text menu (Paste, Select). While lines are
//! selected a bar at the bottom acts on them (Copy, Cut, Paste after, Duplicate, Delete, More:
//! the block menu, Select all, Done), a tap on a line adds or removes it, and a tap on the page
//! around them, or Done, ends the selection.
//!
//! The hold is watched without taking the finger: a row's hover is the finger on it, and the
//! hold only counts when the page didn't move under it (a scroll).

use std::time::Duration;

use roostr_core::State;
use roostr_rt::Task;
use roostr_ui::Theme;
use roostr_ui::ty::{self, Weights as _};
use rune::El;

use crate::editor::{Editor, update_editor, with_editor};

/// A finger held still this long on a line selects it (Anytype's `minimumPressDuration`).
pub(crate) const HOLD: Duration = Duration::from_millis(300);
/// How far (points) the page may move under a held finger before the hold is a scroll.
const HOLD_SLOP: f32 = 8.;

/// A finger down on lines: every row it is over (nested rows all are), where the page was
/// then, and the timer that selects.
pub(crate) struct Hold {
    ids: Vec<String>,
    page_y: f32,
    _timer: Task<()>,
}

/// The iPhone app (a touch screen is the pointer).
#[cfg(not(test))]
pub(crate) fn on() -> bool {
    roostr_core::layout::is_touch()
}

#[cfg(test)]
thread_local! {
    /// Tests stand in for a touch screen.
    pub(crate) static TOUCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn on() -> bool {
    TOUCH.with(std::cell::Cell::get)
}

/// Lines are selected on a touch screen: taps toggle them instead of placing the caret.
pub(crate) fn selecting(e: &Editor) -> bool {
    on() && !e.selected.is_empty()
}

/// A block row on a touch screen: held, it selects; tapped while selecting, it toggles; tapped
/// beside its text otherwise, the caret goes to its end.
pub(crate) fn row(r: El, object_id: &str, id: &str) -> El {
    let (ho, hi) = (object_id.to_string(), id.to_string());
    let (po, pi) = (object_id.to_string(), id.to_string());
    r.on_hover_change(move |s: &mut State, inside: bool, _| {
        with_editor(s, &ho, |e, s| {
            if inside {
                e.hold_enter(&hi, s);
            } else {
                e.hold_leave(&hi);
            }
        });
    })
    .on_press(move |s: &mut State| {
        with_editor(s, &po, |e, s| e.row_tap(&pi, s));
    })
}

/// The page background on a touch screen: a tap ends a selection, else puts the caret on the
/// nearest line (or a new one below the last).
pub(crate) fn background(body: El, object_id: &str) -> El {
    let (mo, po) = (object_id.to_string(), object_id.to_string());
    body.on_pointer_move(move |s: &mut State, p| {
        with_editor(s, &mo, |e, _| e.touch_y = p.y);
    })
    .on_press(move |s: &mut State| {
        with_editor(s, &po, |e, s| e.background_tap(s));
    })
}

impl Editor {
    /// The finger is on row `id`: start the hold, or add the row to the one under way.
    pub(crate) fn hold_enter(&mut self, id: &str, s: &mut State) {
        if self.read_only {
            return;
        }
        if let Some(hold) = &mut self.hold {
            if !hold.ids.iter().any(|x| x == id) {
                hold.ids.push(id.to_string());
            }
            return;
        }
        // A new finger down: whatever the last hold's release did, it is over.
        self.swallow_tap = None;
        let oid = self.object_id.clone();
        let timer = s.rt.timer(HOLD);
        let task = s.rt.spawn(async move |cx| {
            timer.await;
            update_editor(&cx, &oid, |e, s| e.held(s));
        });
        self.hold = Some(Hold {
            ids: vec![id.to_string()],
            page_y: self.bounds[1],
            _timer: task,
        });
    }

    /// The finger left row `id` (lifted, or slid off): once it is on none, no hold.
    pub(crate) fn hold_leave(&mut self, id: &str) {
        if let Some(hold) = &mut self.hold {
            hold.ids.retain(|x| x != id);
            if hold.ids.is_empty() {
                self.hold = None;
            }
        }
    }

    /// The hold's time passed with the page still: the innermost row under the finger is
    /// selected - or, already selected, the selection's block menu opens.
    pub(crate) fn held(&mut self, s: &mut State) {
        let Some(hold) = self.hold.take() else {
            return;
        };
        if (self.bounds[1] - hold.page_y).abs() > HOLD_SLOP {
            return;
        }
        // On the line being typed in: the system's text menu (Paste…) has this hold.
        if let Some(active) = &self.active
            && hold.ids.iter().any(|id| id == active)
        {
            return;
        }
        let depth = |id: &String| {
            let mut n = 0;
            let mut cur = id.clone();
            while let Some(p) = crate::doc::parent_of(&self.blocks, &cur) {
                n += 1;
                cur = p.id.clone();
            }
            n
        };
        let Some(id) = hold.ids.iter().max_by_key(|id| depth(id)).cloned() else {
            return;
        };
        let row = self.selection_row_of(&id);
        self.swallow_tap = Some(id.clone());
        if self.selected.iter().any(|x| *x == row) {
            let at = self
                .rows
                .get(&row)
                .map_or((self.bounds[0] + 24., self.bounds[1] + 24.), |g| {
                    (g.bounds[0] + 24., g.bounds[1] + g.bounds[3])
                });
            self.open_block_menu(&row, at, s);
            return;
        }
        self.selected.push(row);
        self.deactivate_for_selection(s);
    }

    /// A tap on row `id`: toggles it while selecting; otherwise (beside its text) the caret
    /// goes to its end.
    pub(crate) fn row_tap(&mut self, id: &str, s: &mut State) {
        // The release of the hold that just selected this row is no tap.
        if self.swallow_tap.take().is_some_and(|held| held == id) || self.read_only {
            return;
        }
        if !self.selected.is_empty() {
            let row = self.selection_row_of(id);
            self.toggle_select(&row, s);
            return;
        }
        if let Some(len) = self.text_of(id).map(|t| t.text.len()) {
            self.focus_block(id, len, s);
        }
    }

    /// A tap on the page around the lines: ends a selection, else the caret goes to the
    /// nearest line (or a new one below the last).
    pub(crate) fn background_tap(&mut self, s: &mut State) {
        self.swallow_tap = None;
        if !self.selected.is_empty() {
            self.selected.clear();
            return;
        }
        if !self.read_only {
            self.background_click(self.touch_y, s);
        }
    }
}

/// One action of the selection bar.
fn bar_button(t: &Theme, key: String, label: &str) -> El {
    rune::row()
        .key(key)
        .role("button")
        .label(label.to_string())
        .px(12.)
        .py(10.)
        .radius(8.)
        .center()
        .press(t.hover)
        .child(
            rune::text(label.to_string())
                .font(ty::ui(14.).medium())
                .color(t.fg),
        )
}

/// The selection bar along the bottom of the window while lines are selected (touch).
pub(crate) fn selection_bar(e: &Editor, t: &Theme, win_w: f32, win_h: f32) -> El {
    let oid = e.object_id.clone();
    let count = e.selected.len();
    let act = |name: &str, label: &str, f: fn(&mut Editor, &mut State, &mut rune::Tx)| {
        let o = oid.clone();
        bar_button(t, format!("ed-{oid}-sel-{name}"), label).on_press_tx(
            move |s: &mut State, tx| {
                with_editor(s, &o, |e, s| f(e, s, tx));
            },
        )
    };
    let actions = rune::row()
        .key(format!("ed-{oid}-sel-actions"))
        .scroll_x()
        .grow(1.)
        .min_w(0.)
        .child(
            rune::row()
                .items_center()
                .gap(2.)
                .child(act("copy", "Copy", |e, s, tx| e.copy_blocks(false, s, tx)))
                .child(act("cut", "Cut", |e, s, tx| e.copy_blocks(true, s, tx)))
                .child(act("paste", "Paste", |e, _, tx| {
                    let oid = e.object_id.clone();
                    tx.read_clipboard(move |s: &mut State, content, _| {
                        with_editor(s, &oid, |e, s| e.paste_after_selection(content, s));
                    });
                }))
                .child(act("duplicate", "Duplicate", |e, s, _| {
                    for id in e.topmost_selected().into_iter().rev() {
                        e.duplicate_block(&id, s);
                    }
                }))
                .child(act("delete", "Delete", |e, s, _| {
                    e.delete_selection_blocks(s)
                }))
                .child(act("more", "More", |e, s, _| {
                    if let Some(first) = e.selected.first().cloned() {
                        let at = e
                            .rows
                            .get(&first)
                            .map_or((e.bounds[0] + 24., e.bounds[1] + 24.), |g| {
                                (g.bounds[0] + 24., g.bounds[1] + g.bounds[3])
                            });
                        e.open_block_menu(&first, at, s);
                    }
                }))
                .child(act("all", "Select all", |e, s, _| {
                    e.selected = e.root_ids();
                    e.deactivate_for_selection(s);
                })),
        );
    let done_oid = oid.clone();
    rune::row()
        .key(format!("ed-{oid}-sel-bar"))
        .absolute(0., win_h - 64.)
        .w(win_w)
        .h(64.)
        .items_center()
        .gap(6.)
        .px(10.)
        .fill(t.bg)
        .border_sides([1., 0., 0., 0.], t.border)
        .border_layout()
        .child(
            rune::text(format!("{count} selected"))
                .font(ty::ui(13.))
                .color(t.muted),
        )
        .child(actions)
        .child(
            bar_button(t, format!("ed-{oid}-sel-done"), "Done").on_press(move |s: &mut State| {
                with_editor(s, &done_oid, |e, _| e.selected.clear());
            }),
        )
}
