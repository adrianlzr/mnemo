use std::convert::TryInto;
use std::net::UdpSocket;

use anyhow::Context as _;
use aya::{
    include_bytes_aligned,
    maps::lru_hash_map::LruHashMap,
    programs::{Xdp, XdpFlags},
    Ebpf,
};
use clap::Parser;
use kv_xdp_common::{KvValue, KV_PORT, MAX_VALUE_LEN, OP_GET, OP_UPDATE};

#[derive(Parser, Debug)]
struct Opt {
    /// Interface to attach the XDP fast path to.
    #[clap(short, long, default_value = "eth0")]
    iface: String,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let opt = Opt::parse();

    let rlim = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) };

    let mut bpf = Ebpf::load(include_bytes_aligned!(concat!(env!("OUT_DIR"), "/kv-xdp")))?;
    if let Err(e) = aya_log::EbpfLogger::init(&mut bpf) {
        log::warn!("failed to init eBPF logger: {e}");
    }
    let program: &mut Xdp = bpf.program_mut("kv_xdp").unwrap().try_into()?;
    program.load()?;
    program
        .attach(&opt.iface, XdpFlags::default())
        .context("attach XDP program (is the interface XDP-capable?)")?;

    let mut cache: LruHashMap<_, u64, KvValue> =
        LruHashMap::try_from(bpf.map_mut("KV_CACHE").unwrap())?;

    // Slow path: anything the XDP program XDP_PASS'd through — every
    // UPDATE, and any GET that missed the cache — lands here on a
    // completely ordinary socket. This is also where you'd hook in your
    // existing engine-thread/Dict: on UPDATE, write to the Dict as you do
    // today AND push the value into `cache` so the next GET for that key
    // is served straight from XDP.
    let sock = UdpSocket::bind(("0.0.0.0", KV_PORT))?;
    println!(
        "kv-xdp: fast-path GETs served from XDP on {}, slow path (UPDATE + cache-miss GET) on :{KV_PORT}",
        opt.iface
    );

    let mut buf = [0u8; 2048];
    loop {
        let (n, src) = sock.recv_from(&mut buf)?;
        if n < 9 {
            continue; // shorter than op(1) + key(8)
        }
        let op = buf[0];
        let key = u64::from_be_bytes(buf[1..9].try_into().unwrap());

        match op {
            OP_UPDATE if n >= 11 => {
                let requested_len = u16::from_be_bytes(buf[9..11].try_into().unwrap()) as usize;
                let val_len = requested_len.min(MAX_VALUE_LEN).min(n - 11);

                let mut value = KvValue::default();
                value.len = val_len as u16;
                value.data[..val_len].copy_from_slice(&buf[11..11 + val_len]);

                // <-- your existing engine thread's Dict write goes here too.
                cache.insert(&key, &value, 0)?;

                sock.send_to(&[1], src)?; // ack
            }
            OP_GET => {
                // Only reachable on a cache miss — XDP served the hits.
                match cache.get(&key, 0) {
                    Ok(value) => {
                        let mut reply = Vec::with_capacity(2 + value.len as usize);
                        reply.extend_from_slice(&value.len.to_be_bytes());
                        reply.extend_from_slice(&value.data[..value.len as usize]);
                        sock.send_to(&reply, src)?;
                    }
                    Err(_) => {
                        sock.send_to(&0u16.to_be_bytes(), src)?; // not found
                    }
                }
            }
            _ => {}
        }
    }
}
