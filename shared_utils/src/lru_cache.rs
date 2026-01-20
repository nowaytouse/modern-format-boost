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

/// 缓存条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry<V> {
    pub value: V,
    /// 访问时间戳（毫秒，用于序列化）
    pub accessed_at_ms: u64,
    /// 创建时间戳（毫秒）
    pub created_at_ms: u64,
    /// 运行时访问时间（不序列化）
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

/// LRU缓存 - 带容量限制
#[derive(Debug)]
pub struct LruCache<K, V> {
    capacity: usize,
    entries: HashMap<K, CacheEntry<V>>,
    /// 驱逐计数（用于日志）
    eviction_count: u64,
}

impl<K: Hash + Eq + Clone, V: Clone> LruCache<K, V> {
    /// 创建新的LRU缓存
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1), // 至少1个条目
            entries: HashMap::new(),
            eviction_count: 0,
        }
    }

    /// 获取条目（更新访问时间）
    pub fn get(&mut self, key: &K) -> Option<&V> {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.touch();
            Some(&entry.value)
        } else {
            None
        }
    }

    /// 获取条目（不更新访问时间）
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|e| &e.value)
    }

    /// 插入条目（超过容量时驱逐最旧条目）
    pub fn insert(&mut self, key: K, value: V) {
        // 如果key已存在，更新值
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.value = value;
            entry.touch();
            return;
        }

        // 检查容量，需要时驱逐
        while self.entries.len() >= self.capacity {
            self.evict_lru();
        }

        self.entries.insert(key, CacheEntry::new(value));
    }

    /// 检查是否包含key
    pub fn contains_key(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// 获取当前条目数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 获取容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 获取驱逐计数
    pub fn eviction_count(&self) -> u64 {
        self.eviction_count
    }

    /// 驱逐最近最少使用的条目
    fn evict_lru(&mut self) {
        if self.entries.is_empty() {
            return;
        }

        // 找到访问时间最早的条目
        let oldest_key = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.accessed_at_ms)
            .map(|(k, _)| k.clone());

        if let Some(key) = oldest_key {
            self.entries.remove(&key);
            self.eviction_count += 1;
            // 🔥 响亮报告驱逐事件
            eprintln!(
                "📦 LRU Cache: evicted 1 entry (total evictions: {})",
                self.eviction_count
            );
        }
    }

    /// 清空缓存
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

// ═══════════════════════════════════════════════════════════════
// 序列化支持
// ═══════════════════════════════════════════════════════════════

/// 可序列化的缓存数据
#[derive(Debug, Serialize, Deserialize)]
pub struct SerializableCache<K, V> {
    pub capacity: usize,
    pub entries: Vec<(K, CacheEntry<V>)>,
}

impl<K: Hash + Eq + Clone + Serialize, V: Clone + Serialize> LruCache<K, V> {
    /// 序列化为JSON字符串
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
    /// 从JSON字符串反序列化
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let data: SerializableCache<K, V> = serde_json::from_str(json)?;
        let mut cache = Self::new(data.capacity);
        for (key, entry) in data.entries {
            cache.entries.insert(key, entry);
        }
        Ok(cache)
    }

    /// 保存到文件
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

    /// 从文件加载（失败时返回空缓存）
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
        // 此时缓存满了，1是最旧的

        cache.insert(3, "three".to_string());
        // 应该驱逐最旧的条目(1)

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&1), None); // 被驱逐
        assert_eq!(cache.get(&2), Some(&"two".to_string()));
        assert_eq!(cache.get(&3), Some(&"three".to_string()));
    }

    #[test]
    fn test_lru_order() {
        let mut cache: LruCache<i32, String> = LruCache::new(2);

        cache.insert(1, "one".to_string());
        std::thread::sleep(std::time::Duration::from_millis(10));
        cache.insert(2, "two".to_string());

        // 访问1，使其变为最近使用
        std::thread::sleep(std::time::Duration::from_millis(10));
        let _ = cache.get(&1);

        // 插入3，应该驱逐2（最旧）
        cache.insert(3, "three".to_string());

        assert_eq!(cache.get(&1), Some(&"one".to_string())); // 保留
        assert_eq!(cache.get(&2), None); // 被驱逐
        assert_eq!(cache.get(&3), Some(&"three".to_string())); // 新插入
    }
}

// ═══════════════════════════════════════════════════════════════
// 属性测试 (手动实现，避免外部依赖)
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod prop_tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    /// 简单的伪随机数生成器
    fn simple_rng(seed: u64, index: usize) -> u64 {
        let mut hasher = DefaultHasher::new();
        seed.hash(&mut hasher);
        index.hash(&mut hasher);
        hasher.finish()
    }

    // **Feature: video-explorer-robustness-v5.72, Property 1: LRU缓存容量不变性**
    // **Validates: Requirements 2.1, 2.2**
    #[test]
    fn prop_capacity_invariant() {
        // 测试100种不同的随机场景
        for seed in 0..100u64 {
            let capacity = ((simple_rng(seed, 0) % 19) + 1) as usize; // 1-20
            let num_ops = (simple_rng(seed, 1) % 200) as usize;

            let mut cache: LruCache<i32, i32> = LruCache::new(capacity);

            for i in 0..num_ops {
                let key = (simple_rng(seed, i + 2) % 100) as i32;
                let value = (simple_rng(seed, i + 1000) % 1000) as i32;
                cache.insert(key, value);

                // 🔥 核心属性：缓存大小永远不超过容量
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

    // **Feature: video-explorer-robustness-v5.72, Property 2: LRU驱逐正确性**
    // **Validates: Requirements 2.1, 2.2, 2.3**
    #[test]
    fn prop_lru_eviction_correctness() {
        // 测试驱逐的是最旧的条目
        for seed in 0..50u64 {
            let capacity = 3usize;
            let mut cache: LruCache<i32, String> = LruCache::new(capacity);

            // 插入3个条目
            cache.insert(1, "first".to_string());
            std::thread::sleep(std::time::Duration::from_millis(5));
            cache.insert(2, "second".to_string());
            std::thread::sleep(std::time::Duration::from_millis(5));
            cache.insert(3, "third".to_string());

            // 访问第一个，使其变为最近使用
            std::thread::sleep(std::time::Duration::from_millis(5));
            let _ = cache.get(&1);

            // 插入第四个，应该驱逐第二个（最旧）
            cache.insert(4, "fourth".to_string());

            // 🔥 核心属性：被驱逐的是访问时间最早的
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

    // **Feature: video-explorer-robustness-v5.72, Property 3: 缓存序列化Round-Trip**
    // **Validates: Requirements 7.1, 7.2, 7.3**
    #[test]
    fn prop_serialization_round_trip() {
        // 测试序列化后反序列化产生等价状态
        for seed in 0..50u64 {
            let capacity = ((simple_rng(seed, 0) % 10) + 1) as usize;
            let num_entries = (simple_rng(seed, 1) % 20) as usize;

            let mut original: LruCache<i32, i32> = LruCache::new(capacity);

            // 插入随机条目
            for i in 0..num_entries {
                let key = (simple_rng(seed, i + 2) % 50) as i32;
                let value = (simple_rng(seed, i + 100) % 1000) as i32;
                original.insert(key, value);
            }

            // 序列化
            let json = original.to_json().expect("Serialization should succeed");

            // 反序列化
            let restored: LruCache<i32, i32> =
                LruCache::from_json(&json).expect("Deserialization should succeed");

            // 🔥 核心属性：反序列化后的缓存与原始缓存等价
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

            // 验证所有条目都存在
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

    // **Feature: video-explorer-robustness-v5.72, Property 9: 损坏缓存恢复**
    // **Validates: Requirements 7.4**
    #[test]
    fn prop_corrupted_cache_recovery() {
        use std::io::Write;

        // 测试损坏的缓存文件能正常恢复
        let corrupted_jsons = vec![
            "",                                             // 空文件
            "{",                                            // 不完整JSON
            "null",                                         // null值
            "[]",                                           // 数组而非对象
            "{\"capacity\": -1}",                           // 无效容量
            "not json at all",                              // 完全无效
            "{\"capacity\": 10, \"entries\": \"invalid\"}", // entries类型错误
        ];

        for (i, corrupted) in corrupted_jsons.iter().enumerate() {
            // 创建临时文件
            let temp_dir = std::env::temp_dir();
            let temp_file = temp_dir.join(format!("test_corrupted_cache_{}.json", i));

            // 写入损坏内容
            let mut file = std::fs::File::create(&temp_file).unwrap();
            file.write_all(corrupted.as_bytes()).unwrap();

            // 🔥 核心属性：损坏文件应该返回空缓存，而不是崩溃
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

            // 清理
            let _ = std::fs::remove_file(&temp_file);
        }
    }
}
