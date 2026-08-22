//! Retained application state.
//!
//! The element tree is rebuilt every frame, so it cannot own anything that has
//! to outlive a frame. A [`Model`] is the other half of that arrangement: state
//! lives here, the tree reads it while building, and event handlers hold a
//! clone and write to it.
//!
//! Models are single threaded on purpose. A user interface runs on one thread,
//! and paying for atomics on every read of a counter is not a trade worth
//! making. Work that needs other threads sends its results back to the UI
//! thread instead.

use std::cell::{Ref, RefCell, RefMut};
use std::fmt;
use std::rc::Rc;

/// A shared, mutable piece of application state.
///
/// Cloning is a refcount bump, which is what makes it cheap to hand a clone to
/// every event handler that needs it.
pub struct Model<T> {
    inner: Rc<RefCell<T>>,
}

impl<T> Model<T> {
    pub fn new(value: T) -> Self {
        Model {
            inner: Rc::new(RefCell::new(value)),
        }
    }

    /// Borrow the value.
    ///
    /// Panics if a write borrow is active. In practice that means not calling
    /// `update` from inside `read` on the same model.
    #[inline]
    pub fn read(&self) -> Ref<'_, T> {
        self.inner.borrow()
    }

    /// Borrow the value mutably.
    #[inline]
    pub fn write(&self) -> RefMut<'_, T> {
        self.inner.borrow_mut()
    }

    /// Mutate the value and return whatever the closure produces.
    #[inline]
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        f(&mut self.inner.borrow_mut())
    }

    /// Read the value and return whatever the closure produces.
    #[inline]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.inner.borrow())
    }

    /// Whether two handles refer to the same state.
    #[inline]
    pub fn same(&self, other: &Model<T>) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }
}

impl<T: Clone> Model<T> {
    /// A copy of the current value.
    #[inline]
    pub fn get(&self) -> T {
        self.inner.borrow().clone()
    }

    /// Replace the value.
    #[inline]
    pub fn set(&self, value: T) {
        *self.inner.borrow_mut() = value;
    }

    /// Replace the value and return the old one.
    #[inline]
    pub fn replace(&self, value: T) -> T {
        std::mem::replace(&mut self.inner.borrow_mut(), value)
    }
}

impl<T> Clone for Model<T> {
    fn clone(&self) -> Self {
        Model {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<T: Default> Default for Model<T> {
    fn default() -> Self {
        Model::new(T::default())
    }
}

impl<T: fmt::Debug> fmt::Debug for Model<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.inner.try_borrow() {
            Ok(value) => f.debug_tuple("Model").field(&*value).finish(),
            Err(_) => f.write_str("Model(<in use>)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_one_value() {
        let a = Model::new(1i32);
        let b = a.clone();
        b.update(|v| *v += 41);
        assert_eq!(a.get(), 42);
        assert!(a.same(&b));
    }

    #[test]
    fn distinct_models_are_not_the_same() {
        assert!(!Model::new(0).same(&Model::new(0)));
    }

    #[test]
    fn update_returns_the_closure_result() {
        let model = Model::new(vec![1, 2, 3]);
        let len = model.update(|v| {
            v.push(4);
            v.len()
        });
        assert_eq!(len, 4);
    }

    #[test]
    fn set_overwrites_and_replace_hands_back_the_old_value() {
        let model = Model::new(String::from("old"));
        model.set("new".into());
        assert_eq!(model.get(), "new");
        assert_eq!(model.replace("newer".into()), "new");
        assert_eq!(model.get(), "newer");
    }

    #[test]
    fn a_handler_style_capture_writes_through() {
        let count = Model::new(0i32);
        let captured = count.clone();
        let handler: Box<dyn Fn()> = Box::new(move || captured.update(|n| *n += 1));
        handler();
        handler();
        assert_eq!(count.get(), 2);
    }
}
