/// Return whether `value` is a zero-padded, proleptic Gregorian date.
pub fn is_valid_gregorian_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
    {
        return false;
    }

    let year = u16::from(bytes[0] - b'0') * 1_000
        + u16::from(bytes[1] - b'0') * 100
        + u16::from(bytes[2] - b'0') * 10
        + u16::from(bytes[3] - b'0');
    let month = (bytes[5] - b'0') * 10 + (bytes[6] - b'0');
    let day = (bytes[8] - b'0') * 10 + (bytes[9] - b'0');
    let leap_year =
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year => 29,
        2 => 28,
        _ => return false,
    };
    day != 0 && day <= max_day
}

#[cfg(test)]
mod tests {
    use super::is_valid_gregorian_date;

    #[test]
    fn accepts_gregorian_boundaries() {
        for date in ["0000-01-01", "2024-02-29", "2000-02-29", "2026-04-30"] {
            assert!(is_valid_gregorian_date(date), "{date}");
        }
    }

    #[test]
    fn rejects_impossible_gregorian_dates() {
        for date in [
            "2023-02-29",
            "1900-02-29",
            "2026-04-31",
            "2026-00-10",
            "2026-13-01",
            "2026-02-00",
        ] {
            assert!(!is_valid_gregorian_date(date), "{date}");
        }
    }
}
