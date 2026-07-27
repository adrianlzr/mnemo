use clap::Parser;
use rand::Rng;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::task::JoinSet;
use uuid::Uuid;
use xxhash_rust::xxh3::xxh3_64;

#[derive(Parser, Debug)]
#[command(version, about = "UDP GET/UPDATE benchmark client", long_about = None)]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(long, default_value_t = 8080)]
    port: u16,

    /// Number of independent concurrent workers (tokio tasks). This is the
    /// only source of concurrency — see the module docs on why more than
    /// one in-flight request per connection isn't safe with this protocol.
    /// Analogous to Go's number of goroutines, not to a thread count.
    #[arg(long, default_value_t = 1)]
    clients: usize,

    /// Requests per worker. All of them are pre-generated before the
    /// benchmark starts, so a very large value here costs startup time
    /// (UUID + hash generation), not measured throughput.
    #[arg(long, default_value_t = 1)]
    requests: usize,

    /// Real OS threads backing the tokio runtime. Defaults to the CPU
    /// count if unset — the direct equivalent of Go's GOMAXPROCS default.
    /// This is independent of --threads: e.g. --threads 1000
    /// --worker-threads 8 runs 1000 concurrent workers multiplexed onto 8
    /// real OS threads, mirroring how Go would run 1000 goroutines on 8
    /// GOMAXPROCS threads.
    #[arg(long)]
    worker_threads: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Get = 1,
    Update = 2,
}

const OP_SIZE: usize = 1;
const KEY_SIZE: usize = 8;
const VAL_SIZE: usize = 36;

/// The wire protocol always sends a fixed-size 45-byte request
/// (OP + KEY + VALUE) regardless of opcode — the server ignores the value
/// bytes on a GET, but still expects them to be present.
const REQUEST_SIZE: usize = OP_SIZE + KEY_SIZE + VAL_SIZE;

/// NOTE: this assumes a response of VALUE only (36 bytes), matching this
/// project's Rust server's wire format. The Go/C++ servers from earlier in
/// this conversation respond with KEY+VALUE (44 bytes) instead — reading a
/// 44-byte datagram into a 36-byte buffer doesn't error, it silently
/// truncates, so success/latency numbers would still look fine while the
/// actual response content would be wrong. Set this to 44 if you're
/// pointing this client at one of those servers instead.
const RESPONSE_SIZE: usize = VAL_SIZE;

const RESPONSE_TIMEOUT: Duration = Duration::from_millis(10);

/// A single pre-generated wire-format datagram, built before the benchmark
/// clock starts so UUID/hash generation cost is never counted as latency.
struct Request {
    data: [u8; REQUEST_SIZE],
    op: Op,
}

fn rand_op(rng: &mut impl Rng) -> Op {
    if rng.gen_bool(0.5) {
        Op::Get
    } else {
        Op::Update
    }
}

fn rand_key() -> [u8; KEY_SIZE] {
    let bundle_id = Uuid::new_v4().to_string();
    let idfa = Uuid::new_v4().to_string();
    let concat = bundle_id + &idfa;
    let hash = xxh3_64(concat.as_bytes());
    hash.to_be_bytes()
}

fn rand_val() -> [u8; VAL_SIZE] {
    let mut val = [0u8; VAL_SIZE];
    let uuid_str = Uuid::new_v4().to_string(); // 36 chars, fits exactly
    val.copy_from_slice(uuid_str.as_bytes());
    val
}

/// Builds one fixed-size wire request. Value bytes are zero for GET — the
/// server ignores them; keeping the wire size constant is a protocol
/// requirement, not something a GET semantically needs.
fn build_request(rng: &mut impl Rng) -> Request {
    let op = rand_op(rng);
    let mut data = [0u8; REQUEST_SIZE];
    data[0] = op as u8;
    data[OP_SIZE..OP_SIZE + KEY_SIZE].copy_from_slice(&rand_key());
    if op == Op::Update {
        data[OP_SIZE + KEY_SIZE..].copy_from_slice(&rand_val());
    }
    Request { data, op }
}

/// What one worker task reports after finishing its share of requests.
/// Each task owns its own instance entirely and returns it as the task's
/// own result — nothing here is ever touched by more than one task, so no
/// locking is needed anywhere.
struct WorkerResult {
    latencies_ms: Vec<f64>,
    successful: u64,
    failed: u64,
    timedout: u64,
    gets: u64,
    updates: u64,
}

/// Strictly sequential closed-loop client: send, await the matching reply
/// (or time out), record latency, repeat. Exactly one request is ever in
/// flight on `socket` at a time — see the module docs for why that's a
/// correctness requirement, not just a simplification.
async fn run_worker(socket: UdpSocket, requests: Vec<Request>) -> WorkerResult {
    let mut result = WorkerResult {
        latencies_ms: Vec::with_capacity(requests.len()),
        successful: 0,
        failed: 0,
        timedout: 0,
        gets: 0,
        updates: 0,
    };

    // This task's own buffer, reused safely across its own sequential
    // requests — never touched by any other task.
    let mut response = [0u8; RESPONSE_SIZE];

    for req in &requests {
        match req.op {
            Op::Update => result.updates += 1,
            Op::Get => result.gets += 1,
        }

        let start = Instant::now();

        if socket.send(&req.data).await.is_err() {
            result.failed += 1;
            continue;
        }

        match tokio::time::timeout(RESPONSE_TIMEOUT, socket.recv(&mut response)).await {
            Ok(Ok(_)) => {
                result
                    .latencies_ms
                    .push(start.elapsed().as_secs_f64() * 1000.0);
                result.successful += 1;
            }
            Ok(Err(_)) | Err(_) => result.timedout += 1, // real I/O error or timeout elapsed
        }
    }

    result
}

struct BenchmarkStats {
    all_latencies_ms: Vec<f64>,
    total_requests: u64,
    total_successful: u64,
    total_failed: u64,
    total_timedout: u64,
    total_gets: u64,
    total_updates: u64,
    elapsed: Duration,
}

async fn run_benchmark(
    host: &str,
    port: u16,
    num_workers: usize,
    requests_per_worker: usize,
) -> BenchmarkStats {
    println!(
        "Preparing {} workers x {} requests/worker ({} total)...",
        num_workers,
        requests_per_worker,
        num_workers * requests_per_worker
    );

    // Generate every request for every worker BEFORE opening sockets or
    // starting the clock — see module docs.
    let per_worker_requests: Vec<Vec<Request>> = (0..num_workers)
        .map(|_| {
            let mut rng = rand::thread_rng();
            (0..requests_per_worker)
                .map(|_| build_request(&mut rng))
                .collect()
        })
        .collect();

    let target = format!("{host}:{port}");
    let mut sockets = Vec::with_capacity(num_workers);
    for _ in 0..num_workers {
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .expect("failed to bind local socket");
        socket
            .connect(&target)
            .await
            .expect("failed to connect to server");
        sockets.push(socket);
    }

    println!("Starting benchmark...");

    let start = Instant::now();
    let mut tasks: JoinSet<WorkerResult> = JoinSet::new();
    for (socket, requests) in sockets.into_iter().zip(per_worker_requests.into_iter()) {
        tasks.spawn(run_worker(socket, requests));
    }

    let mut stats = BenchmarkStats {
        all_latencies_ms: Vec::new(),
        total_requests: 0,
        total_successful: 0,
        total_failed: 0,
        total_timedout: 0,
        total_gets: 0,
        total_updates: 0,
        elapsed: Duration::default(),
    };

    while let Some(joined) = tasks.join_next().await {
        let result = joined.expect("worker task panicked");
        stats.all_latencies_ms.extend(result.latencies_ms);
        stats.total_successful += result.successful;
        stats.total_failed += result.failed;
        stats.total_timedout += result.timedout;
        stats.total_gets += result.gets;
        stats.total_updates += result.updates;
    }

    stats.elapsed = start.elapsed();
    stats.total_requests = stats.total_successful + stats.total_failed + stats.total_timedout;
    stats
}

fn print_results(s: &BenchmarkStats) {
    if s.all_latencies_ms.is_empty() {
        println!("No successful requests completed");
        return;
    }

    let mut latencies = s.all_latencies_ms.clone();
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let sum: f64 = latencies.iter().sum();
    let avg = sum / latencies.len() as f64;
    let median = latencies[latencies.len() / 2];
    let p95 = latencies[(latencies.len() as f64 * 0.95) as usize];
    let p99 = latencies[(latencies.len() as f64 * 0.99) as usize];

    println!("\n=== BENCHMARK RESULTS ===");
    println!("Total requests:       {}", s.total_requests);
    println!("Get requests:         {}", s.total_gets);
    println!("Update requests:      {}", s.total_updates);
    println!("Successful requests:  {}", s.total_successful);
    println!("Failed requests:      {}", s.total_failed);
    println!("Timedout requests:    {}", s.total_timedout);
    println!(
        "Success rate:         {:.4}%",
        s.total_successful as f64 / s.total_requests as f64 * 100.0
    );
    println!("Total time:           {:.2}s", s.elapsed.as_secs_f64());
    println!(
        "Requests per second:  {:.0}",
        s.total_successful as f64 / s.elapsed.as_secs_f64()
    );
    println!("Average latency:      {avg:.3}ms");
    println!("Median latency:       {median:.3}ms");
    println!("95th percentile:      {p95:.3}ms");
    println!("99th percentile:      {p99:.3}ms");
    println!("Min latency:          {:.3}ms", latencies[0]);
    println!(
        "Max latency:          {:.3}ms",
        latencies[latencies.len() - 1]
    );
}

fn main() {
    let args = Args::parse();

    let mut builder = tokio::runtime::Builder::new_multi_thread();
    if let Some(n) = args.worker_threads {
        builder.worker_threads(n);
    }
    // If --worker-threads isn't set, tokio's own default (the CPU count)
    // applies — the same default philosophy as Go's GOMAXPROCS.
    let runtime = builder
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    let stats = runtime.block_on(run_benchmark(
        &args.host,
        args.port,
        args.clients,
        args.requests,
    ));
    print_results(&stats);
}
