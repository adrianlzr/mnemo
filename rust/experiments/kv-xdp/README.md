# kv-xdp — read-through KV cache at the XDP layer

GET requests that hit the cache never leave the kernel. UPDATEs, and GETs
that miss, fall through to an ordinary UDP socket in userspace.

```
                    ┌─────────────────────────────┐
 packet in ──────▶  │  XDP program (kv-xdp-ebpf)   │
                    │  parse UDP payload           │
                    │  op == GET?                  │
                    │    hit  → rewrite in place,   │──▶ XDP_TX (reply, never
                    │           XDP_TX              │     leaves the kernel)
                    │    miss → XDP_PASS ───────────┼──▶ normal kernel stack
                    │  op == UPDATE → XDP_PASS ─────┼──▶ normal kernel stack
                    └─────────────────────────────┘         │
                                                              ▼
                                                     UdpSocket in kv-xdp
                                                     (slow path: handles
                                                      UPDATE + cache-miss GET,
                                                      writes into KV_CACHE)
```

## Wire protocol

Deliberately simple, all integers big-endian:

**Request**, UDP payload to port 9998:
- `op: u8` — `0` = GET, `1` = UPDATE
- `key: u64` — pre-hashed key (see note below)
- GET: nothing further
- UPDATE: `value_len: u16`, then `value_len` bytes of value (≤ 128 bytes)

**Reply**:
- GET hit/miss: `len: u16` (0 = not found), then `len` bytes of value
- UPDATE: single byte `1` (ack)

**Why a pre-hashed key instead of an arbitrary key string:** hashing
variable-length keys (e.g. XXH3, like your existing store) inside the eBPF
program is possible but adds real verifier complexity — bounded loops over
attacker-controlled-length input are exactly what the verifier is pickiest
about. Doing the hash client-side (or in the userspace slow path, on first
UPDATE) keeps the fast path to a fixed-width map lookup, which is the part
that actually needs to be cheap.

## Build & run

Same toolchain as the echo server:

```bash
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
cargo install bpf-linker

cd kv-xdp
cargo build --release -p kv-xdp
sudo ./target/release/kv-xdp --iface eth0
```

## Where the real friction will be

Being upfront about this since it's the part most likely to need iteration
on your machine, not just adaptation:

- **The verifier is the hard part, not the Rust.** `bpf_xdp_adjust_tail`
  followed by re-deriving every pointer, plus the bounded copy loop writing
  the cached value into the packet, is exactly the kind of code the eBPF
  verifier is strictest about. Expect to see `cargo build` succeed but the
  program get rejected at `attach()` time with a verifier log — that's
  normal for this class of program, not a sign the approach is wrong.
  `RUST_LOG=debug` on the userspace binary will surface the verifier's
  rejection reason via aya's error output.
- **UDP checksum is disabled (0), not recomputed**, since the payload
  actually changes length and content here (unlike the pure echo case).
  `0` is a legal "no checksum" marker for UDP-over-IPv4, but it means
  corrupted replies won't be detected at the transport layer. If that
  matters for your use case, you'd compute a proper checksum over the
  pseudo-header + variable-length payload — more verifier-unfriendly
  looping, so I left it out of this first pass.
- **`aya`/`kv-xdp-common` Pod requirements**: if you change `KvValue`'s
  layout, keep it `#[repr(C)]` with no padding surprises — the `Pod`
  marker trait asserts the type is safe to move across the kernel/user
  boundary as raw bytes, and a mismatched layout between the two crates
  (they're compiled independently, for different targets) is a silent
  correctness bug, not a compile error.
- **Map sizing**: `LruHashMap::with_max_entries(65536, 0)` is a guess.
  Size it to your actual working set — LRU eviction is transparent but
  means "hot" is defined by recency, not by anything your application
  controls.

## Wiring into your actual engine

The comment in `kv-xdp/src/main.rs` marks where your existing engine
thread's `Dict` write belongs: on every `UPDATE`, write to your `Dict` as
today, and also push into `cache` here so the entry is servable from XDP
on the next `GET`. Your IO threads' channel-based handoff to the engine
thread doesn't need to change — this just adds a second write target
alongside the `Dict`.
