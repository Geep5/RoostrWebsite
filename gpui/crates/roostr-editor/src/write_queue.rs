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
use roostr_core::{JsonMap, State};
use roostr_rt::Task;

type Work = Box<dyn FnOnce(&mut State) -> Task<Result<JsonMap>>>;

pub struct WriteQueue {
    tx: mpsc::UnboundedSender<(Work, oneshot::Sender<Result<JsonMap>>)>,
    issued: Rc<Cell<usize>>,
    runner: Option<Task<()>>,
}

impl WriteQueue {
    pub fn new(s: &State) -> Self {
        let (tx, mut rx) = mpsc::unbounded::<(Work, oneshot::Sender<Result<JsonMap>>)>();
        let runner = s.rt.spawn(async move |cx| {
            while let Some((work, reply)) = rx.next().await {
                let task = cx.update(work);
                let _ = reply.send(task.await);
            }
        });
        Self {
            tx,
            issued: Rc::new(Cell::new(0)),
            runner: Some(runner),
        }
    }

    /// Run `work` after every earlier write has settled.
    pub fn run(
        &self,
        s: &State,
        work: impl FnOnce(&mut State) -> Task<Result<JsonMap>> + 'static,
    ) -> Task<Result<JsonMap>> {
        self.issued.set(self.issued.get() + 1);
        let (reply, done) = oneshot::channel();
        if self.tx.unbounded_send((Box::new(work), reply)).is_err() {
            return Task::ready(Err(anyhow!("the editor closed")));
        }
        s.rt.spawn(async move |_| {
            done.await
                .unwrap_or_else(|_| Err(anyhow!("the editor closed")))
        })
    }

    /// Resolves once every write issued so far has settled.
    pub fn idle(&self, s: &State) -> Task<()> {
        let (reply, done) = oneshot::channel();
        if self
            .tx
            .unbounded_send((Box::new(|_| Task::ready(Ok(JsonMap::new()))), reply))
            .is_err()
        {
            return Task::ready(());
        }
        s.rt.spawn(async move |_| {
            let _ = done.await;
        })
    }

    /// How many writes have been issued: a read started before a later write may be stale.
    pub fn issued(&self) -> usize {
        self.issued.get()
    }

    /// The editor is going away: the writes already queued still land, in order.
    pub fn finish(mut self) {
        self.tx.close_channel();
        if let Some(runner) = self.runner.take() {
            runner.detach();
        }
    }
}
