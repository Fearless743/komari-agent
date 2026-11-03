//! Version parsing and comparison.
//!
//! Mirrors `parseVersion` in `komari-agent/update/update.go`: a single leading
//! `v`/`V` is stripped and the remainder is parsed tolerantly (missing minor
//! or patch components default to zero). Pre-release and build metadata follow
//! standard SemVer precedence.

use semver::Version;
use thiserror::Error;

/// Parse a version string with an optional `v` prefix.
///
/// Tolerant forms such as `1`, `1.2` and `v1.2.3-beta.1` are accepted. Build
/// metadata is preserved but, as required by SemVer, does not affect ordering.
pub fn parse_version(input: &str) -> Result<Version, VersionError> {
    let mut value = input.trim();
    if let Some(stripped) = value.strip_prefix('v') {
        value = stripped;
    }
    if let Some(stripped) = value.strip_prefix('V') {
        value = stripped;
    }
    let value = value.trim();

    if value.is_empty() {
        return Err(VersionError::Invalid(input.to_owned()));
    }

    // Split off pre-release/build metadata before padding the core so that
    // "1.2-beta" becomes "1.2.0-beta".
    let suffix_start = value.find(['-', '+']).unwrap_or(value.len());
    let (core, suffix) = value.split_at(suffix_start);

    let mut components: Vec<&str> = core.split('.').collect();
    if components.iter().any(|component| component.is_empty()) {
        return Err(VersionError::Invalid(input.to_owned()));
    }
    // Go's ParseTolerant rejects a short version that already carries
    // pre-release/build metadata ("1.2-beta"), while padding plain "1.2".
    if !suffix.is_empty() && components.len() < 3 {
        return Err(VersionError::Invalid(input.to_owned()));
    }
    while components.len() < 3 {
        components.push("0");
    }

    let normalized = format!("{}{}", components.join("."), suffix);

    Version::parse(&normalized).map_err(|source| VersionError::Semver {
        value: input.to_owned(),
        source,
    })
}

/// Parse a GitHub release tag using the same rule as
/// `go-github-selfupdate`'s `\d+\.\d+\.\d+` extraction.
///
/// The first three-component numeric version is taken from the tag, so arbitrary
/// prefixes such as `release-1.2.3` are accepted. Unlike [`parse_version`],
/// a two-component tag (`1.2`) is rejected, matching Go's release detection.
pub fn parse_release_version(input: &str) -> Result<Version, VersionError> {
    let bytes = input.as_bytes();

    for start in 0..bytes.len() {
        if !bytes[start].is_ascii_digit() {
            continue;
        }

        let mut position = start;
        let mut groups = 0;

        loop {
            let digits_start = position;
            while position < bytes.len() && bytes[position].is_ascii_digit() {
                position += 1;
            }
            if position == digits_start {
                break;
            }
            groups += 1;

            if groups == 3 {
                let candidate = &input[start..];
                return Version::parse(candidate).map_err(|source| VersionError::Semver {
                    value: candidate.to_owned(),
                    source,
                });
            }

            if position < bytes.len() && bytes[position] == b'.' {
                position += 1;
            } else {
                break;
            }
        }
    }

    Err(VersionError::Invalid(input.to_owned()))
}

/// Whether `latest` is newer than `current`.
///
/// Build metadata does not participate in SemVer precedence, so
/// `0.0.6` and `0.0.6+build.1` compare as equal.
pub fn need_update(current: &Version, latest: &Version) -> bool {
    latest.cmp_precedence(current).is_gt()
}

/// Version parsing error.
#[derive(Debug, Error)]
pub enum VersionError {
    #[error("invalid version {0:?}")]
    Invalid(String),

    #[error("failed to parse version {value:?}: {source}")]
    Semver {
        value: String,
        #[source]
        source: semver::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_like_go() {
        let cases = [
            ("v1.2.3", "1.2.3"),
            ("V1.2.3", "1.2.3"),
            ("vV1.2.3", "1.2.3"),
            ("1.2.3-beta.1", "1.2.3-beta.1"),
            ("v1.2.3+meta", "1.2.3+meta"),
            ("1.2.3-pre.1+build.123", "1.2.3-pre.1+build.123"),
            ("  v2.0.0  ", "2.0.0"),
            ("1", "1.0.0"),
            ("1.2", "1.2.0"),
        ];

        for (input, expected) in cases {
            let parsed = parse_version(input).unwrap();
            assert_eq!(parsed.to_string(), expected, "{input}");
        }

        for input in ["", "invalid", "1.2.3.4", "v", "01.2.3", "1.2-beta"] {
            assert!(parse_version(input).is_err(), "{input} should fail");
        }
    }

    #[test]
    fn release_tags_follow_go_regex_extraction() {
        let cases = [
            ("v1.2.3", "1.2.3"),
            ("release-1.2.3", "1.2.3"),
            ("foo/bar1.2.3", "1.2.3"),
            ("release-1.2.3-beta.1", "1.2.3-beta.1"),
            ("1.2.3+build.5", "1.2.3+build.5"),
        ];

        for (input, expected) in cases {
            assert_eq!(
                parse_release_version(input).unwrap().to_string(),
                expected,
                "{input}"
            );
        }

        for input in ["1.2", "1", "1.2.3.4", "release-1.2", "v", "invalid"] {
            assert!(
                parse_release_version(input).is_err(),
                "{input} should be rejected"
            );
        }
    }

    #[test]
    fn need_update_covers_release_and_prerelease_rules() {
        let cases = [
            ("1.0.0", "1.0.1", true),
            ("v1.0.0", "1.1.0", true),
            ("1.2.3", "1.2.3", false),
            ("1.2.4", "1.2.3", false),
            ("1.2.3-beta", "1.2.3", true),
            ("1.2.3", "1.2.3-beta", false),
            ("1.2.3-alpha", "1.2.3-beta", true),
            ("0.0.5", "0.0.6+build.1", true),
            ("0.0.6", "v0.0.6+build.1", false),
            ("2.0.0", "2.0.0-rc.1", false),
        ];

        for (current, latest, expected) in cases {
            let current = parse_version(current).unwrap();
            let latest = parse_version(latest).unwrap();
            assert_eq!(
                need_update(&current, &latest),
                expected,
                "{current} -> {latest}"
            );
        }
    }
}
