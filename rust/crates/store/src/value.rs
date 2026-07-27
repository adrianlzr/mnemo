use crate::{
    compression::{Compress, Decompress},
    timestamp::{TimeWizz, Timestamp},
};

pub const RAW_VALUE_SIZE: usize = 36;
pub type RawValue = [u8; RAW_VALUE_SIZE];
pub const EMPTY_RAW_VALUE: RawValue = [0u8; RAW_VALUE_SIZE];

pub const VALUE_DATA_SIZE: usize = 16;
#[derive(Clone, Debug, PartialEq)]
struct ValueData([u8; VALUE_DATA_SIZE]);

#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    data: ValueData,
    expire_at: Timestamp,
}

impl Value {
    pub fn new(data: RawValue, expire_at: Timestamp) -> Self {
        Self {
            data: data.into(),
            expire_at,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expire_at <= Timestamp::now()
    }
}

// Lookup table for hex digit conversion (0-255 mapping)
static HEX_LOOKUP: [u8; 256] = {
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

static HEX_CHARS: [u8; 16] = *b"0123456789ABCDEF";

impl Compress<ValueData> for RawValue {
    #[inline(always)]
    fn compress(&self) -> ValueData {
        let d = self;
        ValueData([
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

impl Decompress<RawValue> for Value {
    #[inline(always)]
    fn decompress(&self) -> RawValue {
        let src = self.data.0;
        let mut dest: RawValue = EMPTY_RAW_VALUE;

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

        dest
    }
}

impl From<RawValue> for ValueData {
    fn from(value: RawValue) -> Self {
        value.compress()
    }
}

impl From<&Value> for RawValue {
    fn from(value: &Value) -> Self {
        value.decompress()
    }
}
