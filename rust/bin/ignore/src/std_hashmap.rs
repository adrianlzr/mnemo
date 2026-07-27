//! Baseline for comparison against `XXH3Map`: the same measurements, run
//! against `std::collections::HashMap`.
//!
//! Run with:
//!
//! ```text
//! taskset -c 2 cargo run -r -p mnemo-udp-capped --bin bench_hashmap
//! ```
//!
//! Same loops, same overhead subtraction, same output format as `bench.rs`, so
//! the two outputs line up side by side.
//!
//! Two hasher variants, because they answer different questions:
//!
//! - **Default (SipHash-1-3)** is what you get from `HashMap::new()`. Your keys
//!   are already xxh3 hashes, so this hashes a hash — real work that buys
//!   nothing here. It's the honest baseline for "what if I'd just used a
//!   HashMap".
//! - **Identity** feeds the key straight through as its own hash. This is the
//!   apples-to-apples comparison: same work as `XXH3Map` on the hashing side, so
//!   the difference is purely data structure. Safe here because xxh3 distributes
//!   all 64 bits well, and hashbrown wants the top 7 bits for control bytes and
//!   the low bits for the bucket index.
//!
//! Semantics are matched to the store: the value carries an expiry, and a
//! lookup on an expired key reports nothing. Capacity is reserved up front so
//! no rehash happens mid-run.
//!
//! Three caveats that favour the HashMap in this comparison, worth keeping in
//! mind when reading the numbers:
//!
//! - It stores the uncompressed 36-byte UUID, where the store compresses to 16.
//!   That shows up in the RSS column, not the latency ones.
//! - It has no expiry reclamation and no occupancy ceiling — it will happily
//!   rehash and grow, which the store deliberately cannot do.
//! - `insert` returns the previous value, so the overwrite path moves 40 bytes
//!   the store's `update` doesn't.

use std::collections::HashMap;
use std::hash::{BuildHasher, BuildHasherDefault, Hasher};
use std::hint::black_box;
use std::time::Instant;

// Adjust these paths to match your module layout.
use store_capped::{Key, Value};
use time::{Time, Ttl};

const VALUE: Value = Value::EMPTY;
const NOW: Time = Time::from_uptime_secs(1_000);
const THROUGHPUT_SECS: f64 = 1.5;
const LATENCY_SAMPLES: usize = 2_000_000;

const SIZES: [(&str, usize); 5] = [
    ("xsmall", 10_000),
    ("small", 100_000),
    ("medium", 1_000_000),
    ("large", 10_000_000),
    ("xlarge", 100_000_000),
];

/// What the store keeps in an entry, minus the compression: a deadline and a
/// value. 40 bytes, so `(u64, Record)` is 48 per hashbrown bucket.
#[derive(Clone, Copy)]
struct Record {
    expires_at: Time,
    value: Value,
}

fn main() {
    let overhead = clock_overhead_ns();
    println!("clock overhead: {overhead} ns per timed operation (subtracted from all latencies)");
    println!("latency samples per measurement: {LATENCY_SAMPLES}");

    println!("\n=== std HashMap, default hasher (SipHash-1-3) ===\n");
    for (label, entries) in SIZES {
        run::<std::collections::hash_map::RandomState>(label, entries, overhead);
    }

    println!("=== std HashMap, identity hasher (key used as its own hash) ===\n");
    for (label, entries) in SIZES {
        run::<BuildHasherDefault<IdentityHasher>>(label, entries, overhead);
    }
}

/// Measures one table size end to end and prints two lines.
fn run<S>(label: &str, max_entries: usize, overhead: u32)
where
    S: BuildHasher + Default,
{
    let deadline = NOW.plus(Ttl::from_secs(3_600));
    let record = Record {
        expires_at: deadline,
        value: VALUE,
    };

    // Keys first, so they aren't counted in the map's RSS delta below. Stored as
    // raw u64 because that's what a HashMap key would naturally be.
    let keys: Vec<u64> = random_keys(max_entries, 0x5EED_0001);

    let rss_before = rss_bytes();

    // Reserve up front: a rehash partway through the fill would be measuring
    // allocator behaviour, and the store cannot rehash at all.
    let mut map: HashMap<u64, Record, S> =
        HashMap::with_capacity_and_hasher(max_entries, S::default());

    for &key in &keys {
        map.insert(key, record);
    }

    let rss_after = rss_bytes();

    // Warm the lookup path.
    for &key in &keys {
        black_box(map.get(&key));
    }

    println!(
        "{label} - {} entries, capacity {}, rss +{}",
        max_entries,
        map.capacity(),
        mib(rss_after.saturating_sub(rss_before))
    );

    // ---- get -------------------------------------------------------------

    let mut index = 0;
    let mut completed = 0usize;
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < THROUGHPUT_SECS {
        for _ in 0..100_000 {
            let key = black_box(keys[index]);
            black_box(match map.get(&key) {
                Some(found) if !found.expires_at.is_past_at(NOW) => Some(found.value),
                _ => None,
            });
            index = wrap(index, keys.len());
        }
        completed += 100_000;
    }
    let get_rate = completed as f64 / start.elapsed().as_secs_f64();

    let mut samples = Vec::with_capacity(LATENCY_SAMPLES);
    let mut index = 0;
    for _ in 0..LATENCY_SAMPLES {
        let key = keys[index];
        let t0 = Instant::now();
        black_box(match map.get(&key) {
            Some(found) if !found.expires_at.is_past_at(NOW) => Some(found.value),
            _ => None,
        });
        samples.push(t0.elapsed().as_nanos() as u32);
        index = wrap(index, keys.len());
    }
    report("get", get_rate, samples, overhead);

    // ---- update ----------------------------------------------------------
    //
    // Overwrites keys already present, so the map neither grows nor rehashes —
    // the same steady state the store's benchmark measures.

    let mut index = 0;
    let mut completed = 0usize;
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < THROUGHPUT_SECS {
        for _ in 0..100_000 {
            black_box(map.insert(black_box(keys[index]), record));
            index = wrap(index, keys.len());
        }
        completed += 100_000;
    }
    let update_rate = completed as f64 / start.elapsed().as_secs_f64();

    let mut samples = Vec::with_capacity(LATENCY_SAMPLES);
    let mut index = 0;
    for _ in 0..LATENCY_SAMPLES {
        let key = keys[index];
        let t0 = Instant::now();
        black_box(map.insert(key, record));
        samples.push(t0.elapsed().as_nanos() as u32);
        index = wrap(index, keys.len());
    }
    report("update", update_rate, samples, overhead);

    println!();
}

/// Sorts the samples, strips the measurement overhead, and prints one line.
fn report(name: &str, rate: f64, mut samples: Vec<u32>, overhead: u32) {
    for sample in samples.iter_mut() {
        *sample = sample.saturating_sub(overhead);
    }
    samples.sort_unstable();

    let at = |quantile: f64| {
        let last = samples.len() - 1;
        samples[(last as f64 * quantile) as usize]
    };

    println!(
        "  {:<7} {:>9}   p50 {:>8}   p90 {:>8}   p99 {:>8}   p100 {:>8}",
        name,
        rate_str(rate),
        ns(at(0.50)),
        ns(at(0.90)),
        ns(at(0.99)),
        ns(*samples.last().unwrap()),
    );
}

/// Passes a `u64` key through unchanged as its own hash.
///
/// Sound for this workload only because the keys are already xxh3 output. Feed
/// it sequential integers and hashbrown's bucket distribution collapses.
#[derive(Default)]
struct IdentityHasher(u64);

impl Hasher for IdentityHasher {
    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    /// `u64`'s `Hash` impl calls `write_u64`, so this is never reached for our
    /// key type. Folded rather than `unreachable!` so a key-type change degrades
    /// instead of panicking.
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = self.0.rotate_left(8) ^ byte as u64;
        }
    }
}

/// Cost of the two clock reads that wrap every timed operation.
fn clock_overhead_ns() -> u32 {
    let iterations = 1_000_000u64;
    let mut total = 0u64;
    for _ in 0..iterations {
        let t0 = Instant::now();
        total += t0.elapsed().as_nanos() as u64;
    }
    (total / iterations) as u32
}

/// Distinct pseudorandom keys via splitmix64, folded the same way `Key` folds
/// them so both benchmarks see the same key space.
fn random_keys(count: usize, seed: u64) -> Vec<u64> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            Key::from_hash(z ^ (z >> 31)).as_u64()
        })
        .collect()
}

/// Resident set size, for comparing real memory footprint rather than a
/// capacity estimate. Linux only.
fn rss_bytes() -> usize {
    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
        let pages: usize = statm
            .split_whitespace()
            .nth(1)
            .and_then(|field| field.parse().ok())
            .unwrap_or(0);
        pages * 4096
    }
    #[cfg(not(target_os = "linux"))]
    0
}

#[inline]
fn wrap(index: usize, len: usize) -> usize {
    let next = index + 1;
    if next == len { 0 } else { next }
}

fn ns(value: u32) -> String {
    if value < 1_000 {
        format!("{value} ns")
    } else if value < 1_000_000 {
        format!("{:.1} us", value as f64 / 1_000.0)
    } else {
        format!("{:.1} ms", value as f64 / 1_000_000.0)
    }
}

fn rate_str(ops_per_sec: f64) -> String {
    format!("{:.2} M/s", ops_per_sec / 1_000_000.0)
}

fn mib(bytes: usize) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}
