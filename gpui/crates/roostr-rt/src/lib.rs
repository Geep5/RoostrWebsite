//! Roostr's async runtime on Rune.
//!
//! Rune keeps the whole app in one state value `S` and hands it to handlers as `&mut S`.
//! This crate lets async code reach that state the way GPUI's `AsyncApp` did:
//!
//! - [`Executor::spawn`] runs a future on the UI thread. Rune's `poll` hook calls
//!   [`Executor::run`] each frame; between awaits the future calls [`AsyncCx::update`] to
//!   read and change the state.
//! - [`Executor::background`] runs blocking work (ureq requests) on a worker thread on the
//!   desktop. The browser has one thread, so there it is just another UI-thread future.
//! - [`Executor::timer`] completes after a duration of the executor's clock, which `run`
//!   reports so the poll hook can ask Rune to wake it (`Tx::wake_after`).
//!
//! A [`Task`] is the handle to a spawned future: await it for the output, drop it to cancel
//! the future, or [`Task::detach`] it to let it run to the end.

use std::cell::{Cell, RefCell};
use std::collections::{BinaryHeap, HashMap};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use futures::Stream;
use web_time::Instant;

/// Rune's `Waker`: asks the window for a frame, whose `poll` runs the executor.
pub type WakeFn = Arc<dyn Fn() + Send + Sync>;

#[cfg(not(target_family = "wasm"))]
mod bounds {
    /// `Send` on the desktop, nothing in the browser.
    pub trait MaybeSend: Send {}
    impl<T: Send + ?Sized> MaybeSend for T {}

    /// `Sync` on the desktop, nothing in the browser.
    pub trait MaybeSync: Sync {}
    impl<T: Sync + ?Sized> MaybeSync for T {}

    pub type BoxFuture<T> = std::pin::Pin<Box<dyn Future<Output = T> + Send>>;
    pub type BoxStream<T> = std::pin::Pin<Box<dyn futures::Stream<Item = T> + Send>>;
}

#[cfg(target_family = "wasm")]
mod bounds {
    /// `Send` on the desktop, nothing in the browser.
    pub trait MaybeSend {}
    impl<T: ?Sized> MaybeSend for T {}

    /// `Sync` on the desktop, nothing in the browser.
    pub trait MaybeSync {}
    impl<T: ?Sized> MaybeSync for T {}

    pub type BoxFuture<T> = std::pin::Pin<Box<dyn Future<Output = T>>>;
    pub type BoxStream<T> = std::pin::Pin<Box<dyn futures::Stream<Item = T>>>;
}

pub use bounds::{BoxFuture, BoxStream, MaybeSend, MaybeSync};

/// A stream polled on the UI thread.
pub type LocalBoxStream<T> = Pin<Box<dyn Stream<Item = T>>>;

/// Box a future as a [`BoxFuture`].
pub fn boxed<F>(future: F) -> BoxFuture<F::Output>
where
    F: Future + MaybeSend + 'static,
{
    Box::pin(future)
}

/// Box a stream as a [`BoxStream`].
pub fn boxed_stream<S>(stream: S) -> BoxStream<S::Item>
where
    S: Stream + MaybeSend + 'static,
{
    Box::pin(stream)
}

/// What one [`Executor::run`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ran {
    /// A future changed the state (called [`AsyncCx::update`]): the view must re-run.
    pub updated: bool,
    /// How long until the earliest pending timer is due, if any: the poll hook asks Rune to
    /// wake it then.
    pub next_timer: Option<Duration>,
}

/// Rune's poll hook passes this many rounds before yielding the frame, so a future that wakes
/// itself forever cannot freeze the window.
const MAX_ROUNDS: usize = 64;

type LocalFuture = Pin<Box<dyn Future<Output = ()>>>;

/// Reachable from wakers on any thread.
struct Shared {
    ready: Mutex<Vec<u64>>,
    wake: Mutex<Option<WakeFn>>,
}

impl Shared {
    fn wake(&self, id: u64) {
        self.ready.lock().push(id);
        self.ring();
    }

    fn ring(&self) {
        let wake = self.wake.lock().clone();
        if let Some(wake) = wake {
            wake();
        }
    }
}

struct TaskWaker {
    id: u64,
    shared: Arc<Shared>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.shared.wake(self.id);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.shared.wake(self.id);
    }
}

struct Timer {
    due: Instant,
    seq: u64,
    waker: Waker,
}

impl PartialEq for Timer {
    fn eq(&self, other: &Self) -> bool {
        self.due == other.due && self.seq == other.seq
    }
}

impl Eq for Timer {}

impl PartialOrd for Timer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Timer {
    /// Reversed: the heap's top is the earliest deadline.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (other.due, other.seq).cmp(&(self.due, self.seq))
    }
}

struct Inner<S> {
    /// The state while [`Executor::run`] polls; null otherwise.
    state: Cell<*mut S>,
    updating: Cell<bool>,
    updated: Cell<bool>,
    next_id: Cell<u64>,
    /// Each task's future and waker. A future being polled is out of the map, so it may spawn
    /// or cancel other tasks.
    tasks: RefCell<HashMap<u64, (LocalFuture, Waker)>>,
    shared: Arc<Shared>,
    timers: RefCell<BinaryHeap<Timer>>,
    /// Added to the real clock: tests move time forward without waiting.
    skew: Cell<Duration>,
}

/// Runs the app's futures. Clones share one executor; keep one in the state so handlers can
/// spawn (`s.rt.spawn(..)`).
pub struct Executor<S: 'static> {
    inner: Rc<Inner<S>>,
}

impl<S: 'static> Clone for Executor<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<S: 'static> Default for Executor<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: 'static> Executor<S> {
    pub fn new() -> Self {
        Self {
            inner: Rc::new(Inner {
                state: Cell::new(std::ptr::null_mut()),
                updating: Cell::new(false),
                updated: Cell::new(false),
                next_id: Cell::new(0),
                tasks: RefCell::new(HashMap::new()),
                shared: Arc::new(Shared {
                    ready: Mutex::new(Vec::new()),
                    wake: Mutex::new(None),
                }),
                timers: RefCell::new(BinaryHeap::new()),
                skew: Cell::new(Duration::ZERO),
            }),
        }
    }

    /// Rune's waker, from `AppSpec::setup`. Without one, futures still run on each `run`.
    pub fn set_waker(&self, wake: WakeFn) {
        *self.inner.shared.wake.lock() = Some(wake);
        if !self.inner.shared.ready.lock().is_empty() {
            self.inner.shared.ring();
        }
    }

    /// A handle for async code.
    pub fn cx(&self) -> AsyncCx<S> {
        AsyncCx {
            executor: self.clone(),
        }
    }

    /// The executor's clock: real time, plus what [`Self::advance`] added.
    pub fn now(&self) -> Instant {
        Instant::now() + self.inner.skew.get()
    }

    /// Moves the clock forward (tests), so timers fire without waiting.
    pub fn advance(&self, by: Duration) {
        self.inner.skew.set(self.inner.skew.get() + by);
        self.inner.shared.ring();
    }

    /// Whether any future is still pending.
    pub fn is_idle(&self) -> bool {
        self.inner.tasks.borrow().is_empty()
    }

    /// Runs `f(cx)`'s future on the UI thread. It starts on the next [`Self::run`].
    pub fn spawn<T, Fut>(&self, f: impl FnOnce(AsyncCx<S>) -> Fut) -> Task<T>
    where
        T: 'static,
        Fut: Future<Output = T> + 'static,
    {
        self.spawn_future(f(self.cx()))
    }

    fn spawn_future<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T> {
        let id = self.inner.next_id.get();
        self.inner.next_id.set(id + 1);
        let slot: Rc<RefCell<Slot<T>>> = Rc::new(RefCell::new(Slot::default()));
        let out = slot.clone();
        let future: LocalFuture = Box::pin(async move {
            let value = future.await;
            let mut slot = out.borrow_mut();
            slot.value = Some(value);
            if let Some(waker) = slot.waker.take() {
                waker.wake();
            }
        });
        let waker = Waker::from(Arc::new(TaskWaker {
            id,
            shared: self.inner.shared.clone(),
        }));
        self.inner.tasks.borrow_mut().insert(id, (future, waker));
        self.inner.shared.wake(id);
        let cancel = Rc::downgrade(&self.inner);
        Task(TaskInner::Local {
            slot,
            cancel: Some(Box::new(move || {
                if let Some(inner) = cancel.upgrade() {
                    let gone = inner.tasks.borrow_mut().remove(&id);
                    drop(gone);
                }
            })),
        })
    }

    /// Runs blocking work off the UI: a worker thread on the desktop, the UI thread's
    /// executor in the browser.
    pub fn background<T>(&self, future: impl Future<Output = T> + MaybeSend + 'static) -> Task<T>
    where
        T: MaybeSend + 'static,
    {
        #[cfg(not(target_family = "wasm"))]
        {
            let slot: Arc<Mutex<Slot<T>>> = Arc::new(Mutex::new(Slot::default()));
            let out = slot.clone();
            pool::run(Box::new(move || {
                let value = futures::executor::block_on(future);
                let mut slot = out.lock();
                slot.value = Some(value);
                if let Some(waker) = slot.waker.take() {
                    waker.wake();
                }
            }));
            Task(TaskInner::Thread(slot))
        }
        #[cfg(target_family = "wasm")]
        {
            self.spawn_future(future)
        }
    }

    /// Completes `duration` from now on [`Self::now`]'s clock.
    pub fn timer(&self, duration: Duration) -> Task<()> {
        let due = self.now() + duration;
        let inner = Rc::downgrade(&self.inner);
        self.spawn_future(futures::future::poll_fn(move |cx| {
            let Some(inner) = inner.upgrade() else {
                return Poll::Ready(());
            };
            if Instant::now() + inner.skew.get() >= due {
                return Poll::Ready(());
            }
            let seq = inner.next_id.get();
            inner.next_id.set(seq + 1);
            inner.timers.borrow_mut().push(Timer {
                due,
                seq,
                waker: cx.waker().clone(),
            });
            Poll::Pending
        }))
    }

    /// Polls every woken future with `state` reachable through [`AsyncCx::update`]. Call it
    /// from Rune's `poll` hook; return [`Ran::updated`] from the hook and ask Rune to wake at
    /// [`Ran::next_timer`].
    pub fn run(&self, state: &mut S) -> Ran {
        let inner = &*self.inner;
        let previous = inner.state.replace(state as *mut S);
        for round in 0.. {
            self.fire_timers();
            let mut ready = std::mem::take(&mut *inner.shared.ready.lock());
            if ready.is_empty() {
                break;
            }
            if round == MAX_ROUNDS {
                inner.shared.ready.lock().append(&mut ready);
                inner.shared.ring();
                break;
            }
            ready.sort_unstable();
            ready.dedup();
            for id in ready {
                let Some((mut future, waker)) = inner.tasks.borrow_mut().remove(&id) else {
                    continue;
                };
                let mut cx = Context::from_waker(&waker);
                if future.as_mut().poll(&mut cx).is_pending() {
                    inner.tasks.borrow_mut().insert(id, (future, waker));
                }
            }
        }
        inner.state.set(previous);
        let now = self.now();
        Ran {
            updated: inner.updated.replace(false),
            next_timer: inner
                .timers
                .borrow()
                .peek()
                .map(|t| t.due.saturating_duration_since(now)),
        }
    }

    fn fire_timers(&self) {
        let now = self.now();
        loop {
            let due = {
                let mut timers = self.inner.timers.borrow_mut();
                match timers.peek() {
                    Some(t) if t.due <= now => timers.pop(),
                    _ => None,
                }
            };
            match due {
                Some(timer) => timer.waker.wake(),
                None => break,
            }
        }
    }
}

/// What async code holds: the state between awaits, and spawning.
pub struct AsyncCx<S: 'static> {
    executor: Executor<S>,
}

impl<S: 'static> Clone for AsyncCx<S> {
    fn clone(&self) -> Self {
        Self {
            executor: self.executor.clone(),
        }
    }
}

impl<S: 'static> AsyncCx<S> {
    /// Runs `f` with the app state. Only from a future the executor is polling (anything
    /// spawned on it); a nested call is a bug and panics.
    pub fn update<R>(&self, f: impl FnOnce(&mut S) -> R) -> R {
        let inner = &*self.executor.inner;
        let state = inner.state.get();
        assert!(!state.is_null(), "AsyncCx::update outside Executor::run");
        assert!(
            !inner.updating.replace(true),
            "AsyncCx::update inside another update"
        );
        // SAFETY: `run` set the pointer from its `&mut S` and holds that borrow, unused, until
        // every future it polls has returned; the flag above rules out a second `&mut S`.
        let out = f(unsafe { &mut *state });
        inner.updating.set(false);
        inner.updated.set(true);
        out
    }

    /// Reads the app state.
    pub fn read<R>(&self, f: impl FnOnce(&S) -> R) -> R {
        let inner = &*self.executor.inner;
        let state = inner.state.get();
        assert!(!state.is_null(), "AsyncCx::read outside Executor::run");
        assert!(!inner.updating.get(), "AsyncCx::read inside an update");
        // SAFETY: as in `update`; a shared borrow while no `&mut S` exists.
        f(unsafe { &*state })
    }

    pub fn executor(&self) -> &Executor<S> {
        &self.executor
    }

    pub fn spawn<T, Fut>(&self, f: impl FnOnce(AsyncCx<S>) -> Fut) -> Task<T>
    where
        T: 'static,
        Fut: Future<Output = T> + 'static,
    {
        self.executor.spawn(f)
    }

    pub fn background<T>(&self, future: impl Future<Output = T> + MaybeSend + 'static) -> Task<T>
    where
        T: MaybeSend + 'static,
    {
        self.executor.background(future)
    }

    pub fn timer(&self, duration: Duration) -> Task<()> {
        self.executor.timer(duration)
    }

    pub fn now(&self) -> Instant {
        self.executor.now()
    }
}

struct Slot<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            value: None,
            waker: None,
        }
    }
}

type Cancel = Box<dyn FnOnce()>;

enum TaskInner<T> {
    Ready(Option<T>),
    Local {
        slot: Rc<RefCell<Slot<T>>>,
        cancel: Option<Cancel>,
    },
    #[cfg(not(target_family = "wasm"))]
    Thread(Arc<Mutex<Slot<T>>>),
}

/// A spawned future's output. Dropping it cancels a UI-thread future (background work runs on
/// but its output is dropped); [`Task::detach`] lets it finish.
#[must_use = "dropping a Task cancels it; call detach() to let it run"]
pub struct Task<T>(TaskInner<T>);

impl<T> Task<T> {
    /// A task that is already done.
    pub fn ready(value: T) -> Self {
        Task(TaskInner::Ready(Some(value)))
    }

    /// Lets the future run to the end with no one awaiting it.
    pub fn detach(mut self) {
        if let TaskInner::Local { cancel, .. } = &mut self.0 {
            cancel.take();
        }
    }
}

impl<T: 'static, E: std::fmt::Display + 'static> Task<Result<T, E>> {
    /// Lets the future run to the end and logs its error, if any (GPUI's
    /// `detach_and_log_err`).
    #[track_caller]
    pub fn detach_and_log_err<S: 'static>(self, rt: &Executor<S>) {
        let at = std::panic::Location::caller();
        rt.spawn(async move |_| {
            if let Err(error) = self.await {
                log::warn!("{}:{}: {error}", at.file(), at.line());
            }
        })
        .detach();
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        if let TaskInner::Local { slot, cancel } = &mut self.0
            && slot.borrow().value.is_none()
            && let Some(cancel) = cancel.take()
        {
            cancel();
        }
    }
}

impl<T> Unpin for Task<T> {}

impl<T> Future for Task<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let this = self.get_mut();
        match &mut this.0 {
            TaskInner::Ready(value) => {
                Poll::Ready(value.take().expect("Task polled after completion"))
            }
            TaskInner::Local { slot, .. } => {
                let mut slot = slot.borrow_mut();
                match slot.value.take() {
                    Some(value) => Poll::Ready(value),
                    None => {
                        slot.waker = Some(cx.waker().clone());
                        Poll::Pending
                    }
                }
            }
            #[cfg(not(target_family = "wasm"))]
            TaskInner::Thread(slot) => {
                let mut slot = slot.lock();
                match slot.value.take() {
                    Some(value) => Poll::Ready(value),
                    None => {
                        slot.waker = Some(cx.waker().clone());
                        Poll::Pending
                    }
                }
            }
        }
    }
}

/// A stream whose items need blocking reads (an SSE body over ureq), polled on its own thread
/// on the desktop and handed to the UI thread over a channel. In the browser it is the stream
/// itself. The thread ends at the stream's end or the first item after the receiver is gone.
pub fn background_stream<T: MaybeSend + 'static>(stream: BoxStream<T>) -> LocalBoxStream<T> {
    #[cfg(not(target_family = "wasm"))]
    {
        use futures::StreamExt as _;
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let spawned = std::thread::Builder::new()
            .name("roostr-stream".into())
            .spawn(move || {
                let mut stream = stream;
                while let Some(item) = futures::executor::block_on(stream.next()) {
                    if tx.unbounded_send(item).is_err() {
                        break;
                    }
                }
            });
        if let Err(error) = spawned {
            panic!("no thread for a background stream: {error}");
        }
        Box::pin(rx)
    }
    #[cfg(target_family = "wasm")]
    {
        stream
    }
}

#[cfg(not(target_family = "wasm"))]
mod pool {
    use std::sync::mpsc::{Sender, channel};
    use std::sync::{Arc, LazyLock};

    use parking_lot::Mutex;

    pub type Job = Box<dyn FnOnce() + Send>;

    /// Blocking requests wait on the network, not the CPU: enough threads that a refresh's
    /// parallel requests never queue behind each other.
    const THREADS: usize = 16;

    static POOL: LazyLock<Mutex<Sender<Job>>> = LazyLock::new(|| {
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..THREADS {
            let rx = rx.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("roostr-bg-{i}"))
                .spawn(move || {
                    loop {
                        let job = rx.lock().recv();
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                });
            if let Err(error) = spawned {
                panic!("no background thread: {error}");
            }
        }
        Mutex::new(tx)
    });

    pub fn run(job: Job) {
        POOL.lock().send(job).expect("background pool gone");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct State {
        log: Vec<String>,
    }

    #[test]
    fn a_future_changes_the_state_between_awaits() {
        let ex = Executor::<State>::new();
        let mut s = State::default();
        let (tx, rx) = futures::channel::oneshot::channel::<u32>();
        let task = ex.spawn(async move |cx| {
            cx.update(|s| s.log.push("started".into()));
            let n = rx.await.unwrap();
            cx.update(|s| s.log.push(format!("got {n}")));
            n * 2
        });
        assert!(ex.run(&mut s).updated);
        assert_eq!(s.log, ["started"]);
        assert!(!ex.run(&mut s).updated, "nothing woke it");
        tx.send(21).unwrap();
        assert!(ex.run(&mut s).updated);
        assert_eq!(s.log, ["started", "got 21"]);
        let out = ex.spawn(async move |_| task.await);
        ex.run(&mut s);
        assert!(ex.is_idle());
        drop(out);
    }

    #[test]
    fn dropping_a_task_cancels_it_and_detach_lets_it_finish() {
        let ex = Executor::<State>::new();
        let mut s = State::default();
        let (tx1, rx1) = futures::channel::oneshot::channel::<()>();
        let (tx2, rx2) = futures::channel::oneshot::channel::<()>();
        let dropped = ex.spawn(async move |cx| {
            let _ = rx1.await;
            cx.update(|s| s.log.push("dropped one ran".into()));
        });
        ex.spawn(async move |cx| {
            let _ = rx2.await;
            cx.update(|s| s.log.push("detached one ran".into()));
        })
        .detach();
        ex.run(&mut s);
        drop(dropped);
        let _ = tx1.send(());
        let _ = tx2.send(());
        ex.run(&mut s);
        assert_eq!(s.log, ["detached one ran"]);
        assert!(ex.is_idle());
    }

    #[test]
    fn a_timer_fires_when_the_clock_reaches_it() {
        let ex = Executor::<State>::new();
        let mut s = State::default();
        ex.spawn(async move |cx| {
            cx.timer(Duration::from_secs(5)).await;
            cx.update(|s| s.log.push("fired".into()));
        })
        .detach();
        let ran = ex.run(&mut s);
        let wait = ran.next_timer.expect("a pending timer");
        assert!(
            wait > Duration::from_secs(4) && wait <= Duration::from_secs(5),
            "{wait:?}"
        );
        ex.advance(Duration::from_secs(3));
        assert!(!ex.run(&mut s).updated);
        ex.advance(Duration::from_secs(3));
        assert!(ex.run(&mut s).updated);
        assert_eq!(s.log, ["fired"]);
        assert_eq!(ex.run(&mut s).next_timer, None);
    }

    #[test]
    fn background_work_lands_on_the_ui_thread_and_wakes_it() {
        let ex = Executor::<State>::new();
        let mut s = State::default();
        let (woke_tx, woke_rx) = std::sync::mpsc::channel::<()>();
        let woke_tx = Mutex::new(woke_tx);
        ex.set_waker(Arc::new(move || {
            let _ = woke_tx.lock().send(());
        }));
        let main = std::thread::current().id();
        ex.spawn(async move |cx| {
            let (n, thread) = cx
                .background(async move { (6 * 7, std::thread::current().id()) })
                .await;
            cx.update(|s| s.log.push(format!("{n} {}", thread != main)));
        })
        .detach();
        loop {
            ex.run(&mut s);
            if !s.log.is_empty() {
                break;
            }
            woke_rx.recv().unwrap();
        }
        assert_eq!(s.log, ["42 true"]);
    }

    #[test]
    fn a_background_stream_delivers_every_item_in_order() {
        use futures::StreamExt as _;
        let items = background_stream(boxed_stream(futures::stream::iter(0..100)));
        let got: Vec<i32> = futures::executor::block_on(items.collect());
        assert_eq!(got, (0..100).collect::<Vec<_>>());
    }
}
