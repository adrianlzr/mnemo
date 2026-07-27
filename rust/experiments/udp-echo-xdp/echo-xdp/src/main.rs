use std::convert::TryInto;

use anyhow::Context as _;
use aya::{
    include_bytes_aligned,
    maps::XskMap,
    programs::{Xdp, XdpFlags},
    Ebpf,
};
use clap::Parser;
use xsk_rs::{
    config::{SocketConfig, UmemConfig},
    socket::Socket,
    umem::Umem,
};

#[derive(Parser, Debug)]
struct Opt {
    /// Network interface to attach to (must support XDP; native/driver mode
    /// gives the real performance win, "generic"/SKB mode also works for testing).
    #[clap(short, long, default_value = "eth0")]
    iface: String,

    /// RX queue to bind the AF_XDP socket to. For multi-queue NICs, run one
    /// instance of this program per queue (pin each to its own core) to use
    /// all queues, mirroring the SO_REUSEPORT-per-thread shape.
    #[clap(short, long, default_value_t = 0)]
    queue: u32,
}

const FRAME_COUNT: u32 = 4096;

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let opt = Opt::parse();

    // Bump memlock rlimit; required for loading eBPF maps/programs.
    let rlim = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) };

    // Load and attach the XDP program built by echo-xdp-ebpf.
    let mut bpf = Ebpf::load(include_bytes_aligned!(concat!(
        env!("OUT_DIR"),
        "/echo-xdp"
    )))?;
    if let Err(e) = aya_log::EbpfLogger::init(&mut bpf) {
        log::warn!("failed to init eBPF logger: {e}");
    }
    let program: &mut Xdp = bpf.program_mut("echo_xdp").unwrap().try_into()?;
    program.load()?;
    program
        .attach(&opt.iface, XdpFlags::default())
        .context("attach XDP program (is the interface XDP-capable?)")?;

    // Set up the UMEM (shared mmap'd packet buffer) and bind an AF_XDP
    // socket to the requested interface/queue.
    let (umem, mut frame_descs) = Umem::new(
        UmemConfig::default(),
        FRAME_COUNT.try_into().unwrap(),
        false,
    )
    .context("create UMEM")?;

    let (mut tx_q, mut rx_q, fq_cq) = unsafe {
        Socket::new(
            SocketConfig::default(),
            &umem,
            &opt.iface.parse().context("parse interface name")?,
            opt.queue,
        )
    }
    .context("bind AF_XDP socket")?;
    let (mut fill_q, mut comp_q) = fq_cq.expect("missing fill/completion queue");

    // Register this socket in XSK_MAP at [queue_id] so the XDP program's
    // redirect() call finds it.
    let mut xsk_map: XskMap<_> = bpf.map_mut("XSK_MAP").unwrap().try_into()?;
    xsk_map.set(opt.queue, tx_q.fd().as_raw_fd() as u32, 0)?;

    // Give the kernel all our frames up front via the fill queue so it has
    // somewhere to write incoming packets.
    unsafe { fill_q.produce(&frame_descs) };

    println!(
        "echoing UDP :9999 on {} queue {} (ctrl-c to stop)",
        opt.iface, opt.queue
    );

    let mut rx_descs = frame_descs.split_off(frame_descs.len() / 2);

    loop {
        // Reclaim frames the kernel has finished transmitting.
        let n_comp = unsafe { comp_q.consume(&mut frame_descs) };
        if n_comp > 0 {
            unsafe { fill_q.produce(&frame_descs[..n_comp]) };
        }

        let n_rx = unsafe { rx_q.poll_and_consume(&mut rx_descs, 100)? };
        if n_rx == 0 {
            continue;
        }

        let mut n_replies = 0;
        for desc in &mut rx_descs[..n_rx] {
            if unsafe { echo_in_place(&umem, desc) } {
                n_replies += 1;
            }
        }

        if n_replies > 0 {
            unsafe { tx_q.produce_and_wakeup(&rx_descs[..n_replies])? };
        }
        // Frames that failed to parse as our echo packet go back on the
        // fill queue unchanged so the kernel can reuse them.
        if n_replies < n_rx {
            unsafe { fill_q.produce(&rx_descs[n_replies..n_rx]) };
        }
    }
}

/// Rewrites a received frame into its own echo reply, in place, and flips
/// its descriptor's length to the reply length. Returns true if the frame
/// was a well-formed Eth/IPv4/UDP packet on the echo port (and is now ready
/// to transmit), false otherwise.
///
/// Because IP/UDP checksums are ones-complement sums over the header words,
/// swapping src<->dst fields (same set of words, different order) leaves
/// the checksum valid — no recomputation needed for a pure mirror-reply.
unsafe fn echo_in_place(umem: &Umem, desc: &mut xsk_rs::FrameDesc) -> bool {
    const ETH_LEN: usize = 14;
    const IP_LEN: usize = 20;
    const UDP_LEN: usize = 8;
    const ECHO_PORT: u16 = 9999;

    let mut data = umem.frame_mut(desc).0;
    if data.len() < ETH_LEN + IP_LEN + UDP_LEN {
        return false;
    }

    // --- Ethernet: swap src/dst MAC ---
    let (eth, rest) = data.split_at_mut(ETH_LEN);
    eth.swap(0, 6);
    eth.swap(1, 7);
    eth.swap(2, 8);
    eth.swap(3, 9);
    eth.swap(4, 10);
    eth.swap(5, 11);
    if eth[12] != 0x08 || eth[13] != 0x00 {
        return false; // not IPv4
    }

    let (ip, rest) = rest.split_at_mut(IP_LEN);
    if ip[9] != 17 {
        return false; // not UDP
    }
    // swap src (bytes 12..16) / dst (bytes 16..20) IP addresses
    for i in 0..4 {
        ip.swap(12 + i, 16 + i);
    }

    let udp = &mut rest[..UDP_LEN];
    let dst_port = u16::from_be_bytes([udp[2], udp[3]]);
    if dst_port != ECHO_PORT {
        return false;
    }
    // swap src (0..2) / dst (2..4) ports
    udp.swap(0, 2);
    udp.swap(1, 3);

    // Length unchanged: payload after the UDP header is echoed as-is.
    true
}
