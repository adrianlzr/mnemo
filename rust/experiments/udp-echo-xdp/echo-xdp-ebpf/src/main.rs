#![no_std]
#![no_main]

use aya_ebpf::{
    bindings::xdp_action,
    macros::{map, xdp},
    maps::XskMap,
    programs::XdpContext,
};
use aya_log_ebpf::info;
use core::mem;
use network_types::{
    eth::{EthHdr, EtherType},
    ip::{IpProto, Ipv4Hdr},
    udp::UdpHdr,
};

/// UDP port the echo server listens on. Only packets to this port get
/// redirected to userspace via AF_XDP; everything else falls through to the
/// normal kernel network stack.
const ECHO_PORT: u16 = 9999;

/// Maps RX queue index -> bound AF_XDP socket fd. Userspace populates this
/// after binding its socket(s); one entry per RX queue you're listening on.
#[map]
static XSK_MAP: XskMap = XskMap::with_max_entries(64, 0);

#[xdp]
pub fn echo_xdp(ctx: XdpContext) -> u32 {
    match try_echo_xdp(&ctx) {
        Ok(action) => action,
        Err(_) => xdp_action::XDP_PASS,
    }
}

#[inline(always)]
fn ptr_at<T>(ctx: &XdpContext, offset: usize) -> Result<*const T, ()> {
    let start = ctx.data();
    let end = ctx.data_end();
    let len = mem::size_of::<T>();
    if start + offset + len > end {
        return Err(());
    }
    Ok((start + offset) as *const T)
}

fn try_echo_xdp(ctx: &XdpContext) -> Result<u32, ()> {
    let eth_hdr: *const EthHdr = ptr_at(ctx, 0)?;
    if unsafe { (*eth_hdr).ether_type } != EtherType::Ipv4 {
        return Ok(xdp_action::XDP_PASS);
    }

    let ip_hdr: *const Ipv4Hdr = ptr_at(ctx, EthHdr::LEN)?;
    if unsafe { (*ip_hdr).proto } != IpProto::Udp {
        return Ok(xdp_action::XDP_PASS);
    }

    let udp_hdr: *const UdpHdr = ptr_at(ctx, EthHdr::LEN + Ipv4Hdr::LEN)?;
    let dst_port = u16::from_be(unsafe { (*udp_hdr).dest });

    if dst_port != ECHO_PORT {
        return Ok(xdp_action::XDP_PASS);
    }

    // RX queue index doubles as the XSK_MAP key: userspace binds one AF_XDP
    // socket per queue it wants to service, and registers it at that index.
    let queue_id = unsafe { (*ctx.ctx).rx_queue_index };

    info!(ctx, "redirecting echo packet from queue {}", queue_id);

    // On XSK_MAP miss (no socket bound for this queue), fall back to XDP_PASS
    // instead of dropping, so the packet just goes through the normal stack.
    match XSK_MAP.redirect(queue_id, xdp_action::XDP_PASS as u64) {
        Ok(action) => Ok(action),
        Err(_) => Ok(xdp_action::XDP_PASS),
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
