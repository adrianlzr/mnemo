use std::sync::LazyLock;
use std::time::Instant;

static START: LazyLock<Instant> = LazyLock::new(Instant::now);
pub type Timestamp = u64;

pub trait TimeWizz {
    fn now() -> Self;
    fn future(by: Self) -> Self;
    fn bucket<const BUCKET_MS: Timestamp>(self) -> Self;
}

impl TimeWizz for Timestamp {
    #[inline]
    fn now() -> Self {
        START.elapsed().as_millis() as Self
    }

    fn future(by: Self) -> Self {
        Self::now() + by
    }

    /// Rounds down to the nearest multiple of `BUCKET_MS` (e.g. 230 → 200 for
    /// BUCKET_MS = 100). Integer division truncates the remainder, so `/ then *`
    /// is not a no-op — it snaps the value to the bucket boundary below it.
    #[inline(always)]
    fn bucket<const BUCKET_MS: Timestamp>(self) -> Self {
        (self / BUCKET_MS) * BUCKET_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL_WINDOW_MS: Timestamp = 100;

    #[test]
    fn timestamp_bucket_matches_expected() {
        assert_eq!((350 as Timestamp).bucket::<TTL_WINDOW_MS>(), 300)
    }
}
