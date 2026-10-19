//! Shared mutable state: `Arc<Mutex>` on native (futures are `Send`),
//! `Rc<RefCell>` in the browser (one thread; web handles are not `Send`).

#[cfg(not(target_family = "wasm"))]
mod imp {
    pub(crate) type Ptr<T> = std::sync::Arc<T>;
    pub(crate) type WeakPtr<T> = std::sync::Weak<T>;

    pub(crate) struct Locked<T>(parking_lot::Mutex<T>);

    impl<T> Locked<T> {
        pub(crate) fn new(value: T) -> Self {
            Self(parking_lot::Mutex::new(value))
        }

        /// Never call back into code that may take this lock again.
        pub(crate) fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
            f(&mut self.0.lock())
        }
    }

    pub(crate) fn downgrade<T>(ptr: &Ptr<T>) -> WeakPtr<T> {
        std::sync::Arc::downgrade(ptr)
    }
}

#[cfg(target_family = "wasm")]
mod imp {
    pub(crate) type Ptr<T> = std::rc::Rc<T>;
    pub(crate) type WeakPtr<T> = std::rc::Weak<T>;

    pub(crate) struct Locked<T>(std::cell::RefCell<T>);

    impl<T> Locked<T> {
        pub(crate) fn new(value: T) -> Self {
            Self(std::cell::RefCell::new(value))
        }

        /// Never call back into code that may take this lock again.
        pub(crate) fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
            f(&mut self.0.borrow_mut())
        }
    }

    pub(crate) fn downgrade<T>(ptr: &Ptr<T>) -> WeakPtr<T> {
        std::rc::Rc::downgrade(ptr)
    }
}

pub(crate) use imp::{Locked, Ptr, WeakPtr, downgrade};
