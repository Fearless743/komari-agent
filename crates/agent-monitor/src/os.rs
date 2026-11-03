//! Operating system information.

/// Architecture string using Go's `GOARCH` naming, as uploaded by the Go agent.
pub(crate) fn arch() -> String {
    map_arch(std::env::consts::ARCH)
}

/// Map Rust's target architecture to the value Go reports through
/// `runtime.GOARCH`.
pub(crate) fn map_arch(arch: &str) -> String {
    match arch {
        "x86_64" => "amd64",
        "x86" => "386",
        "aarch64" => "arm64",
        "loongarch64" => "loong64",
        "powerpc" => "ppc",
        "powerpc64" => "ppc64",
        "mips64el" => "mips64le",
        other => other,
    }
    .to_owned()
}
