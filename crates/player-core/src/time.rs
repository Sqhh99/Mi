//! Time formatting for player labels and playlist rows.

/// Shown in the playlist before a track's duration is known.
pub const UNKNOWN_DURATION: &str = "--:--";

/// "mm:ss" with zero-padded minutes, used beside the progress bar.
pub fn fmt_clock(ms: u64) -> String {
    let total = ms / 1000;
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// "m:ss" with unpadded minutes, used in playlist rows.
pub fn fmt_duration(ms: u64) -> String {
    let total = ms / 1000;
    format!("{}:{:02}", total / 60, total % 60)
}

/// "h:mm:ss" (or "m:ss" under an hour), used for queue and album totals.
pub fn fmt_long_duration(ms: u64) -> String {
    let total = ms / 1000;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes}:{seconds:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_clock(0), "00:00");
        assert_eq!(fmt_clock(65_999), "01:05");
        assert_eq!(fmt_clock(6_000_000), "100:00");
        assert_eq!(fmt_duration(0), "0:00");
        assert_eq!(fmt_duration(240_000), "4:00");
        assert_eq!(fmt_duration(61_000), "1:01");
        assert_eq!(fmt_long_duration(61_000), "1:01");
        assert_eq!(fmt_long_duration(3_723_000), "1:02:03");
    }
}
