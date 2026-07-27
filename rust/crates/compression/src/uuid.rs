use crate::compression::{Compress, Decompress};

const UUID_SIZE: usize = 36;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct UUID([u8; UUID_SIZE]);

impl UUID {
    pub const SIZE: usize = UUID_SIZE;
    pub const EMPTY: Self = Self([0u8; UUID_SIZE]);

    pub const fn default() -> Self {
        Self::EMPTY
    }

    #[inline]
    pub const fn from_wire(bytes: &[u8]) -> Self {
        let mut b = [0u8; UUID_SIZE];
        b.copy_from_slice(bytes);

        Self(b)
    }

    #[inline]
    pub const fn into_wire(self) -> [u8; UUID_SIZE] {
        let mut b = [0u8; UUID_SIZE];
        b.copy_from_slice(&self.0);

        b
    }
}

pub const COMPRESSED_UUID_SIZE: usize = 16;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CompressedUUID([u8; COMPRESSED_UUID_SIZE]);
impl CompressedUUID {
    pub const EMPTY: Self = Self([0u8; COMPRESSED_UUID_SIZE]);

    pub const fn default() -> Self {
        Self::EMPTY
    }
}

pub static HEX_LOOKUP: [u8; 256] = {
    let mut table = [0xFFu8; 256];
    table[b'0' as usize] = 0;
    table[b'1' as usize] = 1;
    table[b'2' as usize] = 2;
    table[b'3' as usize] = 3;
    table[b'4' as usize] = 4;
    table[b'5' as usize] = 5;
    table[b'6' as usize] = 6;
    table[b'7' as usize] = 7;
    table[b'8' as usize] = 8;
    table[b'9' as usize] = 9;
    table[b'a' as usize] = 10;
    table[b'b' as usize] = 11;
    table[b'c' as usize] = 12;
    table[b'd' as usize] = 13;
    table[b'e' as usize] = 14;
    table[b'f' as usize] = 15;
    table[b'A' as usize] = 10;
    table[b'B' as usize] = 11;
    table[b'C' as usize] = 12;
    table[b'D' as usize] = 13;
    table[b'E' as usize] = 14;
    table[b'F' as usize] = 15;
    table
};
impl Compress<CompressedUUID> for UUID {
    #[inline(always)]
    fn compress(&self) -> CompressedUUID {
        let d = self.0;
        CompressedUUID([
            (HEX_LOOKUP[d[0] as usize] << 4) | HEX_LOOKUP[d[1] as usize],
            (HEX_LOOKUP[d[2] as usize] << 4) | HEX_LOOKUP[d[3] as usize],
            (HEX_LOOKUP[d[4] as usize] << 4) | HEX_LOOKUP[d[5] as usize],
            (HEX_LOOKUP[d[6] as usize] << 4) | HEX_LOOKUP[d[7] as usize],
            (HEX_LOOKUP[d[9] as usize] << 4) | HEX_LOOKUP[d[10] as usize],
            (HEX_LOOKUP[d[11] as usize] << 4) | HEX_LOOKUP[d[12] as usize],
            (HEX_LOOKUP[d[14] as usize] << 4) | HEX_LOOKUP[d[15] as usize],
            (HEX_LOOKUP[d[16] as usize] << 4) | HEX_LOOKUP[d[17] as usize],
            (HEX_LOOKUP[d[19] as usize] << 4) | HEX_LOOKUP[d[20] as usize],
            (HEX_LOOKUP[d[21] as usize] << 4) | HEX_LOOKUP[d[22] as usize],
            (HEX_LOOKUP[d[24] as usize] << 4) | HEX_LOOKUP[d[25] as usize],
            (HEX_LOOKUP[d[26] as usize] << 4) | HEX_LOOKUP[d[27] as usize],
            (HEX_LOOKUP[d[28] as usize] << 4) | HEX_LOOKUP[d[29] as usize],
            (HEX_LOOKUP[d[30] as usize] << 4) | HEX_LOOKUP[d[31] as usize],
            (HEX_LOOKUP[d[32] as usize] << 4) | HEX_LOOKUP[d[33] as usize],
            (HEX_LOOKUP[d[34] as usize] << 4) | HEX_LOOKUP[d[35] as usize],
        ])
    }
}

pub static HEX_CHARS: [u8; 16] = *b"0123456789ABCDEF";
impl Decompress<UUID> for CompressedUUID {
    #[inline(always)]
    fn decompress(&self) -> UUID {
        let mut dest = [0u8; UUID_SIZE];
        let src = self.0;

        unsafe {
            // First 8 hex chars (4 bytes)
            *dest.get_unchecked_mut(0) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(0) >> 4) as usize);
            *dest.get_unchecked_mut(1) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(0) & 0xF) as usize);
            *dest.get_unchecked_mut(2) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(1) >> 4) as usize);
            *dest.get_unchecked_mut(3) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(1) & 0xF) as usize);
            *dest.get_unchecked_mut(4) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(2) >> 4) as usize);
            *dest.get_unchecked_mut(5) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(2) & 0xF) as usize);
            *dest.get_unchecked_mut(6) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(3) >> 4) as usize);
            *dest.get_unchecked_mut(7) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(3) & 0xF) as usize);

            *dest.get_unchecked_mut(8) = b'-';

            // Next 4 hex chars (2 bytes)
            *dest.get_unchecked_mut(9) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(4) >> 4) as usize);
            *dest.get_unchecked_mut(10) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(4) & 0xF) as usize);
            *dest.get_unchecked_mut(11) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(5) >> 4) as usize);
            *dest.get_unchecked_mut(12) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(5) & 0xF) as usize);

            *dest.get_unchecked_mut(13) = b'-';

            // Next 4 hex chars (2 bytes)
            *dest.get_unchecked_mut(14) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(6) >> 4) as usize);
            *dest.get_unchecked_mut(15) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(6) & 0xF) as usize);
            *dest.get_unchecked_mut(16) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(7) >> 4) as usize);
            *dest.get_unchecked_mut(17) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(7) & 0xF) as usize);

            *dest.get_unchecked_mut(18) = b'-';

            // Next 4 hex chars (2 bytes)
            *dest.get_unchecked_mut(19) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(8) >> 4) as usize);
            *dest.get_unchecked_mut(20) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(8) & 0xF) as usize);
            *dest.get_unchecked_mut(21) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(9) >> 4) as usize);
            *dest.get_unchecked_mut(22) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(9) & 0xF) as usize);

            *dest.get_unchecked_mut(23) = b'-';

            // Last 12 hex chars (6 bytes)
            *dest.get_unchecked_mut(24) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(10) >> 4) as usize);
            *dest.get_unchecked_mut(25) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(10) & 0xF) as usize);
            *dest.get_unchecked_mut(26) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(11) >> 4) as usize);
            *dest.get_unchecked_mut(27) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(11) & 0xF) as usize);
            *dest.get_unchecked_mut(28) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(12) >> 4) as usize);
            *dest.get_unchecked_mut(29) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(12) & 0xF) as usize);
            *dest.get_unchecked_mut(30) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(13) >> 4) as usize);
            *dest.get_unchecked_mut(31) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(13) & 0xF) as usize);
            *dest.get_unchecked_mut(32) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(14) >> 4) as usize);
            *dest.get_unchecked_mut(33) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(14) & 0xF) as usize);
            *dest.get_unchecked_mut(34) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(15) >> 4) as usize);
            *dest.get_unchecked_mut(35) =
                *HEX_CHARS.get_unchecked((*src.get_unchecked(15) & 0xF) as usize);
        }

        UUID(dest)
    }
}

impl From<&[u8]> for UUID {
    fn from(value: &[u8]) -> Self {
        Self::from_wire(value)
    }
}

impl From<[u8; 36]> for UUID {
    fn from(value: [u8; 36]) -> UUID {
        Self(value)
    }
}

impl From<UUID> for [u8; 36] {
    fn from(value: UUID) -> [u8; 36] {
        value.0
    }
}

impl From<UUID> for CompressedUUID {
    fn from(v: UUID) -> Self {
        v.compress()
    }
}

impl From<CompressedUUID> for UUID {
    fn from(v: CompressedUUID) -> Self {
        v.decompress()
    }
}
