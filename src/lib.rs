//! Disk usage scanning and display.

pub mod browse;
pub mod render;
pub mod scan;
pub mod tree;

/// Parses sizes such as `500`, `64k`, `10M`, `2G`. Units are 1024-based.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let bad = || format!("invalid size '{s}', expected like 500, 64k, 10M or 2G");
    let digits = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(digits);
    let n: u64 = num.parse().map_err(|_| bad())?;
    let mult: u64 = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return Err(bad()),
    };
    n.checked_mul(mult).ok_or_else(bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("500"), Ok(500));
        assert_eq!(parse_size("64k"), Ok(65536));
        assert_eq!(parse_size("10MiB"), Ok(10 << 20));
        assert_eq!(parse_size("2G"), Ok(2 << 30));
        for bad in ["", "k", "-1", "1x", "99999999999999999999", "99999999T"] {
            assert!(parse_size(bad).is_err(), "{bad}");
        }
    }
}
