//! One ordered lane for an editor's writes (`src/lib/editor/write-queue.ts`).
//!
//! Every mutation the editor issues runs after the previous one settles, so
//! a debounced text save and a structural write (split, move, remove) can
//! never land out of order - the engine sees edits in the order the person
//! made them. A failed write does not stall the lane; its caller sees the
//! error.

use std::cell::Cell;
use std::rc::Rc;

use anyhow::{Result, anyhow};
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui_kit::{App, AppContext as _, Task};
use roostr_core::JsonMap;

type Work = Box<dyn FnOnce(&mut App) -> Task<Result<JsonMap>>>;

pub struct WriteQueue {
    tx: mpsc::UnboundedSender<(Work, oneshot::Sender<Result<JsonMap>>)>,
    issued: Rc<Cell<usize>>,
    _runner: Task<()>,
}

impl WriteQueue {
    pub fn new(cx: &mut App) -> Self {
        let (tx, mut rx) = mpsc::unbounded::<(Work, oneshot::Sender<Result<JsonMap>>)>();
        let runner = cx.spawn(async move |cx| {
            while let Some((work, reply)) = rx.next().await {
                let task = cx.update(work);
                let _ = reply.send(task.await);
            }
        });
        Self {
            tx,
            issued: Rc::new(Cell::new(0)),
            _runner: runner,
        }
    }

    /// Run `work` after every earlier write has settled.
    pub fn run(
        &self,
        cx: &App,
        work: impl FnOnce(&mut App) -> Task<Result<JsonMap>> + 'static,
    ) -> Task<Result<JsonMap>> {
        self.issued.set(self.issued.get() + 1);
        let (reply, done) = oneshot::channel();
        if self.tx.unbounded_send((Box::new(work), reply)).is_err() {
            return Task::ready(Err(anyhow!("the editor closed")));
        }
        cx.background_spawn(async move {
            done.await
                .unwrap_or_else(|_| Err(anyhow!("the editor closed")))
        })
    }

    /// Resolves once every write issued so far has settled.
    pub fn idle(&self, cx: &App) -> Task<()> {
        let (reply, done) = oneshot::channel();
        if self
            .tx
            .unbounded_send((Box::new(|_| Task::ready(Ok(JsonMap::new()))), reply))
            .is_err()
        {
            return Task::ready(());
        }
        cx.background_spawn(async move {
            let _ = done.await;
        })
    }

    /// How many writes have been issued: a read started before a later write may be stale.
    pub fn issued(&self) -> usize {
        self.issued.get()
    }
}
