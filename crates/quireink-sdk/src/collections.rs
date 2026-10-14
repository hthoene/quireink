//! Memory-lean collections for small devices.

use alloc::vec::Vec;
use core::borrow::Borrow;
use core::fmt;

/// A map stored as a vector sorted by key.
///
/// Unlike `BTreeMap`, which allocates nodes with room for eleven entries,
/// it needs exactly one allocation of `len × (key + value)` bytes (after
/// [`VecMap::shrink_to_fit`]). Lookups are binary searches; inserts move
/// the entries behind the new one, which is fine for registries and other
/// small maps that are filled once and then mostly read.
#[derive(Clone, PartialEq, Eq)]
pub struct VecMap<K, V> {
    entries: Vec<(K, V)>,
}

impl<K, V> Default for VecMap<K, V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for VecMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.entries.iter().map(|(k, v)| (k, v)))
            .finish()
    }
}

impl<K: Ord, V> VecMap<K, V> {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn find<Q: Ord + ?Sized>(&self, key: &Q) -> Result<usize, usize>
    where
        K: Borrow<Q>,
    {
        self.entries.binary_search_by(|(k, _)| k.borrow().cmp(key))
    }

    pub fn get<Q: Ord + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.find(key).ok().map(|i| &self.entries[i].1)
    }

    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        self.find(key).ok().map(|i| &mut self.entries[i].1)
    }

    pub fn contains_key<Q: Ord + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.find(key).is_ok()
    }

    /// Inserts or replaces; returns the previous value.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        match self.find(&key) {
            Ok(i) => Some(core::mem::replace(&mut self.entries[i].1, value)),
            Err(i) => {
                self.entries.insert(i, (key, value));
                None
            }
        }
    }

    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        self.find(key).ok().map(|i| self.entries.remove(i).1)
    }

    /// The value for `key`, inserting `default()` first if there is none.
    pub fn get_or_insert_with(&mut self, key: K, default: impl FnOnce() -> V) -> &mut V {
        let i = match self.find(&key) {
            Ok(i) => i,
            Err(i) => {
                self.entries.insert(i, (key, default()));
                i
            }
        };
        &mut self.entries[i].1
    }

    /// Entries in key order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &V)> + ExactSizeIterator {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &K> + ExactSizeIterator {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &V> + ExactSizeIterator {
        self.entries.iter().map(|(_, v)| v)
    }

    pub fn values_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut V> + ExactSizeIterator {
        self.entries.iter_mut().map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Releases unused capacity, e.g. once registration is complete.
    pub fn shrink_to_fit(&mut self) {
        self.entries.shrink_to_fit();
    }
}

impl<'a, K, V> IntoIterator for &'a VecMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter =
        core::iter::Map<core::slice::Iter<'a, (K, V)>, fn(&'a (K, V)) -> (&'a K, &'a V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().map(|(k, v)| (k, v))
    }
}

impl<K: Ord, V> FromIterator<(K, V)> for VecMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = Self::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    #[test]
    fn behaves_like_a_sorted_map() {
        let mut map: VecMap<String, u32> = VecMap::new();
        assert_eq!(map.insert("b".into(), 2), None);
        assert_eq!(map.insert("a".into(), 1), None);
        assert_eq!(map.insert("c".into(), 3), None);
        assert_eq!(map.insert("b".into(), 20), Some(2));
        assert_eq!(map.get("b"), Some(&20));
        assert!(map.contains_key("a") && !map.contains_key("d"));
        assert_eq!(
            map.keys().map(String::as_str).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        *map.get_or_insert_with("d".into(), || 0) += 4;
        *map.get_or_insert_with("a".into(), || 0) += 10;
        assert_eq!(map.values().copied().collect::<Vec<_>>(), [11, 20, 3, 4]);
        assert_eq!(map.remove("c"), Some(3));
        assert_eq!(map.len(), 3);
        let collected: Vec<(&String, &u32)> = (&map).into_iter().collect();
        assert_eq!(collected.len(), 3);
    }
}
