#![no_std]
#![no_main]

use aya_ebpf::{
    bindings::xdp_action,
    helpers::bpf_xdp_adjust_tail,
    macros::{map, xdp},
    maps::LruHashMap,
    programs::XdpContext,
};
use aya_log_ebpf::info;
use kv_xdp_common::{KvValue, KV_PORT, MAX_VALUE_LEN, OP_GET};

const ETH_LEN: usize = 14;
const IP_LEN: usize = 20; // no IP options
const UDP_LEN: usize = 8;
const HDR_LEN: usize = ETH_LEN + IP_LEN + UDP_LEN;
/// Request payload: 1 byte op + 8 byte key.
const REQ_HDR_LEN: usize = 9;

#[map]
static KV_CACHE: LruHashMap<u64, KvValue> = LruHashMap::with_max_entries(65536, 0);

#[xdp]
pub fn kv_xdp(ctx: XdpContext) -> u32 {
    match try_kv_xdp(ctx) {
        Ok(action) => action,
        Err(_) => xdp_action::XDP_PASS,
    }
}

/// Bounds-checks `[offset, offset+len)` against the current packet and
/// returns the address of `offset` if it's in range. Must be re-derived
/// after any call that can move data_end (i.e. bpf_xdp_adjust_tail) —
/// pointers taken before that call are not valid after it.
#[inline(always)]
fn ptr_at(ctx: &XdpContext, offset: usize, len: usize) -> Result<usize, ()> {
    let start = ctx.data();
    let end = ctx.data_end();
    if start + offset + len > end {
        return Err(());
    }
    Ok(start + offset)
}

fn try_kv_xdp(ctx: XdpContext) -> Result<u32, ()> {
    // --- parse & filter down to "this is a GET for us" ---
    let eth = ptr_at(&ctx, 0, ETH_LEN)?;
    let ethertype = unsafe { u16::from_be_bytes([*((eth + 12) as *const u8), *((eth + 13) as *const u8)]) };
    if ethertype != 0x0800 {
        return Ok(xdp_action::XDP_PASS); // not IPv4
    }

    let ip = ptr_at(&ctx, ETH_LEN, IP_LEN)?;
    let proto = unsafe { *((ip + 9) as *const u8) };
    if proto != 17 {
        return Ok(xdp_action::XDP_PASS); // not UDP
    }

    let udp = ptr_at(&ctx, ETH_LEN + IP_LEN, UDP_LEN)?;
    let dst_port = unsafe { u16::from_be_bytes([*((udp + 2) as *const u8), *((udp + 3) as *const u8)]) };
    if dst_port != KV_PORT {
        return Ok(xdp_action::XDP_PASS); // not our port
    }

    let payload = ptr_at(&ctx, HDR_LEN, REQ_HDR_LEN)?;
    let op = unsafe { *(payload as *const u8) };
    if op != OP_GET {
        return Ok(xdp_action::XDP_PASS); // UPDATEs always go to userspace
    }

    let mut key_bytes = [0u8; 8];
    for (i, b) in key_bytes.iter_mut().enumerate() {
        *b = unsafe { *((payload + 1 + i) as *const u8) };
    }
    let key = u64::from_be_bytes(key_bytes);

    // --- cache lookup: miss falls through to the normal kernel stack,
    // where userspace answers it and (in this design) can populate the
    // cache for next time ---
    let value = match unsafe { KV_CACHE.get(&key) } {
        Some(v) => *v,
        None => return Ok(xdp_action::XDP_PASS),
    };

    info!(&ctx, "serving cached GET for key {}", key);

    let val_len = (value.len as usize).min(MAX_VALUE_LEN);

    // --- resize the packet: reply payload is [len:u16][value bytes],
    // request payload was [op:u8][key:u64] — different sizes, so the
    // packet has to grow or shrink to match ---
    let new_payload_len = 2 + val_len;
    let old_total = ctx.data_end() - ctx.data();
    let old_payload_len = old_total.saturating_sub(HDR_LEN);
    let delta = new_payload_len as i32 - old_payload_len as i32;

    if unsafe { bpf_xdp_adjust_tail(ctx.ctx, delta) } != 0 {
        return Ok(xdp_action::XDP_ABORTED);
    }

    // Packet memory may have moved: every pointer must be re-derived here.
    let eth = ptr_at(&ctx, 0, ETH_LEN)?;
    let ip = ptr_at(&ctx, ETH_LEN, IP_LEN)?;
    let udp = ptr_at(&ctx, ETH_LEN + IP_LEN, UDP_LEN)?;
    let payload = ptr_at(&ctx, HDR_LEN, new_payload_len)?;

    unsafe {
        // Ethernet: swap src/dst MAC (6 bytes each).
        for i in 0..6 {
            let a = *((eth + i) as *const u8);
            let b = *((eth + 6 + i) as *const u8);
            *((eth + i) as *mut u8) = b;
            *((eth + 6 + i) as *mut u8) = a;
        }

        // IPv4: swap src/dst, fix total length, recompute header checksum
        // (the swap alone wouldn't need this, but the length change does).
        for i in 0..4 {
            let a = *((ip + 12 + i) as *const u8);
            let b = *((ip + 16 + i) as *const u8);
            *((ip + 12 + i) as *mut u8) = b;
            *((ip + 16 + i) as *mut u8) = a;
        }
        let new_total_len = (IP_LEN + UDP_LEN + new_payload_len) as u16;
        let tl = new_total_len.to_be_bytes();
        *((ip + 2) as *mut u8) = tl[0];
        *((ip + 3) as *mut u8) = tl[1];
        *((ip + 10) as *mut u8) = 0; // zero checksum field before recompute
        *((ip + 11) as *mut u8) = 0;
        let csum = ip_checksum(ip).to_be_bytes();
        *((ip + 10) as *mut u8) = csum[0];
        *((ip + 11) as *mut u8) = csum[1];

        // UDP: swap ports, fix length, disable checksum (0 is a valid
        // "no checksum" marker for UDP-over-IPv4 — cheaper than recomputing
        // over a variable-length payload; see README for the tradeoff).
        for i in 0..2 {
            let a = *((udp + i) as *const u8);
            let b = *((udp + 2 + i) as *const u8);
            *((udp + i) as *mut u8) = b;
            *((udp + 2 + i) as *mut u8) = a;
        }
        let new_udp_len = (UDP_LEN + new_payload_len) as u16;
        let ul = new_udp_len.to_be_bytes();
        *((udp + 4) as *mut u8) = ul[0];
        *((udp + 5) as *mut u8) = ul[1];
        *((udp + 6) as *mut u8) = 0;
        *((udp + 7) as *mut u8) = 0;

        // Payload: 2-byte length prefix + the cached value.
        let lp = (val_len as u16).to_be_bytes();
        *(payload as *mut u8) = lp[0];
        *((payload + 1) as *mut u8) = lp[1];
        let mut i = 0;
        while i < MAX_VALUE_LEN {
            if i >= val_len {
                break;
            }
            *((payload + 2 + i) as *mut u8) = value.data[i];
            i += 1;
        }
    }

    Ok(xdp_action::XDP_TX)
}

/// Standard Internet checksum (RFC 1071) over the 20-byte IPv4 header.
/// Unrolled to a fixed 10 iterations so the verifier can see it terminates.
#[inline(always)]
fn ip_checksum(ip: usize) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i < IP_LEN {
        let word = unsafe { u16::from_be_bytes([*((ip + i) as *const u8), *((ip + i + 1) as *const u8)]) };
        sum += word as u32;
        i += 2;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
