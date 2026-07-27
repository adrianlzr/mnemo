# udp-echo-xdp

A UDP echo server on port 9999 built from two pieces:

1. **`echo-xdp-ebpf`** — an XDP program (runs in the NIC driver, in-kernel).
   It parses just enough of each packet (Eth/IPv4/UDP) to check the
   destination port, and redirects matching packets straight into an
   AF_XDP socket via `XSK_MAP`. Everything else gets `XDP_PASS`ed to the
   normal kernel stack untouched.
2. **`echo-xdp`** — a userspace program that loads/attaches that XDP
   program (via `aya`), binds an AF_XDP socket (via `xsk-rs`), and runs
   the actual echo loop entirely off the shared UMEM ring buffers — no
   `recvfrom`/`sendto` syscalls per packet, no `sk_buff` allocation.

The reply is built by swapping src/dst MAC, IP, and port **in place** and
retransmitting the same buffer. Both IP and UDP checksums are ones-complement
sums over their header words — swapping src/dst just reorders the same words,
so the existing checksum stays valid without recomputation.

## Build

```bash
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
cargo install bpf-linker

cd udp-echo-xdp
cargo build --release -p echo-xdp   # build.rs cross-compiles the ebpf crate too
```

## Run

```bash
sudo ./target/release/echo-xdp --iface eth0 --queue 0
```

Test from another host (or another interface in a veth/netns setup):

```bash
socat - UDP:<target-ip>:9999
```

## Things you'll need to adjust for your setup

- **NIC/driver support.** Native XDP (the real performance path) needs a
  driver that implements `ndo_bpf`/XDP hooks (most modern Intel `ixgbe`/`i40e`,
  Mellanox `mlx5`, etc.). On anything else, `XdpFlags::default()` will fall
  back to generic/SKB mode, which still works for correctness testing but
  loses most of the throughput benefit — it re-enters the normal RX path
  before your XDP program runs.
- **Multi-queue scaling.** This binds one AF_XDP socket to one RX queue.
  For real throughput you'd run one instance of `echo-xdp` per queue
  (`--queue 0`, `--queue 1`, ...), each pinned to the core servicing that
  queue's IRQ — the direct analogue of your per-thread `SO_REUSEPORT`
  workers, just below the kernel socket layer instead of at it.
- **`aya`/`xsk-rs` API drift.** Both crates move fast; pin exact versions
  in `Cargo.toml` once you've got it building, and check `docs.rs` for the
  versions you land on if something doesn't match — map/type names and
  queue method signatures have changed across releases.
- **No IPv4 options / no fragmentation handling.** The header offsets
  assume a bare 20-byte IPv4 header. Extend `ptr_at` math if you need to
  handle options.
- **Testing without real hardware:** a `veth` pair + separate network
  namespaces is the standard way to exercise this locally before deploying
  to a box with a real XDP-capable NIC — `xsk-rs`'s own test suite does
  exactly that.

## Comparing against your UDP KV store

If your 160k RPS ceiling turns out to be kernel-socket-layer overhead
(interrupt/syscall/`sk_buff` cost) rather than your engine-thread/channel
design, this is the tool that specifically removes that layer — worth
benchmarking side by side with the `SO_REUSEPORT` version under the same
load generator to see where the number actually moves.
