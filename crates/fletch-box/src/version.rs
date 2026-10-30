//! Ordering release versions, of images and of the CLIs on tools disks.

/// A version as numbers to order by: `v2.1.312` → `[2, 1, 312]`, so `0.10.0`
/// sorts after `0.9.0`. A leading `v` is ignored, and a part counts by its
/// leading digits, 0 when it has none (`latest` → `[0]`).
pub fn version_key(version: &str) -> Vec<u64> {
    version
        .trim_start_matches('v')
        .split('.')
        .map(|part| {
            let end = part
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(part.len());
            part[..end].parse().unwrap_or(0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::version_key;

    #[test]
    fn versions_order_as_numbers() {
        assert!(version_key("v2.1.314") > version_key("v2.1.312"));
        assert!(version_key("2.1.312") > version_key("2.1.9"));
        assert!(version_key("v2.10.0") > version_key("v2.9.99"));
        assert!(version_key("0.10.0") > version_key("0.9.0"));
        assert!(version_key("v0.0.1") > version_key("latest"));
        assert_eq!(version_key("v1.2.3"), version_key("1.2.3"));
    }
}
