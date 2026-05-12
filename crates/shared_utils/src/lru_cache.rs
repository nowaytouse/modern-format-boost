//! LRU Cache Module - Least Recently Used cache with capacity limits
//!
//! 🔥 v5.72: Solves long-running memory leak issues
//!
//! ## Features
//! - Capacity Limit: Automatically evicts oldest entries when the limit is exceeded
//! - LRU Tracking: Updates timestamps upon access
//! - Serialization Support: Persistent storage to JSON files

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry<V> {
    pub value: V,
    pub accessed_at_ms: u64,
    pub created_at_ms: u64,
    #[serde(skip)]
    accessed_instant: Option<Instant>,
}

impl<V> CacheEntry<V> {
    fn current_epoch_millis_u64() -> u64 {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis();
        u64::try_from(now_ms).unwrap_or_else(|_| {
            tracing::warn!(
                "☢️ [ANOMALY] Current epoch millis exceeded u64; using u64::MAX as cache timestamp"
            );
            u64::MAX
        })
    }

    fn new(value: V) -> Self {
        let now_ms_u64 = Self::current_epoch_millis_u64();
        Self {
            value,
            accessed_at_ms: now_ms_u64,
            created_at_ms: now_ms_u64,
            accessed_instant: Some(Instant::now()),
        }
    }

    fn touch(&mut self) {
        self.accessed_at_ms = Self::current_epoch_millis_u64();
        self.accessed_instant = Some(Instant::now());
    }
}

#[derive(Debug)]
pub struct LruCache<K, V> {
    capacity: usize,
    entries: HashMap<K, CacheEntry<V>>,
    eviction_count: u64,
}

impl<K: Hash + Eq + Clone, V: Clone> LruCache<K, V> {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            eviction_count: 0,
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        {
            let entry = self.entries.get_mut(key)?;
            entry.touch();
            Some(&entry.value)
        }
    }

    pub fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|e| &e.value)
    }

    pub fn insert(&mut self, key: K, value: V) {
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.value = value;
            entry.touch();
            return;
        }

        while self.entries.len() >= self.capacity {
            self.evict_lru();
        }

        self.entries.insert(key, CacheEntry::new(value));
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub const fn eviction_count(&self) -> u64 {
        self.eviction_count
    }

    fn evict_lru(&mut self) {
        if self.entries.is_empty() {
            return;
        }

        let oldest_key = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.accessed_at_ms)
            .map(|(k, _)| k.clone());

        if let Some(key) = oldest_key {
            self.entries.remove(&key);
            self.eviction_count += 1;
            crate::log_debug!(
                "📦 LRU Cache: evicted 1 entry (total evictions: {})",
                self.eviction_count
            );
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SerializableCache<K, V> {
    pub capacity: usize,
    pub entries: Vec<(K, CacheEntry<V>)>,
}

impl<K: Hash + Eq + Clone + Serialize, V: Clone + Serialize> LruCache<K, V> {
    /// Convert the cache to JSON.
    ///
    /// # Errors
    /// Returns a `serde_json::Error` if serialization fails.
    pub fn to_json(&self) -> Result<String, serde_json::Error>
    where
        K: Serialize,
        V: Serialize,
    {
        let data = SerializableCache {
            capacity: self.capacity,
            entries: self
                .entries
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        serde_json::to_string_pretty(&data)
    }
}

impl<K: Hash + Eq + Clone + for<'de> Deserialize<'de>, V: Clone + for<'de> Deserialize<'de>>
    LruCache<K, V>
{
    /// Restore the cache from JSON.
    ///
    /// # Errors
    /// Returns a `serde_json::Error` if deserialization fails.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let data: SerializableCache<K, V> = serde_json::from_str(json)?;
        let mut cache = Self::new(data.capacity);
        for (key, entry) in data.entries {
            cache.entries.insert(key, entry);
        }
        Ok(cache)
    }

    /// Save the cache to a file.
    ///
    /// # Errors
    /// Returns an `io::Result` if saving fails.
    pub fn save_to_file(&self, path: &std::path::Path) -> std::io::Result<()>
    where
        K: Serialize,
        V: Serialize,
    {
        let json = self
            .to_json()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    #[must_use]
    pub fn load_from_file(path: &std::path::Path, capacity: usize) -> Self {
        std::fs::read_to_string(path).map_or_else(
            |_| Self::new(capacity),
            |json| {
                Self::from_json(&json).map_or_else(
                    |e| {
                        crate::log_anomaly!(
                            crate::static_logs::messages::LABEL_ANOMALY,
                            "LRU Cache: failed to parse cache file, starting fresh: {e}"
                        );
                        Self::new(capacity)
                    },
                    |cache| {
                        crate::log_info!(
                            crate::static_logs::messages::LABEL_DETECTION,
                            "LRU Cache: loaded {} entries from {}",
                            cache.len(),
                            path.display()
                        );
                        cache
                    },
                )
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut cache: LruCache<i32, String> = LruCache::new(3);

        cache.insert(1_i32, "one".to_string());
        cache.insert(2_i32, "two".to_string());
        cache.insert(3_i32, "three".to_string());

        assert_eq!(cache.len(), 3);
        assert_eq!(cache.get(&1_i32), Some(&"one".to_string()));
        assert_eq!(cache.get(&2_i32), Some(&"two".to_string()));
        assert_eq!(cache.get(&3_i32), Some(&"three".to_string()));
    }

    #[test]
    fn test_eviction() {
        let mut cache: LruCache<i32, String> = LruCache::new(2);

        cache.insert(1_i32, "one".to_string());
        std::thread::sleep(std::time::Duration::from_millis(5));
        cache.insert(2_i32, "two".to_string());

        cache.insert(3_i32, "three".to_string());

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&1_i32), None);
        assert_eq!(cache.get(&2_i32), Some(&"two".to_string()));
        assert_eq!(cache.get(&3_i32), Some(&"three".to_string()));
    }

    #[test]
    fn test_lru_order() {
        let mut cache: LruCache<i32, String> = LruCache::new(2);

        cache.insert(1_i32, "one".to_string());
        std::thread::sleep(std::time::Duration::from_millis(10));
        cache.insert(2_i32, "two".to_string());

        std::thread::sleep(std::time::Duration::from_millis(10));
        let _ = cache.get(&1_i32);

        cache.insert(3_i32, "three".to_string());

        assert_eq!(cache.get(&1_i32), Some(&"one".to_string()));
        assert_eq!(cache.get(&2_i32), None);
        assert_eq!(cache.get(&3_i32), Some(&"three".to_string()));
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn simple_rng(seed: u64, index: usize) -> u64 {
        let mut hasher = DefaultHasher::new();
        seed.hash(&mut hasher);
        index.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn prop_capacity_invariant() {
        for seed in 0..100u64 {
            let capacity = crate::numeric_cast::u64_to_usize_sat((simple_rng(seed, 0) % 19) + 1);
            let num_ops = (simple_rng(seed, 1) % 200) as usize;

            let mut cache: LruCache<i32, i32> = LruCache::new(capacity);

            for i in 0..num_ops {
                let key = (simple_rng(seed, i + 2) % 100) as i32;
                let value = (simple_rng(seed, i + 1000) % 1000) as i32;
                cache.insert(key, value);

                assert!(
                    cache.len() <= capacity,
                    "Seed {}: Cache size {} exceeded capacity {} after {} ops",
                    seed,
                    cache.len(),
                    capacity,
                    i + 1
                );
            }
        }
    }

    #[test]
    fn prop_lru_eviction_correctness() {
        for seed in 0..50u64 {
            let capacity = 3usize;
            let mut cache: LruCache<i32, String> = LruCache::new(capacity);

            cache.insert(1_i32, "first".to_string());
            std::thread::sleep(std::time::Duration::from_millis(5));
            cache.insert(2_i32, "second".to_string());
            std::thread::sleep(std::time::Duration::from_millis(5));
            cache.insert(3_i32, "third".to_string());

            std::thread::sleep(std::time::Duration::from_millis(5));
            let _ = cache.get(&1_i32);

            cache.insert(4_i32, "fourth".to_string());

            assert!(
                cache.get(&1_i32).is_some(),
                "Seed {seed}: Entry 1 should be kept (recently accessed)"
            );
            assert!(
                cache.get(&2_i32).is_none(),
                "Seed {seed}: Entry 2 should be evicted (oldest)"
            );
            assert!(
                cache.get(&3_i32).is_some(),
                "Seed {seed}: Entry 3 should be kept"
            );
            assert!(
                cache.get(&4_i32).is_some(),
                "Seed {seed}: Entry 4 should be kept (just inserted)"
            );
        }
    }

    #[test]
    fn prop_serialization_round_trip() {
        for seed in 0..50u64 {
            let capacity = crate::numeric_cast::u64_to_usize_sat((simple_rng(seed, 0) % 10) + 1);
            let num_entries = (simple_rng(seed, 1) % 20) as usize;

            let mut original: LruCache<i32, i32> = LruCache::new(capacity);

            for i in 0..num_entries {
                let key = (simple_rng(seed, i + 2) % 50) as i32;
                let value = (simple_rng(seed, i + 100) % 1000) as i32;
                original.insert(key, value);
            }

            let json = original
                .to_json()
                .unwrap_or_else(|e| panic!("error: {e:?}"));

            let restored: LruCache<i32, i32> =
                LruCache::from_json(&json).unwrap_or_else(|e| panic!("error: {e:?}"));

            assert_eq!(
                original.len(),
                restored.len(),
                "Seed {seed}: Length mismatch after round-trip"
            );
            assert_eq!(
                original.capacity(),
                restored.capacity(),
                "Seed {seed}: Capacity mismatch after round-trip"
            );

            for (key, entry) in &original.entries {
                let restored_entry = restored.entries.get(key);
                assert!(
                    restored_entry.is_some(),
                    "Seed {seed}: Key {key} missing after round-trip"
                );
                assert_eq!(
                    entry.value,
                    restored_entry
                        .unwrap_or_else(|| panic!("missing entry"))
                        .value,
                    "Seed {seed}: Value mismatch for key {key}"
                );
            }
        }
    }

    #[test]
    fn prop_corrupted_cache_recovery() {
        use std::io::Write;

        let corrupted_jsons = [
            "",
            "{",
            "null",
            "[]",
            "{\"capacity\": -1}",
            "not json at all",
            "{\"capacity\": 10, \"entries\": \"invalid\"}",
        ];

        for (i, corrupted) in corrupted_jsons.iter().enumerate() {
            let temp_dir = std::env::temp_dir();
            let temp_file = temp_dir.join(format!("test_corrupted_cache_{i}.json"));

            let mut file =
                std::fs::File::create(&temp_file).unwrap_or_else(|e| panic!("error: {e:?}"));
            file.write_all(corrupted.as_bytes())
                .unwrap_or_else(|e| panic!("error: {e:?}"));

            let cache: LruCache<i32, i32> = LruCache::load_from_file(&temp_file, 10);
            assert_eq!(
                cache.len(),
                0,
                "Corrupted JSON #{i} should result in empty cache"
            );
            assert_eq!(
                cache.capacity(),
                10,
                "Corrupted JSON #{i} should use provided capacity"
            );

            let _ = crate::io_utils::safe_remove_file(&temp_file);
        }
    }
}
