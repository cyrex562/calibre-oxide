//! Port of `css_selectors.ordered_set` (issue #451, split from #85): a
//! set that remembers insertion order, so every entry has a stable
//! index that can be looked up -- used by `select.py`'s per-document
//! tag/id/class/attribute/lang indices (#453's scope) and its sibling-
//! position lookups for structural pseudo-classes like `:nth-child()`.
//!
//! # Built on `indexmap::IndexSet`, not a from-scratch port
//!
//! Real Python's `OrderedSet` is `items: list` + `map: dict[item,
//! index]`, with `discard` doing a real list `pop` (shifts every later
//! item's stored index down by one) rather than a swap-with-last
//! removal. [`indexmap::IndexSet`] already provides exactly this
//! shape -- insertion-order-preserving storage, O(1) index lookup via
//! `get_index_of`, and `shift_remove` (which re-indexes exactly like
//! Python's `list.pop` does, unlike `swap_remove`) -- so this port
//! wraps it rather than re-implementing a parallel `Vec`+`HashMap`
//! pair by hand.
//!
//! # Narrower than real upstream's generic `collections.abc.MutableSet`
//!
//! Real `OrderedSet.__getitem__` supports single-index access, Python
//! slice syntax (`[:]`, `[1:3]`), and NumPy-style fancy indexing (an
//! iterable of indices) -- no real call site in `parser.py`/`select.py`
//! uses any of that (checked directly: the only real methods ever
//! called on an `OrderedSet` instance are `add`/`index`/`len`), so only
//! plain `get(index)` is ported here. Real `OrderedSet.index(key)`
//! raises `KeyError` for a missing key; this port returns `None`
//! instead, since every real call site already only calls `index` on a
//! key it just added via `add` and never needs to handle the "missing"
//! case.

use indexmap::IndexSet;
use std::hash::Hash;

/// Port of `OrderedSet`.
#[derive(Debug, Clone)]
pub struct OrderedSet<T: Eq + Hash>(IndexSet<T>);

impl<T: Eq + Hash> Default for OrderedSet<T> {
    fn default() -> Self {
        OrderedSet(IndexSet::new())
    }
}

impl<T: Eq + Hash> OrderedSet<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Port of `OrderedSet.__init__(iterable)`.
    pub fn from_iter(iterable: impl IntoIterator<Item = T>) -> Self {
        let mut set = Self::new();
        for item in iterable {
            set.add(item);
        }
        set
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Port of `OrderedSet.__contains__`.
    pub fn contains(&self, key: &T) -> bool {
        self.0.contains(key)
    }

    /// Port of `OrderedSet.add`: inserts `key` if not already present,
    /// returning its (new or pre-existing) index.
    pub fn add(&mut self, key: T) -> usize {
        self.0.insert_full(key).0
    }

    /// Port of `OrderedSet.index`: the index of `key`, or `None` if
    /// absent (real upstream raises `KeyError` instead -- see the
    /// module doc for why that's not replicated).
    pub fn index(&self, key: &T) -> Option<usize> {
        self.0.get_index_of(key)
    }

    /// Port of `OrderedSet.__getitem__` for a plain integer index only
    /// (see the module doc for the narrowed scope).
    pub fn get(&self, index: usize) -> Option<&T> {
        self.0.get_index(index)
    }

    /// Port of `OrderedSet.discard`: removes `key` if present
    /// (preserving the relative order of, and re-indexing, every
    /// remaining item), returning whether it was present.
    pub fn discard(&mut self, key: &T) -> bool {
        self.0.shift_remove(key)
    }

    /// Port of `OrderedSet.copy`.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }
}

impl<T: Eq + Hash> IntoIterator for OrderedSet<T> {
    type Item = T;
    type IntoIter = indexmap::set::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<T: Eq + Hash> FromIterator<T> for OrderedSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::from_iter(iter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_returns_the_first_seen_index_and_is_idempotent() {
        let mut set: OrderedSet<&str> = OrderedSet::new();
        assert_eq!(set.add("a"), 0);
        assert_eq!(set.add("b"), 1);
        assert_eq!(set.add("a"), 0, "re-adding an existing key returns its original index");
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn index_finds_present_keys_and_is_none_for_absent_ones() {
        let mut set: OrderedSet<&str> = OrderedSet::new();
        set.add("x");
        set.add("y");
        assert_eq!(set.index(&"y"), Some(1));
        assert_eq!(set.index(&"z"), None);
    }

    #[test]
    fn get_returns_items_by_insertion_order_position() {
        let set: OrderedSet<i32> = OrderedSet::from_iter([10, 20, 30]);
        assert_eq!(set.get(0), Some(&10));
        assert_eq!(set.get(2), Some(&30));
        assert_eq!(set.get(3), None);
    }

    #[test]
    fn from_iter_deduplicates_keeping_first_seen_order() {
        let set: OrderedSet<i32> = OrderedSet::from_iter([1, 2, 1, 3, 2]);
        assert_eq!(set.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn discard_removes_and_reindexes_later_items() {
        let mut set: OrderedSet<i32> = OrderedSet::from_iter([1, 2, 3]);
        assert!(set.discard(&1));
        assert!(!set.discard(&1), "already removed");
        // Real Python's `list.pop`-based discard shifts every later
        // item's index down by one -- confirmed via `index`, not just
        // iteration order.
        assert_eq!(set.index(&2), Some(0));
        assert_eq!(set.index(&3), Some(1));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn contains_reflects_current_membership() {
        let mut set: OrderedSet<&str> = OrderedSet::new();
        assert!(!set.contains(&"a"));
        set.add("a");
        assert!(set.contains(&"a"));
        set.discard(&"a");
        assert!(!set.contains(&"a"));
    }
}
