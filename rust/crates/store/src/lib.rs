use ahash::RandomState;
use dashmap::DashMap;
use std::collections::HashSet;
use std::time::Duration;

mod compression;
pub mod key;
mod timestamp;
pub mod value;

use crate::key::Key;
use crate::timestamp::{TimeWizz, Timestamp};
use crate::value::{EMPTY_RAW_VALUE, RawValue, Value};

const STORE_CAP: usize = 1_000_000;
const TTL_BUCKETS_CAP: usize = 1_000;

/// HashMap to store xxh3 hash (KEY_SIZE) -> DEVICE_ID (VAL_SIZE)
pub struct XXH3KVStore {
    pub data: DashMap<Key, Value, RandomState>,
    pub ttl_buckets: DashMap<Timestamp, HashSet<Key, RandomState>, RandomState>,
}

impl Default for XXH3KVStore {
    fn default() -> Self {
        let data = DashMap::with_capacity_and_hasher(STORE_CAP, RandomState::default());
        let ttl_buckets =
            DashMap::with_capacity_and_hasher(TTL_BUCKETS_CAP, RandomState::default());

        Self { data, ttl_buckets }
    }
}

impl XXH3KVStore {
    pub fn new() -> Self {
        Default::default()
    }

    #[inline]
    pub fn get(&self, key: Key) -> RawValue {
        self.data
            .get(&key)
            .filter(|e| !e.value().is_expired())
            .map_or(EMPTY_RAW_VALUE, |e| e.value().into())
    }

    #[inline]
    pub fn update(&self, key: Key, value: RawValue) {
        let expire_at = Timestamp::future(1_000);

        let value = Value::new(value, expire_at);
        self.data.insert(key, value);

        let new_expiry_bucket = expire_at.bucket::<100>();
        self.ttl_buckets
            .entry(new_expiry_bucket)
            .or_insert_with(|| {
                HashSet::<Key, RandomState>::with_capacity_and_hasher(1_000, RandomState::default())
            })
            .insert(key);
    }

    #[inline]
    pub async fn stats_loop(&self) {
        let mut interval = tokio::time::interval(Duration::from_millis(10));

        loop {
            interval.tick().await;
            print!(
                "\r\x1B[2KStore stats: Len: {}, TTL: {}",
                self.data.len(),
                self.ttl_buckets.len()
            );
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
        }
    }

    #[inline]
    pub async fn ttl_loop(&self) {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        interval.tick().await;

        let mut cursor = Timestamp::now().bucket::<100>();

        loop {
            interval.tick().await;
            let now_bucket = Timestamp::now().bucket::<100>();

            while cursor < now_bucket {
                if let Some((_, bucket_to_remove)) = self.ttl_buckets.remove(&cursor) {
                    for key in bucket_to_remove.iter() {
                        self.data.remove_if(key, |_, value| value.is_expired());
                    }
                }
                cursor = cursor.saturating_add(100);
            }
        }
    }
}
