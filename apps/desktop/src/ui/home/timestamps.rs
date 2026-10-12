use chrono::{Offset, TimeZone, Timelike};

pub(super) fn local_clock_time(last: u64, timezone: &impl TimeZone) -> Option<String> {
    let timestamp = i64::try_from(last).ok().filter(|t| *t > 0)?;
    let time = chrono::DateTime::from_timestamp(timestamp, 0)?.with_timezone(timezone);
    let local = time.naive_utc().checked_add_offset(time.offset().fix())?;
    Some(format!("{:02}:{:02}", local.hour(), local.minute()))
}

#[cfg(test)]
mod tests {
    use super::local_clock_time;
    use chrono::{FixedOffset, TimeZone, Utc};

    #[test]
    fn local_clock_uses_the_requested_timezone_and_handles_invalid_timestamps() {
        let timestamp = Utc
            .with_ymd_and_hms(2026, 10, 12, 23, 4, 59)
            .unwrap()
            .timestamp() as u64;
        let east = FixedOffset::east_opt(8 * 3600).unwrap();
        let west = FixedOffset::west_opt(7 * 3600).unwrap();
        assert_eq!(local_clock_time(timestamp, &east).as_deref(), Some("07:04"));
        assert_eq!(local_clock_time(timestamp, &west).as_deref(), Some("16:04"));
        assert!(local_clock_time(0, &east).is_none());
        assert!(local_clock_time(u64::MAX, &east).is_none());
        assert!(local_clock_time(i64::MAX as u64, &east).is_none());
        assert!(
            local_clock_time(chrono::DateTime::<Utc>::MAX_UTC.timestamp() as u64, &east).is_none()
        );
    }
}
