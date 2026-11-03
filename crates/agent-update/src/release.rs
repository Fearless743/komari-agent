//! GitHub release metadata and platform asset selection.
//!
//! The Go agent delegates to `go-github-selfupdate`, which selects the asset
//! whose file name contains the current `GOOS` and `GOARCH`. The Komari release
//! workflow uploads raw binaries named `komari-agent-{os}-{arch}[.exe]`, so the
//! selection below matches that naming first and falls back to token matching.

use serde::Deserialize;

/// A GitHub release as returned by `/repos/{owner}/{repo}/releases/latest`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct GithubRelease {
    #[serde(default)]
    pub tag_name: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// A downloadable release asset.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
    /// GitHub's optional `digest` field, for example `sha256:ab12...`.
    #[serde(default)]
    pub digest: Option<String>,
}

/// Target platform using Go's `GOOS`/`GOARCH` naming.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}

impl Platform {
    pub fn new(os: impl Into<String>, arch: impl Into<String>) -> Self {
        Self {
            os: os.into(),
            arch: arch.into(),
        }
    }

    /// Platform of the running binary, mapped to Go names.
    pub fn current() -> Self {
        Self::new(
            normalize_os(std::env::consts::OS),
            normalize_arch(std::env::consts::ARCH),
        )
    }

    /// Canonical Komari asset name, including `.exe` on Windows.
    pub fn asset_name(&self) -> String {
        if self.os == "windows" {
            format!("komari-agent-{}-{}.exe", self.os, self.arch)
        } else {
            format!("komari-agent-{}-{}", self.os, self.arch)
        }
    }
}

/// Select the asset for `platform`.
///
/// Preference order:
///
/// 1. exact `komari-agent-{os}-{arch}[.exe]` (and the short `{os}-{arch}`);
/// 2. any `komari-agent-*` file whose name contains both platform tokens;
/// 3. any other file containing both platform tokens.
///
/// Checksum, signature and archive assets are ignored: the updater replaces
/// the running binary directly and never extracts archives.
pub fn select_asset<'a>(assets: &'a [Asset], platform: &Platform) -> Option<&'a Asset> {
    let os = platform.os.to_ascii_lowercase();
    let arch = platform.arch.to_ascii_lowercase();

    let exact = [
        format!("komari-agent-{os}-{arch}"),
        format!("komari-agent-{os}-{arch}.exe"),
        format!("{os}-{arch}"),
        format!("{os}-{arch}.exe"),
    ];

    let mut best: Option<(u8, &Asset)> = None;

    for asset in assets {
        let name = asset.name.to_ascii_lowercase();

        if is_ignored_asset(&name) {
            continue;
        }

        // Never install a Windows executable on Unix, or the other way around.
        if os != "windows" && name.ends_with(".exe") {
            continue;
        }

        let tokens: Vec<&str> = name
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .collect();

        if !tokens.contains(&os.as_str()) || !tokens.contains(&arch.as_str()) {
            continue;
        }

        let score = if exact.iter().any(|candidate| candidate == &name) {
            0
        } else if name.starts_with("komari-agent") {
            1
        } else {
            2
        };

        if best.is_none_or(|(best_score, _)| score < best_score) {
            best = Some((score, asset));
        }
    }

    best.map(|(_, asset)| asset)
}

fn is_ignored_asset(name: &str) -> bool {
    const IGNORED_SUFFIXES: &[&str] = &[
        ".sha256",
        ".sha256sum",
        ".sha512",
        ".sha512sum",
        ".sig",
        ".asc",
        ".txt",
        ".json",
        ".tar.gz",
        ".tgz",
        ".tar",
        ".zip",
        ".gz",
        ".xz",
        ".bz2",
        ".7z",
    ];

    IGNORED_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

fn normalize_os(os: &str) -> String {
    match os {
        "macos" => "darwin".to_owned(),
        other => other.to_owned(),
    }
}

fn normalize_arch(arch: &str) -> String {
    match arch {
        "x86_64" | "amd64" => "amd64".to_owned(),
        "aarch64" | "arm64" => "arm64".to_owned(),
        "x86" | "i386" | "i586" | "i686" => "386".to_owned(),
        "arm" | "armv7" => "arm".to_owned(),
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_owned(),
            browser_download_url: format!("https://example.com/{name}"),
            size: 1024,
            digest: None,
        }
    }

    #[test]
    fn selects_exact_linux_asset() {
        let assets = vec![
            asset("komari-agent-linux-arm64"),
            asset("komari-agent-linux-amd64"),
            asset("komari-agent-darwin-amd64"),
        ];

        let selected = select_asset(&assets, &Platform::new("linux", "amd64")).unwrap();
        assert_eq!(selected.name, "komari-agent-linux-amd64");
    }

    #[test]
    fn selects_windows_exe_and_ignores_unix_asset_with_same_tokens() {
        let assets = vec![
            asset("komari-agent-windows-amd64"),
            asset("notes-windows-amd64.txt"),
            asset("komari-agent-windows-amd64.sha256"),
        ];

        let selected = select_asset(&assets, &Platform::new("windows", "amd64")).unwrap();
        assert_eq!(selected.name, "komari-agent-windows-amd64");
    }

    #[test]
    fn rejects_checksums_archives_and_wrong_platforms() {
        let assets = vec![
            asset("komari-agent-linux-amd64.sha256"),
            asset("komari-agent-linux-amd64.tar.gz"),
            asset("komari-agent-windows-amd64.exe"),
            asset("komari-agent-linux-arm64"),
        ];

        assert!(select_asset(&assets, &Platform::new("linux", "amd64")).is_none());
    }

    #[test]
    fn falls_back_to_token_matching() {
        let assets = vec![asset("komari_agent_linux_amd64_bin")];

        let selected = select_asset(&assets, &Platform::new("linux", "amd64")).unwrap();
        assert_eq!(selected.name, "komari_agent_linux_amd64_bin");
    }

    #[test]
    fn platform_names_match_release_workflow() {
        assert_eq!(normalize_os("macos"), "darwin");
        assert_eq!(normalize_arch("x86_64"), "amd64");
        assert_eq!(normalize_arch("aarch64"), "arm64");
        assert_eq!(normalize_arch("i686"), "386");
        assert_eq!(
            Platform::new("windows", "amd64").asset_name(),
            "komari-agent-windows-amd64.exe"
        );
        assert_eq!(
            Platform::new("linux", "arm64").asset_name(),
            "komari-agent-linux-arm64"
        );
    }
}
