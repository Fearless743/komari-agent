//! GitHub release metadata and platform asset selection.
//!
//! The Go agent delegates to `go-github-selfupdate`, which lists releases and
//! selects the asset whose name contains the current `GOOS` and `GOARCH`. The
//! Komari release workflow uploads raw binaries named
//! `komari-agent-{os}-{arch}[.exe]`, so the selection below is a strict
//! whitelist for those canonical names.

use serde::Deserialize;

/// An entry returned by `/repos/{owner}/{repo}/releases`.
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
    pub fn new(os: impl AsRef<str>, arch: impl AsRef<str>) -> Self {
        Self {
            os: normalize_os(os.as_ref()),
            arch: normalize_arch(arch.as_ref()),
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
/// Only the canonical release asset name is accepted:
///
/// * Unix: `komari-agent-{os}-{arch}`;
/// * Windows: `komari-agent-{os}-{arch}.exe`.
///
/// This is a whitelist, not a substring match. Packages (`.deb`, `.rpm`,
/// `.msi`, `.dmg`, `.pkg`, `.apk`, `.AppImage`, `.sh`, `.run`), archives
/// (`.tar.gz`, `.zip`, `.zst`, ...) and checksum/signature files are therefore
/// never installed as the agent binary.
pub fn select_asset<'a>(assets: &'a [Asset], platform: &Platform) -> Option<&'a Asset> {
    let expected = platform.asset_name();
    assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case(&expected))
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
    fn selects_windows_exe_over_extensionless_asset() {
        let assets = vec![
            asset("komari-agent-windows-amd64"),
            asset("komari-agent-windows-amd64.sha256"),
            asset("komari-agent-windows-amd64.exe"),
        ];

        let selected = select_asset(&assets, &Platform::new("windows", "amd64")).unwrap();
        assert_eq!(selected.name, "komari-agent-windows-amd64.exe");
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
    fn rejects_packages_archives_and_non_canonical_names() {
        let assets = vec![
            asset("komari-agent-linux-amd64.deb"),
            asset("komari-agent-linux-amd64.rpm"),
            asset("komari-agent-linux-amd64.zst"),
            asset("komari-agent-linux-amd64.AppImage"),
            asset("komari-agent-linux-amd64.sh"),
            asset("komari-agent-linux-amd64.run"),
            asset("komari-agent-linux-amd64.tar.gz"),
            asset("komari_agent_linux_amd64_bin"),
        ];

        assert!(select_asset(&assets, &Platform::new("linux", "amd64")).is_none());
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
