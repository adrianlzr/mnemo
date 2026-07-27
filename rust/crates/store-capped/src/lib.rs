//! Fixed-capacity, single-threaded key-value store.
//!
//! # Sizing
//!
//! Callers state how many entries the store must hold; the slot count is
//! derived from that and an occupancy percentage. Use the [`xxh3_map!`] macro:
//!
//! ```ignore
//! type Store = xxh3_map!(1_000_000_000);      // default occupancy
//! type Dense = xxh3_map!(1_000_000_000, 80);  // tighter, longer probe tail
//! ```
//!
//! `CAP` is arbitrary — bucket selection uses a multiply-shift rather than a
//! mask, so the table is sized to the load factor you asked for instead of
//! rounding up to the next power of two. That rounding is the dominant memory
//! cost at scale: at one billion entries a power-of-two table must jump to 2^31
//! slots (64 GiB), while 70% occupancy asks for 1.43e9 slots (42.6 GiB).
//!
//! # Layout
//!
//! Open addressing with linear probing, **directly addressed**: an entry lives
//! at (or just after) the bucket its key hashes to. There is no bucket → slot
//! indirection, so a lookup issues one memory reference whose address is kt1n
//! from the key alone, rather than a chain of three dependent loads.
//!
//! [`Entry`] is exactly 32 bytes and 32-byte aligned, so an entry never
//! straddles a 64-byte cache line and two entries share one. A first-probe hit
//! touches exactly one line.
//!
//! # Zero-validity
//!
//! The map is allocated from an anonymous zeroed `mmap`, so every field must be
//! valid — and mean the right thing — when all its bytes are zero:
//!
//! - [`Key`] is `repr(transparent)` over `u64`, and zero is the reserved
//!   empty-slot sentinel. [`Key::from_hash`] cannot produce it.
//! - `Entry::expires_at` is [`Time`], `repr(transparent)` over `u32`, whose zero
//!   value is `Time::NEVER`.
//! - `Entry::value` is a `CompressedUUID`, a byte array; zero is valid.
//! - `len: usize` is trivially zero-valid.
//!
//! [`EMPTY_ENTRY`] spells this out, and `tests::zero_bit_pattern_is_empty_entry`
//! asserts it. If a field gains a type whose zero is not valid, that test fails
//! rather than [`XXH3Map::new`] becoming silently unsound.
//!
//! # Concurrency
//!
//! None. One thread owns the map exclusively.
use compression::uuid;
use std::fmt;
use time::{Time, Ttl};

/// Default share of slots that may be claimed, as a percentage.
///
/// At 0.70 load a successful search averages 2.17 probes — about 1.6 cache
/// lines, and the extra lines are sequential and prefetchable.
pub const DEFAULT_OCCUPANCY_PCT: usize = 70;

/// Hardest occupancy the map will accept. Linear probing degrades sharply past
/// this, and the empty-slot margin is what makes probe termination provable.
pub const MAX_OCCUPANCY_PCT: usize = 90;

/// Smallest permitted slot count.
const MIN_CAP: usize = 16;

/// Convenience lifetime for callers: `Some(t1.plus(DEFAULT_TTL))`.
pub const DEFAULT_TTL: Ttl = Ttl::from_secs(1000);

/// Slots needed to hold `max_entries` at `occupancy_pct` load.
pub const fn cap_for(max_entries: usize, occupancy_pct: usize) -> usize {
    assert!(occupancy_pct > 0, "occupancy must be positive");
    assert!(
        occupancy_pct <= MAX_OCCUPANCY_PCT,
        "occupancy exceeds MAX_OCCUPANCY_PCT"
    );
    assert!(
        max_entries <= usize::MAX / 100,
        "max_entries would overflow the slot computation"
    );
    max_entries * 100 / occupancy_pct + 1
}

/// Declares a store type from the number of entries it must hold.
///
/// Optionally you can pass a occupancy_pct, otherwise it will use
/// the default value DEFAULT_OCCUPANCY_PCT.
///
/// ```ignore
/// type Store = xxh3_map!(1_000_000_000);
/// type Store10PctOccupancy = xxh3_map!(1_000_000, 10);
/// ```
#[macro_export]
macro_rules! xxh3_map {
    ($max_entries:expr) => {
        $crate::xxh3_map!($max_entries, $crate::DEFAULT_OCCUPANCY_PCT)
    };
    ($max_entries:expr, $occupancy_pct:expr) => {
        $crate::XXH3Map<
            { $crate::cap_for($max_entries, $occupancy_pct) },
            { $max_entries }
        >
    };
}

/// A stored key: an xxh3 hash, already computed by the client.
///
/// Zero is reserved as the empty-slot sentinel and cannot be constructed
/// through the public API, so a `Key` in hand is always a storable key.
#[repr(transparent)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Key(u64);

impl Key {
    pub const SIZE: usize = std::mem::size_of::<Self>();
    /// The empty-slot sentinel. Deliberately unreachable from outside.
    pub const EMPTY: Key = Key(0);

    /// Folds a raw hash into the key space, keeping the sentinel reserved.
    ///
    /// The collision this introduces has probability 2^-64 per key and costs
    /// one predictable branch at the parse boundary.
    #[inline]
    pub const fn from_hash(hash: u64) -> Self {
        Self(if hash == 0 { 1 } else { hash })
    }

    /// Decodes an 8-byte wire key.
    ///
    /// Big-endian is xxh3's canonical byte order, so the value reconstructed
    /// here is bit-identical to the one the client computed. Every client must
    /// agree — a client that packs little-endian produces a different key for
    /// the same input string, silently and without error.
    #[inline]
    pub const fn from_wire(bytes: &[u8]) -> Self {
        let mut b = [0u8; 8];
        b.copy_from_slice(bytes);
        Self::from_hash(u64::from_be_bytes(b))
    }

    #[inline]
    pub const fn to_wire(self) -> [u8; 8] {
        self.0.to_be_bytes()
    }

    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    #[inline]
    const fn is_empty(self) -> bool {
        self.0 == Self::EMPTY.0
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({:#018x})", self.0)
    }
}

impl From<&[u8]> for Key {
    fn from(value: &[u8]) -> Self {
        Self::from_wire(value)
    }
}

pub type Value = uuid::UUID;
pub type StoredValue = uuid::CompressedUUID;

/// One slot. Exactly 32 bytes, 32-byte aligned.
#[repr(C, align(32))]
#[derive(Clone, Copy)]
struct Entry {
    /// Stored key, or [`Key::EMPTY`] if this slot has never been claimed.
    key: Key,
    /// Deadline. `Time::NEVER` (zero) means no expiry.
    expires_at: Time,
    value: StoredValue,
}

impl Entry {
    #[inline]
    const fn is_occupied(&self) -> bool {
        !self.key.is_empty()
    }

    /// Occupied but past its deadline. An expired entry terminates nothing — it
    /// holds its place in the probe chain until reused.
    #[inline]
    const fn is_expired(&self, t1: Time) -> bool {
        self.expires_at.is_past_at(t1)
    }
    /// Update the entry with new key, value and expires_at
    #[inline]
    fn update(&mut self, key: Key, value: Value, expires_at: Time) {
        self.key = key;
        self.value = value.into();
        self.expires_at = expires_at;
    }
}

#[derive(Debug)]
pub enum GetError {
    NotFound,
    Expired,
    ProbeExhaustion,
}
pub type GetResult = Result<Value, GetError>;

pub enum InsertError {
    Full,
    ProbeExhaustion,
}
pub type InsertResult = Result<Value, InsertError>;

/// `CAP` is the slot count and `MAX_ENTRIES` the number of slots that may be
/// claimed. Prefer the [`xxh3_map!`] macro over naming both directly.
#[repr(C)]
pub struct XXH3Map<
    const CAP: usize = { cap_for(1000, DEFAULT_OCCUPANCY_PCT) },
    const MAX_ENTRIES: usize = 1000,
> {
    entries: [Entry; CAP],
    /// Slots that have ever been claimed, including entries t1 expired.
    ///
    /// Monotone: reclaiming an expired slot in place does not decrement it,
    /// because zeroing a slot mid-cluster would orphan every key behind it.
    /// This is a high-water mark, not a live count.
    len: usize,
}

impl<const CAP: usize, const MAX_ENTRIES: usize> XXH3Map<CAP, MAX_ENTRIES> {
    /// Total slots.
    pub const SLOTS: usize = CAP;

    /// Slots that may be claimed before writes start failing.
    pub const ENTRY_LIMIT: usize = MAX_ENTRIES;

    /// Evaluated at monomorphization — bad parameters fail the build.
    const CHECK: () = {
        assert!(CAP >= MIN_CAP, "slot count is too small");
        assert!(
            CAP <= usize::MAX / 100,
            "slot count would overflow the occupancy computation"
        );
        assert!(
            MAX_ENTRIES < CAP,
            "the entry limit must leave at least one empty slot"
        );
        assert!(
            MAX_ENTRIES * 100 / CAP <= MAX_OCCUPANCY_PCT,
            "the entry limit exceeds MAX_OCCUPANCY_PCT of the slot count"
        );
    };

    /// Allocates the map in an anonymous zeroed mapping and leaks it.
    ///
    /// The mapping is never unmapped: the map is process-lifetime state, so its
    /// address is stable forever and raw pointers into it stay valid.
    ///
    /// Transparent huge pages are requested but not required. At large `CAP` the
    /// table vastly exceeds TLB coverage with 4 KiB pages, and 2 MiB pages cut
    /// the page-walk cost substantially. Failure is ignored — THP may be
    /// disabled or unavailable, which costs throughput, not correctness.
    ///
    /// # Panics
    ///
    /// If the mapping cannot be created.
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> &'static mut Self {
        let () = Self::CHECK;

        // Pin the process epoch before any entry can carry a deadline, so
        // "uptime" means process start rather than first request.
        time::init();

        // SAFETY: Every field of `Self` is valid and correct when
        // all bytes are zero — see the module-level zero-validity notes, which
        // `tests::zero_bit_pattern_is_empty_entry` enforces.
        unsafe { mmap::mmap_zeroed() }
    }

    /// Home bucket for `key`, by multiply-shift (Lemire's fastrange).
    ///
    /// Maps the full 64-bit key uniformly onto `[0, CAP)` for any `CAP`, using
    /// one widening multiply — a `umulh`, roughly three cycles, and invisible
    /// beside the memory reference that follows. The bias against perfect
    /// uniformity is on the order of `CAP / 2^64`.
    #[inline]
    const fn bucket_of(key: Key) -> usize {
        ((key.0 as u128 * CAP as u128) >> 64) as usize
    }

    /// Next slot in the probe sequence, wrapping at `CAP`.
    #[inline]
    const fn next(i: usize) -> usize {
        let n = i + 1;
        if n == CAP { 0 } else { n }
    }

    /// Returns the live value for `key`.
    ///
    /// `now` is supplied by the caller so that it can avoid
    /// reading the clock on every slot probe.
    #[inline]
    pub fn get(&mut self, key: Key, now: Time) -> GetResult {
        let mut i = Self::bucket_of(key);

        for _ in 0..MAX_ENTRIES + 1 {
            // SAFETY: No need for bounds checking. MAX_ENTRIES + 1 is always < CAP
            let entry = unsafe { self.entries.get_unchecked_mut(i) };

            if !entry.is_occupied() {
                return Err(GetError::NotFound);
            }
            if entry.key == key {
                if entry.is_expired(now) {
                    return Err(GetError::Expired);
                }
                return Ok(entry.value.into());
            }

            i = Self::next(i);
        }
        Err(GetError::ProbeExhaustion)
    }

    /// Writes `value` under `key`, replacing whatever was there before.
    ///
    /// `ttl` is a duration — pass `Some(ttl)` for a TTL,
    /// or `None` for an entry that never expires. Be deliberate about
    /// `None`: a slot with no deadline is never reclaimable, so those entries
    /// hold their slot against the entry limit for the life of the process.
    ///
    /// # Writes can be dropped silently
    ///
    /// If the table has claimed `MAX_ENTRIES` slots and there is no expired
    /// entry in this key's probe chain to recycle, the write is discarded and
    /// nothing is reported.
    ///
    /// # How a slot gets chosen
    ///
    /// Linear probing outward from the key's home bucket, stopping at whichever
    /// of these comes first:
    ///
    /// - **An empty slot.** Nothing has ever been written here, so `key` is
    ///   definitely absent from the table — an occupied slot never gets zeroed
    ///   back out, so an empty slot is a hard end-of-chain marker. Claim the
    ///   first expired slot we walked past if there was one, otherwise claim
    ///   this slot and charge it against the limit.
    /// - **The key itself.** Overwrite in place, live or expired. This costs
    ///   nothing against the limit since the slot is already ours.
    ///
    /// Expired slots we pass are remembered but not taken immediately, and the
    /// ordering here is load-bearing. `key` may still be sitting further down
    /// the chain; grabbing an earlier expired slot on sight would leave two
    /// entries for the same key, and the stale one would resurface as soon as
    /// the newer one expired. Waiting for the empty slot is what proves the key
    /// is genuinely absent.
    ///
    /// # Termination
    ///
    /// `MAX_ENTRIES + 1` iterations is enough by pigeonhole: at most
    /// `MAX_ENTRIES` slots are occupied, so no run of `MAX_ENTRIES + 1`
    /// consecutive slots can be free of an empty one. `CHECK` asserts
    /// `MAX_ENTRIES < CAP`, so the walk can't lap the table either. The bound is
    /// a backstop against an invariant violation, never a semantic limit — which
    /// is why `len` bookkeeping below is not optional.
    pub fn insert(&mut self, key: Key, value: Value, now: Time, ttl: Option<Ttl>) -> InsertResult {
        let expires_at = match ttl {
            Some(ttl) => now.plus(ttl),
            None => Time::NEVER,
        };

        let mut i = Self::bucket_of(key);
        let mut reclaimable: Option<usize> = None;

        for _ in 0..MAX_ENTRIES + 1 {
            // SAFETY: No need for bounds checking. MAX_ENTRIES + 1 is always < CAP
            let entry = unsafe { self.entries.get_unchecked_mut(i) };

            if !entry.is_occupied() {
                match reclaimable {
                    // SAFETY: No need for bounds checking. MAX_ENTRIES + 1 is always < CAP
                    Some(ri) => {
                        unsafe {
                            self.entries
                                .get_unchecked_mut(ri)
                                .update(key, value, expires_at);
                        };
                        return Ok(value);
                    }
                    None if self.len < MAX_ENTRIES => {
                        entry.update(key, value, expires_at);
                        self.len += 1;

                        return Ok(value);
                    }
                    // Full, with nothing to recycle. The write is dropped.
                    None => return Err(InsertError::Full),
                };
            }

            if entry.key == key {
                entry.update(key, value, expires_at);
                return Ok(value);
            }

            if entry.is_expired(now) && reclaimable.is_none() {
                reclaimable = Some(i);
            }

            i = Self::next(i);
        }

        // Unreachable while the entry-limit invariant holds.
        debug_assert!(false, "probe sequence exhausted with len={}", self.len);
        Err(InsertError::ProbeExhaustion)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CAP: usize = 64;
    const TEST_LIMIT: usize = 44;
    type TestMap = XXH3Map<TEST_CAP, TEST_LIMIT>;

    /// Smallest key whose home bucket is `bucket`, offset by `nth`.
    ///
    /// Inverts fastrange: the smallest `k` with `k * CAP >> 64 == bucket` is
    /// `ceil(bucket * 2^64 / CAP)`. Small offsets stay in the same bucket.
    fn key_for_bucket(bucket: usize, nth: usize) -> Key {
        let cap = TEST_CAP as u128;
        let base = ((bucket as u128) << 64).div_ceil(cap);
        Key::from_hash(base as u64 + nth as u64)
    }

    /// A syntactically valid UUID that varies in its last character.
    fn uuid_of(n: u8) -> Value {
        let mut v = *b"00000000-0000-0000-0000-000000000000";
        v[35] = b'0' + (n % 10);
        v.into()
    }

    fn at(secs: u32) -> Time {
        Time::from_uptime_secs(secs)
    }

    #[test]
    fn entry_tiles_cache_lines() {
        assert_eq!(std::mem::size_of::<Entry>(), 32);
        assert_eq!(std::mem::align_of::<Entry>(), 32);
    }

    /// Guards the invariant `XXH3Map::new` depends on.
    #[test]
    fn zero_bit_pattern_is_empty_entry() {
        let zeroed: Entry = unsafe { std::mem::zeroed() };
        assert_eq!(zeroed.key, Key::EMPTY);
        assert_eq!(zeroed.value, StoredValue::EMPTY);
        assert_eq!(zeroed.expires_at, Time::NEVER);
        assert!(!zeroed.is_occupied());
        assert!(!zeroed.is_expired(at(u32::MAX)));
    }

    #[test]
    fn roundtrip() {
        let map = TestMap::new();
        let ttl = Some(Ttl::from_secs(1));
        let t1 = at(1);
        let t2 = at(2);
        let k = key_for_bucket(3, 0);
        let v = uuid_of(7);

        // New map must not have the key before insertion.
        std::assert_matches!(map.get(k, t1), Err(GetError::NotFound));
        assert_eq!(map.len, 0);

        // Key must be present after insert and len must be 1.
        map.insert(k, v, t1, ttl);
        assert_eq!(map.get(k, t1).unwrap(), v);
        assert_eq!(map.len, 1);

        // Key must be expired in the t2 and len must be 0.
        assert_eq!(map.get(k, t2).unwrap(), Value::EMPTY);
        assert_eq!(map.len, 0);
    }

    #[test]
    fn none_expiry_never_expires() {
        let map = TestMap::new();
        let t1 = at(0);
        let t2 = at(u32::MAX);
        let k = key_for_bucket(3, 0);
        let v = uuid_of(1);

        map.insert(k, v, t1, None);
        assert_eq!(map.get(k, t2).unwrap(), v);
    }

    #[test]
    fn overwrite_does_not_grow_len() {
        let map = TestMap::new();
        let t1: Time = at(0);
        let k = key_for_bucket(3, 0);
        let v1 = uuid_of(1);
        let v2 = uuid_of(2);

        map.insert(k, v1, t1, None);
        map.insert(k, v2, t1, None);

        assert_eq!(map.len, 1);
        assert_eq!(map.get(k, t1).unwrap(), v2);
    }

    #[test]
    fn expired_slot_is_reclaimed() {
        let map = TestMap::new();
        let ttl = Some(Ttl::from_secs(0));
        let t1 = at(1);
        let t2 = at(2);
        let k = key_for_bucket(5, 0);
        let k_collission = key_for_bucket(5, 1);
        let v1 = uuid_of(1);
        let v2 = uuid_of(2);

        map.insert(k, v1, t1, ttl);
        assert_eq!(map.len, 1);

        assert_eq!(map.get(k, t2).unwrap(), Value::EMPTY);
        assert_eq!(map.len, 1, "expired entries hold their slot until reused");

        map.insert(k_collission, v2, t2, None);
        assert_eq!(map.len, 1, "reclaiming a slot must not grow len");
        assert_eq!(map.get(k_collission, t2).unwrap(), v2);
        assert_eq!(map.get(k, t2).unwrap(), Value::EMPTY);
    }

    #[test]
    fn expiry() {
        let map = TestMap::new();
        let t1 = at(1);
        let t2 = at(2);
        let ttl = Some(Ttl::from_secs(1));

        let collisions = 2;
        for bucket in 0..TEST_LIMIT / collisions {
            for nth in 0..collisions {
                let k = key_for_bucket(bucket, nth);
                let v = uuid_of(bucket as u8 + 1);
                map.insert(k, v, t1, ttl);
            }
        }
    }

    #[test]
    fn full_table_doesnt_grow_but_accepts_inserts() {
        let map = TestMap::new();
        let t1 = at(1);
        let v1 = uuid_of(1);
        let v2 = uuid_of(2);

        for bucket in 0..TEST_LIMIT {
            let k = key_for_bucket(bucket, 0);
            map.insert(k, v1, t1, None);
        }
        assert_eq!(map.len, TestMap::ENTRY_LIMIT);

        // New entry must not be inserted when full
        let new = key_for_bucket(TEST_LIMIT + 1, 0);
        map.insert(new, v1, t1, None);
        assert_eq!(map.len, TestMap::ENTRY_LIMIT);
        assert_eq!(map.get(new, t1).unwrap(), Value::EMPTY);

        // Existing keys remain reachable and updatable when full.
        let first = key_for_bucket(0, 0);
        assert_eq!(map.get(first, t1).unwrap(), v1);
        map.insert(first, v2, t1, None);
        assert_eq!(map.get(first, t1).unwrap(), v2);
    }

    #[test]
    fn full_table_expired_slot_is_reclaimed() {
        let map = TestMap::new();
        let ttl = Some(Ttl::from_secs(1));
        let t1 = at(1);
        let t2 = at(2);
        let v1: uuid::UUID = uuid_of(1);
        let v2 = uuid_of(2);

        for bucket in 1..TEST_LIMIT + 1 {
            let k = key_for_bucket(bucket, 0);
            map.insert(k, v1, t1, ttl);
        }
        assert_eq!(map.len, TestMap::ENTRY_LIMIT);

        // Assert that the old key is in the table.
        let old = key_for_bucket(1, 0);
        assert_eq!(map.get(old, t1).unwrap(), v1);

        // New key must be inserted even if the table is full
        // because the old key must be considered as expired.
        let new = key_for_bucket(1, 1);
        map.insert(new, v2, t2, None);
        assert_eq!(map.get(new, t2).unwrap(), v2);
        assert_eq!(map.len, TestMap::ENTRY_LIMIT);

        // Assert that the old key is no longer in the table
        assert_eq!(map.get(old, t1).unwrap(), Value::EMPTY);
    }
}
