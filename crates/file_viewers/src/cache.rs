//! A cache bounded by the memory its entries take.

use std::collections::HashMap;
use std::hash::Hash;

struct Entry<V> {
    value: V,
    bytes: usize,
    last_used: u64,
}

/// Keeps the most recently used values within a byte budget.
pub struct ByteBudgetCache<K, V> {
    entries: HashMap<K, Entry<V>>,
    budget: usize,
    used: usize,
    clock: u64,
}

impl<K: Hash + Eq + Clone, V> ByteBudgetCache<K, V> {
    pub fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            budget,
            used: 0,
            clock: 0,
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Returns the value for `key` and marks it as just used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let now = self.tick();
        let entry = self.entries.get_mut(key)?;
        entry.last_used = now;
        Some(&entry.value)
    }

    /// Returns the value for `key` without affecting what gets evicted next.
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|entry| &entry.value)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// Every entry, in no particular order and without affecting what gets evicted next.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(key, entry)| (key, &entry.value))
    }

    /// Stores `value`, then evicts the least recently used entries until the budget holds. The
    /// entry just stored is never evicted, so one oversized value can still be shown.
    pub fn insert(&mut self, key: K, value: V, bytes: usize) {
        let now = self.tick();
        if let Some(previous) = self.entries.insert(
            key.clone(),
            Entry {
                value,
                bytes,
                last_used: now,
            },
        ) {
            self.used -= previous.bytes;
        }
        self.used += bytes;

        while self.used > self.budget && self.entries.len() > 1 {
            let oldest = self
                .entries
                .iter()
                .filter(|(candidate, _)| **candidate != key)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(candidate, _)| candidate.clone());
            let Some(oldest) = oldest else {
                break;
            };
            self.remove(&oldest);
        }
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let entry = self.entries.remove(key)?;
        self.used -= entry.bytes;
        Some(entry.value)
    }

    /// Drops every entry `keep` rejects.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let mut used = self.used;
        self.entries.retain(|key, entry| {
            let kept = keep(key);
            if !kept {
                used -= entry.bytes;
            }
            kept
        });
        self.used = used;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.used = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn used_bytes(&self) -> usize {
        self.used
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
