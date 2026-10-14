//! Agent release families select a contract; payload validation establishes facts.
//! Minor/patch upgrades and binary replacement do not require adapter setup.

pub fn supports_series(version: &str, minimum: (u64, u64, u64)) -> bool {
    let release = if let Some((release, suffix)) = version.split_once(['-', '+']) {
        if suffix.is_empty()
            || !suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
        {
            return false;
        }
        release
    } else {
        version
    };
    let mut parts = release.split('.');
    let mut number = || {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse::<u64>().ok()
    };
    let (Some(major), Some(minor), Some(patch)) = (number(), number(), number()) else {
        return false;
    };
    parts.next().is_none() && major == minimum.0 && (major, minor, patch) >= minimum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_family_accepts_upgrades_but_rejects_old_major_and_malformed_versions() {
        for version in ["1.2.14", "1.3.5", "1.99.42", "1.99.42+build.7"] {
            assert!(supports_series(version, (1, 2, 14)), "{version}");
        }
        for version in ["0.160.1", "0.162.2", "0.200.0"] {
            assert!(supports_series(version, (0, 160, 1)), "{version}");
        }
        for version in [
            "1.2.13",
            "0.99.0",
            "2.0.0",
            "1.3",
            "1.3.5.1",
            "1.x.5",
            "1.3.5-",
            "1.3.5\n",
            "1.3.5+bad suffix",
        ] {
            assert!(!supports_series(version, (1, 2, 14)), "{version}");
        }
        assert!(!supports_series("0.160.0", (0, 160, 1)));
        assert!(!supports_series("1.0.0", (0, 160, 1)));
    }
}
