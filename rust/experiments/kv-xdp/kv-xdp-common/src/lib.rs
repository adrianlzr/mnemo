#![cfg_attr(not(feature = "user"), no_std)]

/// UDP port the KV server listens on.
pub const KV_PORT: u16 = 9998;

/// Max value size the eBPF fast path will serve. Bounded on purpose: the
/// XDP copy loop needs a compile-time-known trip count for the verifier,
/// and larger values eat into the eBPF stack (512 bytes total).
pub const MAX_VALUE_LEN: usize = 128;

/// Request op codes (first byte of the UDP payload).
pub const OP_GET: u8 = 0;
pub const OP_UPDATE: u8 = 1;

/// Cache entry stored in the eBPF LRU hash map, keyed by a pre-hashed u64.
///
/// Note the protocol takes a pre-hashed key directly rather than an
/// arbitrary-length key string — hashing arbitrary keys (e.g. XXH3) inside
/// the eBPF program is possible but adds real verifier complexity for no
/// benefit here. Client-side (or the userspace engine thread) does the
/// hashing; the fast path just does a fixed-width lookup.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvValue {
    pub len: u16,
    pub _pad: [u8; 6],
    pub data: [u8; MAX_VALUE_LEN],
}

impl Default for KvValue {
    fn default() -> Self {
        Self {
            len: 0,
            _pad: [0; 6],
            data: [0; MAX_VALUE_LEN],
        }
    }
}

// Map values crossing the kernel/userspace boundary must be plain-old-data.
// This impl has to live here (not in the userspace crate) because of Rust's
// orphan rules: neither `aya::Pod` nor `KvValue` is local to the userspace
// crate, but KvValue *is* local to this crate.
#[cfg(feature = "user")]
unsafe impl aya::Pod for KvValue {}
