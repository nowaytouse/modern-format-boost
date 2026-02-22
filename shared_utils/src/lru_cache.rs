//! LRU Cache Module - 带容量限制的最近最少使用缓存
//!
//! 🔥 v5.72: 解决长时间运行内存泄漏问题
//!
//! ## 功能
//! - 容量限制：超过上限自动驱逐最旧条目
//! - LRU追踪：访问时更新时间戳
//! - 序列化支持：可持久化到JSON文件

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
    fn new(value: V) -> Self {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;
        Self {
            value,
            accessed_at_ms: now_ms,
            created_at_ms: now_ms,
            accessed_instant: Some(Instant::now()),
        }
    }

    fn touch(&mut self) {
        self.accessed_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;
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
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            eviction_count: 0,
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.touch();
            Some(&entry.value)
        } else {
            None
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

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn eviction_count(&self) -> u64 {
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
            eprintln!(
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
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let data: SerializableCache<K, V> = serde_json::from_str(json)?;
        let mut cache = Self::new(data.capacity);
        for (key, entry) in data.entries {
            cache.entries.insert(key, entry);
        }
        Ok(cache)
    }

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

    pub fn load_from_file(path: &std::path::Path, capacity: usize) -> Self {
        match std::fs::read_to_string(path) {
            Ok(json) => match Self::from_json(&json) {
                Ok(cache) => {
                    eprintln!(
                        "📦 LRU Cache: loaded {} entries from {:?}",
                        cache.len(),
                        path
                    );
                    cache
                }
                Err(e) => {
                    eprintln!(
                        "⚠️ LRU Cache: failed to parse cache file, starting fresh: {}",
                        e
                    );
                    Self::new(capacity)
                }
            },
            Err(_) => Self::new(capacity),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut cache: LruCache<i32, String> = LruCache::new(3);

        cache.insert(1, "one".to_string());
        cache.insert(2, "two".to_string());
        cache.insert(3, "three".to_string());

        assert_eq!(cache.len(), 3);
        assert_eq!(cache.get(&1), Some(&"one".to_string()));
        assert_eq!(cache.get(&2), Some(&"two".to_string()));
        assert_eq!(cache.get(&3), Some(&"three".to_string()));
    }

    #[test]
    fn test_eviction() {
        let mut cache: LruCache<i32, String> = LruCache::new(2);

        cache.insert(1, "one".to_string());
        std::thread::sleep(std::time::Duration::from_millis(5));
        cache.insert(2, "two".to_string());

        cache.insert(3, "three".to_string());

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&1), None);
        assert_eq!(cache.get(&2), Some(&"two".to_string()));
        assert_eq!(cache.get(&3), Some(&"three".to_string()));
    }

    #[test]
    fn test_lru_order() {
        let mut cache: LruCache<i32, String> = LruCache::new(2);

        cache.insert(1, "one".to_string());
        std::thread::sleep(std::time::Duration::from_millis(10));
        cache.insert(2, "two".to_string());

        std::thread::sleep(std::time::Duration::from_millis(10));
        let _ = cache.get(&1);

        cache.insert(3, "three".to_string());

        assert_eq!(cache.get(&1), Some(&"one".to_string()));
        assert_eq!(cache.get(&2), None);
        assert_eq!(cache.get(&3), Some(&"three".to_string()));
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
            let capacity = ((simple_rng(seed, 0) % 19) + 1) as usize;
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

            cache.insert(1, "first".to_string());
            std::thread::sleep(std::time::Duration::from_millis(5));
            cache.insert(2, "second".to_string());
            std::thread::sleep(std::time::Duration::from_millis(5));
            cache.insert(3, "third".to_string());

            std::thread::sleep(std::time::Duration::from_millis(5));
            let _ = cache.get(&1);

            cache.insert(4, "fourth".to_string());

            assert!(
                cache.get(&1).is_some(),
                "Seed {}: Entry 1 should be kept (recently accessed)",
                seed
            );
            assert!(
                cache.get(&2).is_none(),
                "Seed {}: Entry 2 should be evicted (oldest)",
                seed
            );
            assert!(
                cache.get(&3).is_some(),
                "Seed {}: Entry 3 should be kept",
                seed
            );
            assert!(
                cache.get(&4).is_some(),
                "Seed {}: Entry 4 should be kept (just inserted)",
                seed
            );
        }
    }

    #[test]
    fn prop_serialization_round_trip() {
        for seed in 0..50u64 {
            let capacity = ((simple_rng(seed, 0) % 10) + 1) as usize;
            let num_entries = (simple_rng(seed, 1) % 20) as usize;

            let mut original: LruCache<i32, i32> = LruCache::new(capacity);

            for i in 0..num_entries {
                let key = (simple_rng(seed, i + 2) % 50) as i32;
                let value = (simple_rng(seed, i + 100) % 1000) as i32;
                original.insert(key, value);
            }

            let json = original.to_json().expect("Serialization should succeed");

            let restored: LruCache<i32, i32> =
                LruCache::from_json(&json).expect("Deserialization should succeed");

            assert_eq!(
                original.len(),
                restored.len(),
                "Seed {}: Length mismatch after round-trip",
                seed
            );
            assert_eq!(
                original.capacity(),
                restored.capacity(),
                "Seed {}: Capacity mismatch after round-trip",
                seed
            );

            for (key, entry) in &original.entries {
                let restored_entry = restored.entries.get(key);
                assert!(
                    restored_entry.is_some(),
                    "Seed {}: Key {} missing after round-trip",
                    seed,
                    key
                );
                assert_eq!(
                    entry.value,
                    restored_entry.unwrap().value,
                    "Seed {}: Value mismatch for key {}",
                    seed,
                    key
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
            let temp_file = temp_dir.join(format!("test_corrupted_cache_{}.json", i));

            let mut file = std::fs::File::create(&temp_file).unwrap();
            file.write_all(corrupted.as_bytes()).unwrap();

            let cache: LruCache<i32, i32> = LruCache::load_from_file(&temp_file, 10);
            assert_eq!(
                cache.len(),
                0,
                "Corrupted JSON #{} should result in empty cache",
                i
            );
            assert_eq!(
                cache.capacity(),
                10,
                "Corrupted JSON #{} should use provided capacity",
                i
            );

            let _ = std::fs::remove_file(&temp_file);
        }
    }
}
