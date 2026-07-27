//! Measures how many gets and updates per second the store handles, and the
//! latency distribution of each, at three table sizes.
//!
//! Run with:
//!
//! ```text
//! taskset -c 2 cargo run -r -p mnemo-udp-capped --bin bench
//! ```
//!
//! Two separate measurements per operation, because they need different loops:
//!
//! - **Throughput** runs a tight loop with no clock reads inside it and divides
//!   at the end. This is the accurate ops/sec figure.
//! - **Latency** times each operation individually, which means two clock reads
//!   per operation. On aarch64 that costs ~20ns a pair — comparable to the
//!   operation itself — so the cost is measured up front and subtracted from
//!   every sample. It's still the noisier of the two numbers.
//!
//! A note on p100: it is almost always a scheduler preemption, a page fault, or
//! the hypervisor stealing time, not anything the store did. Treat it as "worst
//! interruption observed during the run" and pin the process with `taskset` to
//! make it less awful.

use std::hint::black_box;
use std::time::{Duration, Instant};

// Adjust these paths to match your module layout.
use store_capped::{DEFAULT_OCCUPANCY_PCT, Key, Value, XXH3Map, cap_for};
use time::{Time, Ttl};

/// A valid UUID to store. Content is irrelevant; the store never inspects it.
const VALUE: Value = Value::EMPTY;

/// Fixed clock value, so no operation reads the real clock.
const NOW: Time = Time::from_uptime_secs(1_000);

/// How long each throughput loop runs.
const THROUGHPUT_SECS: f64 = 1.5;

/// How many operations get timed individually for the percentile report.
/// At 4 bytes per sample this is 8 MiB of samples.
const LATENCY_SAMPLES: usize = 2_000_000;

fn main() {
    let overhead = clock_overhead_ns();
    println!("clock overhead: {overhead} ns per timed operation (subtracted from all latencies)");
    println!("latency samples per measurement: {LATENCY_SAMPLES}");
    println!();

    run::<{ cap_for(10_000, DEFAULT_OCCUPANCY_PCT) }, 10_000>("xsmall", overhead);
    run::<{ cap_for(100_000, DEFAULT_OCCUPANCY_PCT) }, 100_000>("small", overhead);
    run::<{ cap_for(1_000_000, DEFAULT_OCCUPANCY_PCT) }, 1_000_000>("medium", overhead);
    run::<{ cap_for(10_000_000, DEFAULT_OCCUPANCY_PCT) }, 10_000_000>("large", overhead);
    run::<{ cap_for(100_000_000, DEFAULT_OCCUPANCY_PCT) }, 100_000_000>("xlarge", overhead);
    run::<{ cap_for(300_000_000, DEFAULT_OCCUPANCY_PCT) }, 300_000_000>("3xlarge", overhead);
}

/// Measures one table size end to end and prints two lines.
fn run<const CAP: usize, const MAX_ENTRIES: usize>(label: &str, overhead: u32) {
    let map = XXH3Map::<CAP, MAX_ENTRIES>::new();
    let deadline = Some(Ttl::from_secs(3_600));

    // Random keys. Generated up front so the RNG never runs inside a timed loop.
    let keys = random_keys(MAX_ENTRIES, 0x5EED_0001);

    // let rss_before = rss_bytes();
    // Fill to the entry limit. This is also the warm-up pass: a freshly mmap'd
    // table faults in on first touch at 1-2us per 4 KiB page, which would
    // otherwise dominate everything measured below.
    for &key in &keys {
        map.insert(key, VALUE, NOW, deadline);
    }

    // let rss_after = rss_bytes();

    // Second pass so the lookup path is warm too.
    for &key in &keys {
        black_box(map.get(key, NOW));
    }

    println!(
        "{label} - {} entries in {} slots, {}",
        MAX_ENTRIES,
        CAP,
        mib(std::mem::size_of::<XXH3Map<CAP, MAX_ENTRIES>>())
    );

    // ---- get -------------------------------------------------------------

    let mut index = 0;
    let mut completed = 0usize;
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < THROUGHPUT_SECS {
        for _ in 0..100_000 {
            black_box(map.get(black_box(keys[index]), NOW));
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
        black_box(map.get(key, NOW));
        samples.push(t0.elapsed().as_nanos() as u32);
        index = wrap(index, keys.len());
    }
    report("get", get_rate, samples, overhead);

    // ---- insert ----------------------------------------------------------
    //
    // Overwrites keys that are already present, so no slot is claimed and the
    // table stays at a fixed load factor. That makes the loop repeatable and
    // matches a store whose keys are refreshed rather than churned.

    let mut index = 0;
    let mut completed = 0usize;
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < THROUGHPUT_SECS {
        for _ in 0..100_000 {
            map.insert(black_box(keys[index]), VALUE, NOW, deadline);
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
        map.insert(key, VALUE, NOW, deadline);
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
        "  {:<7} {:>9}   p50 {:<8}   p90 {:<8}   p99 {:<8}   p100 {:<8}",
        name,
        rate_str(rate),
        ns(at(0.50)),
        ns(at(0.90)),
        ns(at(0.99)),
        ns(*samples.last().unwrap()),
    );
}

/// Cost of the two clock reads that wrap every timed operation.
///
/// Measured exactly the way the latency loops measure, so the number is directly
/// subtractable: an empty operation between `Instant::now()` and `elapsed()`.
fn clock_overhead_ns() -> u32 {
    let iterations = 1_000_000u64;
    let mut total = 0u64;
    for _ in 0..iterations {
        let t0 = Instant::now();
        total += t0.elapsed().as_nanos() as u64;
    }
    (total / iterations) as u32
}

/// Distinct pseudorandom keys via splitmix64. Deterministic, no dependencies.
fn random_keys(count: usize, seed: u64) -> Vec<Key> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            Key::from_hash(z ^ (z >> 31))
        })
        .collect()
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

/// Silences the unused-import warning if `Duration` isn't otherwise referenced.
#[allow(dead_code)]
fn _unused(_: Duration) {}
