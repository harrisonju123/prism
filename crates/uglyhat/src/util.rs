use chrono::Duration;

/// Parse a human-readable duration string like "30m", "2h", "1d".
/// Returns a chrono Duration. Supports m (minutes), h (hours), d (days).
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".to_string());
    }
    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: i64 = num_str
        .parse()
        .map_err(|_| format!("invalid duration number: {num_str:?}"))?;
    match unit {
        "m" => Ok(Duration::minutes(num)),
        "h" => Ok(Duration::hours(num)),
        "d" => Ok(Duration::days(num)),
        _ => Err(format!("unknown duration unit: {unit:?} (use m/h/d)")),
    }
}
