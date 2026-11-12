//! A map that keeps at most `cap` entries, dropping the oldest insert
//! first. What it drops is handed back so the caller can free it (images
//! must also leave the window's GPU atlas: `Window::drop_image`).

use std::borrow::Borrow;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

pub struct Bounded<K, V> {
    map: HashMap<K, V>,
    /// Keys oldest first.
    order: VecDeque<K>,
    cap: usize,
}

impl<K: Eq + Hash + Clone, V> Bounded<K, V> {
    pub fn new(cap: usize) -> Self {
        Bounded {
            map: HashMap::new(),
            order: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    pub fn get<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.map.get(key)
    }

    pub fn contains_key<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.map.contains_key(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.map.iter()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Inserts `value`; returns the values it pushed out (a replaced one
    /// for the same key, then the oldest past the cap).
    #[must_use]
    pub fn insert(&mut self, key: K, value: V) -> Vec<V> {
        let mut gone = vec![];
        if let Some(old) = self.map.insert(key.clone(), value) {
            gone.push(old);
            self.order.retain(|k| *k != key);
        }
        self.order.push_back(key);
        while self.map.len() > self.cap {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            gone.extend(self.map.remove(&oldest));
        }
        gone
    }

    /// Empties it; returns every value.
    #[must_use]
    pub fn clear(&mut self) -> Vec<V> {
        self.order.clear();
        self.map.drain().map(|(_, v)| v).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_the_oldest_past_the_cap() {
        let mut c = Bounded::new(2);
        assert!(c.insert("a", 1).is_empty());
        assert!(c.insert("b", 2).is_empty());
        assert_eq!(c.insert("c", 3), vec![1]);
        assert!(!c.contains_key(&"a"));
        assert_eq!(c.get(&"c"), Some(&3));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn replacing_hands_back_the_old_value() {
        let mut c = Bounded::new(2);
        let _ = c.insert("a", 1);
        let _ = c.insert("b", 2);
        assert_eq!(c.insert("a", 9), vec![1]);
        // "a" is now newest: "b" goes next.
        assert_eq!(c.insert("c", 3), vec![2]);
        let mut all = c.clear();
        all.sort();
        assert_eq!(all, vec![3, 9]);
        assert_eq!(c.len(), 0);
    }
}
