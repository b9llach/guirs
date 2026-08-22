//! Cheap strings and stable element identity.

use std::borrow::Borrow;
use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};
use std::ops::Deref;
use std::sync::Arc;

/// An immutable string that is cheap to clone.
///
/// Element ids, class names and label text are copied constantly during a
/// render pass. Backing them with an `Arc<str>` makes every one of those copies
/// a refcount bump instead of an allocation.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SharedString(Arc<str>);

impl SharedString {
    pub fn new(s: impl AsRef<str>) -> Self {
        SharedString(Arc::from(s.as_ref()))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether two shared strings point at the same allocation. A fast path for
    /// change detection that never yields a false positive.
    #[inline]
    pub fn ptr_eq(&self, other: &SharedString) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Default for SharedString {
    fn default() -> Self {
        SharedString(Arc::from(""))
    }
}

impl Deref for SharedString {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for SharedString {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for SharedString {
    #[inline]
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SharedString {
    fn from(s: &str) -> Self {
        SharedString(Arc::from(s))
    }
}

impl From<String> for SharedString {
    fn from(s: String) -> Self {
        SharedString(Arc::from(s.as_str()))
    }
}

impl From<&String> for SharedString {
    fn from(s: &String) -> Self {
        SharedString(Arc::from(s.as_str()))
    }
}

impl From<Arc<str>> for SharedString {
    fn from(s: Arc<str>) -> Self {
        SharedString(s)
    }
}

impl PartialEq<str> for SharedString {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for SharedString {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl fmt::Display for SharedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SharedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

// ---------------------------------------------------------------------------
// Element identity
// ---------------------------------------------------------------------------

/// One segment of an element's identity path.
///
/// Identity is what lets retained per element state (hover animation progress,
/// scroll offset, text selection) survive a rebuild of the element tree. An id
/// only has to be unique among its siblings, since the full path is what gets
/// hashed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ElementId {
    /// A developer supplied name, as in `.id("sidebar")`.
    Name(SharedString),
    /// Position among siblings. Assigned automatically when no name is given.
    Index(usize),
    /// An application supplied key, typically a database row id. Preferred for
    /// list items, because it stays stable when the list is reordered.
    Key(u64),
}

impl From<&'static str> for ElementId {
    fn from(s: &'static str) -> Self {
        ElementId::Name(SharedString::from(s))
    }
}

impl From<SharedString> for ElementId {
    fn from(s: SharedString) -> Self {
        ElementId::Name(s)
    }
}

impl From<String> for ElementId {
    fn from(s: String) -> Self {
        ElementId::Name(SharedString::from(s))
    }
}

impl From<usize> for ElementId {
    fn from(i: usize) -> Self {
        ElementId::Index(i)
    }
}

impl From<u64> for ElementId {
    fn from(k: u64) -> Self {
        ElementId::Key(k)
    }
}

impl fmt::Display for ElementId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ElementId::Name(n) => write!(f, "{n}"),
            ElementId::Index(i) => write!(f, "[{i}]"),
            ElementId::Key(k) => write!(f, "#{k}"),
        }
    }
}

/// A hash of a full identity path, stable across frames and across runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct GlobalElementId(pub u64);

impl GlobalElementId {
    /// The identity of the window root.
    pub const ROOT: GlobalElementId = GlobalElementId(0);

    /// Extend this path with one more segment.
    pub fn child(self, segment: &ElementId) -> GlobalElementId {
        let mut hasher = stable_hasher();
        self.0.hash(&mut hasher);
        segment.hash(&mut hasher);
        GlobalElementId(hasher.finish())
    }

    /// Derive a sub identity, for state a single element needs several slots
    /// for (a scroll view's horizontal and vertical bars, for instance).
    pub fn scoped(self, tag: &'static str) -> GlobalElementId {
        let mut hasher = stable_hasher();
        self.0.hash(&mut hasher);
        tag.hash(&mut hasher);
        GlobalElementId(hasher.finish())
    }
}

impl fmt::Display for GlobalElementId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// A hasher with a fixed seed.
///
/// The default `RandomState` reseeds per process, which would make element ids,
/// and therefore any state keyed by them, differ between runs. Everything that
/// needs a persistent identity hashes through this instead.
#[inline]
pub fn stable_hasher() -> impl Hasher {
    STABLE_SEED.build_hasher()
}

/// Hash any hashable value with the fixed seed.
#[inline]
pub fn stable_hash<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut hasher = stable_hasher();
    value.hash(&mut hasher);
    hasher.finish()
}

static STABLE_SEED: std::sync::LazyLock<ahash::RandomState> = std::sync::LazyLock::new(|| {
    ahash::RandomState::with_seeds(
        0x9e37_79b9_7f4a_7c15,
        0xbf58_476d_1ce4_e5b9,
        0x94d0_49bb_1331_11eb,
        0x2545_f491_4f6c_dd1d,
    )
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_strings_compare_by_value() {
        let a = SharedString::from("sidebar");
        let b = SharedString::from(String::from("sidebar"));
        assert_eq!(a, b);
        assert!(!a.ptr_eq(&b));
        assert_eq!(a, "sidebar");
    }

    #[test]
    fn identity_paths_are_order_sensitive() {
        let root = GlobalElementId::ROOT;
        let a = root.child(&ElementId::from("panel")).child(&ElementId::Index(0));
        let b = root.child(&ElementId::Index(0)).child(&ElementId::from("panel"));
        assert_ne!(a, b);
    }

    #[test]
    fn identity_is_reproducible() {
        let path = |()| {
            GlobalElementId::ROOT
                .child(&ElementId::from("root"))
                .child(&ElementId::Key(42))
        };
        assert_eq!(path(()), path(()));
    }

    #[test]
    fn scoped_ids_do_not_collide_with_children() {
        let base = GlobalElementId::ROOT.child(&ElementId::from("scroll"));
        assert_ne!(base.scoped("vertical"), base.scoped("horizontal"));
        assert_ne!(base.scoped("vertical"), base);
    }
}
