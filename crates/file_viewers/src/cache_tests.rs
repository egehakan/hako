use super::*;

#[test]
fn evicts_the_least_recently_used_entry_first() {
    let mut cache = ByteBudgetCache::new(100);
    cache.insert("a", 1, 40);
    cache.insert("b", 2, 40);
    assert_eq!(cache.get(&"a"), Some(&1));

    cache.insert("c", 3, 40);

    assert!(cache.contains(&"a"));
    assert!(!cache.contains(&"b"));
    assert!(cache.contains(&"c"));
    assert_eq!(cache.used_bytes(), 80);
}

#[test]
fn peeking_does_not_protect_an_entry() {
    let mut cache = ByteBudgetCache::new(100);
    cache.insert("a", 1, 40);
    cache.insert("b", 2, 40);
    assert_eq!(cache.peek(&"a"), Some(&1));

    cache.insert("c", 3, 40);

    assert!(!cache.contains(&"a"));
}

#[test]
fn an_oversized_entry_is_kept_alone() {
    let mut cache = ByteBudgetCache::new(100);
    cache.insert("a", 1, 40);
    cache.insert("huge", 2, 500);

    assert_eq!(cache.len(), 1);
    assert_eq!(cache.get(&"huge"), Some(&2));
    assert_eq!(cache.used_bytes(), 500);
}

#[test]
fn replacing_an_entry_updates_the_accounting() {
    let mut cache = ByteBudgetCache::new(100);
    cache.insert("a", 1, 60);
    cache.insert("a", 2, 10);

    assert_eq!(cache.used_bytes(), 10);
    assert_eq!(cache.get(&"a"), Some(&2));
}

#[test]
fn retain_and_remove_release_their_bytes() {
    let mut cache = ByteBudgetCache::new(1000);
    for key in 0..10u32 {
        cache.insert(key, key, 10);
    }
    cache.retain(|key| key % 2 == 0);
    assert_eq!(cache.len(), 5);
    assert_eq!(cache.used_bytes(), 50);

    assert_eq!(cache.remove(&4), Some(4));
    assert_eq!(cache.remove(&4), None);
    assert_eq!(cache.used_bytes(), 40);

    cache.clear();
    assert!(cache.is_empty());
    assert_eq!(cache.used_bytes(), 0);
}
