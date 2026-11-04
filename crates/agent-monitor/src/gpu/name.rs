//! Basic GPU model detection used by the basic-info payload.
//!
//! Linux follows `gpu_linux.go`: `lspci` first, then `/sys/class/drm` (with
//! device-tree SoC names). macOS uses `system_profiler`, FreeBSD `pciconf`,
//! and Windows enumerates DXGI adapters like `gpu_windows.go`.

#![cfg_attr(
    not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        windows
    )),
    allow(dead_code)
)]

use std::collections::BTreeMap;

use crate::options::MonitorOptions;

/// Detect the installed GPU model. Returns `"None"` on Linux when there is no
/// GPU and `"Unknown"` on platforms where the probe failed, matching Go.
pub(crate) fn detect(options: &MonitorOptions) -> String {
    #[cfg(target_os = "linux")]
    {
        let lspci = super::run_tool(options, None, "lspci", &[])
            .as_deref()
            .map(parse_lspci)
            .filter(|name| name != "None");
        if let Some(name) = lspci {
            name
        } else {
            sysfs_name(options)
        }
    }

    #[cfg(target_os = "macos")]
    {
        let _ = options;
        super::run_tool(options, None, "system_profiler", &["SPDisplaysDataType"])
            .as_deref()
            .map(parse_system_profiler)
            .unwrap_or_else(|| "Unknown".to_owned())
    }

    #[cfg(target_os = "freebsd")]
    {
        let _ = options;
        super::run_tool(options, None, "pciconf", &["-lv"])
            .as_deref()
            .map(parse_pciconf)
            .unwrap_or_else(|| "Unknown".to_owned())
    }

    #[cfg(windows)]
    {
        let _ = options;
        super::windows::gpu_name()
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        windows
    )))]
    {
        let _ = options;
        "Unknown".to_owned()
    }
}

/// Format model names like Go's `formatGPUNameList`: duplicates collapse to
/// `"<name> × <count>"`, preserving first-seen order, and an empty list
/// becomes `"None"`.
pub fn format_gpu_name_list(names: &[String]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut order: Vec<&str> = Vec::new();

    for name in names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let count = counts.entry(name).or_insert(0);
        if *count == 0 {
            order.push(name);
        }
        *count += 1;
    }

    if order.is_empty() {
        return "None".to_owned();
    }

    order
        .into_iter()
        .map(|name| match counts[name] {
            1 => name.to_owned(),
            count => format!("{name} × {count}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(target_os = "linux")]
fn parse_lspci(output: &str) -> String {
    const PRIORITY_VENDORS: [&str; 8] = [
        "nvidia",
        "amd",
        "radeon",
        "intel",
        "arc",
        "snap",
        "qualcomm",
        "snapdragon",
    ];
    let mut names = Vec::new();

    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        if !(lower.contains("vga") || lower.contains("3d") || lower.contains("display")) {
            continue;
        }
        if PRIORITY_VENDORS.iter().any(|vendor| lower.contains(vendor))
            && let Some(name) = extract_lspci_name(line)
        {
            append_lspci_name(&mut names, name);
        }
    }

    if names.is_empty() {
        for line in output.lines() {
            let lower = line.to_ascii_lowercase();
            if (lower.contains("vga") || lower.contains("3d") || lower.contains("display"))
                && let Some(name) = extract_lspci_name(line)
            {
                append_lspci_name(&mut names, name);
            }
        }
    }

    format_gpu_name_list(&names)
}

#[cfg(target_os = "linux")]
fn extract_lspci_name(line: &str) -> Option<String> {
    let (_, name) = line.rsplit_once(':')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let name = match name.rfind('(') {
        Some(index) => name[..index].trim(),
        None => name,
    };
    if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    }
}

#[cfg(target_os = "linux")]
fn append_lspci_name(names: &mut Vec<String>, name: String) {
    let lower = name.to_ascii_lowercase();
    let excluded = lower.starts_with("1111")
        || (lower.starts_with("cirrus logic") && lower.contains("gd 5"))
        || lower.contains("virtio")
        || lower.contains("vmware")
        || lower.contains("qxl")
        || lower.contains("hyper-v");
    if !excluded {
        names.push(name);
    }
}

#[cfg(target_os = "linux")]
fn sysfs_name(options: &MonitorOptions) -> String {
    const EXCLUDED_DRIVERS: [&str; 11] = [
        "virtio-pci",
        "virtio_gpu",
        "bochs-drm",
        "qxl",
        "vmwgfx",
        "cirrus",
        "vboxvideo",
        "hyperv_fb",
        "simpledrm",
        "simplefb",
        "cirrus-qemu",
    ];

    let drm = options.sys_path("class/drm");
    let mut names = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&drm) {
        for entry in entries.filter_map(Result::ok) {
            let file_name = entry.file_name();
            let Some(base) = file_name.to_str() else {
                continue;
            };
            if !is_drm_card_name(base) {
                continue;
            }

            let path = entry.path();
            let Ok(link) = std::fs::read_link(path.join("device/driver")) else {
                continue;
            };
            let Some(driver) = link.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if EXCLUDED_DRIVERS.contains(&driver) {
                continue;
            }

            let exact_model = std::fs::read(path.join("device/of_node/compatible"))
                .ok()
                .and_then(|raw| parse_soc_model(driver, &raw));
            if let Some(model) = exact_model {
                append_sysfs_name(&mut names, model);
                continue;
            }

            let mapped = match driver {
                "vc4" | "vc4-drm" => Some("Broadcom VideoCore IV/VI (Raspberry Pi)"),
                "v3d" | "v3d-drm" => Some("Broadcom V3D (Raspberry Pi 4/5)"),
                "msm" | "msm_drm" => Some("Qualcomm Adreno (Unknown Model)"),
                "panfrost" => Some("ARM Mali (Panfrost)"),
                "lima" => Some("ARM Mali (Lima)"),
                "sun4i-drm" | "sunxi-drm" => Some("Allwinner Display Engine"),
                "tegra" => Some("NVIDIA Tegra"),
                "ast" => Some("ASPEED Technology, Inc. ASPEED Graphics Family"),
                "i915" | "i915-drm" => Some("Intel Integrated Graphics"),
                "mgag200" => Some("Matrox G200 Series"),
                _ => None,
            };
            if let Some(name) = mapped {
                append_sysfs_name(&mut names, name.to_owned());
            } else if !driver.is_empty() {
                append_sysfs_name(&mut names, format!("Direct Render Manager {driver}"));
            }
        }
    }

    if !names.is_empty() {
        return format_gpu_name_list(&names);
    }

    if let Ok(model) = std::fs::read_to_string(options.sys_path("firmware/devicetree/base/model"))
        && model.contains("Raspberry Pi")
    {
        return "Broadcom VideoCore (Integrated)".to_owned();
    }

    "None".to_owned()
}

#[cfg(target_os = "linux")]
fn is_drm_card_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix("card") else {
        return false;
    };
    !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(target_os = "linux")]
fn append_sysfs_name(names: &mut Vec<String>, name: String) {
    let lower = name.to_ascii_lowercase();
    let excluded = lower.contains("virtio")
        || lower.contains("vmware")
        || lower.contains("qxl")
        || lower.contains("hyper-v")
        || lower.contains("cirrus");
    if !excluded && !name.trim().is_empty() {
        names.push(name);
    }
}

#[cfg(target_os = "linux")]
fn parse_soc_model(driver: &str, raw: &[u8]) -> Option<String> {
    let content = String::from_utf8_lossy(raw).replace('\0', " ");
    let lower = content.to_ascii_lowercase();

    if driver == "msm" || lower.contains("adreno") {
        if let Some(model) =
            token_after(&lower, "adreno-").or_else(|| token_after(&lower, "adreno_"))
        {
            let digits: String = model.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty() {
                return Some(format!("Qualcomm Adreno {digits}"));
            }
        }
        return Some("Qualcomm Adreno".to_owned());
    }

    if driver == "panfrost" || driver == "lima" || lower.contains("mali") {
        if let Some(model) = token_after(&lower, "mali-").or_else(|| token_after(&lower, "mali_")) {
            // Go's regex is `mali[-_]([a-z]\d+)`: a letter followed by digits.
            // "mali-450" therefore falls back to the generic name.
            let mut chars = model.chars();
            if let Some(first) = chars.next()
                && first.is_ascii_alphabetic()
            {
                let digits: String = chars.take_while(char::is_ascii_digit).collect();
                if !digits.is_empty() {
                    return Some(format!("ARM Mali {}{digits}", first.to_ascii_uppercase()));
                }
            }
        }
        return Some("ARM Mali".to_owned());
    }

    if matches!(driver, "vc4" | "vc4-drm" | "v3d") {
        if lower.contains("bcm2712") {
            return Some("Broadcom VideoCore VII (Pi 5)".to_owned());
        }
        if lower.contains("bcm2711") {
            return Some("Broadcom VideoCore VI (Pi 4)".to_owned());
        }
        if lower.contains("bcm2837") || lower.contains("bcm2835") {
            return Some("Broadcom VideoCore IV".to_owned());
        }
    }

    if lower.contains("allwinner") || lower.contains("sun50i") || lower.contains("sun8i") {
        if let Some(model) =
            token_after(&lower, "sun50i-").or_else(|| token_after(&lower, "sun8i-"))
        {
            let model: String = model
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric())
                .collect();
            if !model.is_empty() {
                return Some(format!("Allwinner {}", model.to_ascii_uppercase()));
            }
        }
        return Some("Allwinner Display Engine".to_owned());
    }

    if driver == "tegra" {
        if lower.contains("tegra194") {
            return Some("NVIDIA Tegra Xavier".to_owned());
        }
        if lower.contains("tegra234") {
            return Some("NVIDIA Orin".to_owned());
        }
        if lower.contains("tegra210") {
            return Some("NVIDIA Tegra X1".to_owned());
        }
    }

    None
}

#[cfg(target_os = "linux")]
fn token_after<'a>(haystack: &'a str, needle: &str) -> Option<&'a str> {
    haystack
        .find(needle)
        .map(|index| &haystack[index + needle.len()..])
}

#[cfg(target_os = "macos")]
fn parse_system_profiler(output: &str) -> String {
    let names: Vec<String> = output
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("Chipset Model:"))
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .collect();
    format_gpu_name_list(&names)
}

#[cfg(target_os = "freebsd")]
fn parse_pciconf(output: &str) -> String {
    let names: Vec<String> = output
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("VGA") || line.contains("Display"))
        .map(str::to_owned)
        .collect();
    format_gpu_name_list(&names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_duplicates_and_empty_list() {
        assert_eq!(
            format_gpu_name_list(&[
                "NVIDIA GeForce RTX 4090".to_owned(),
                "NVIDIA GeForce RTX 4090".to_owned(),
                "AMD Radeon RX 7900 XTX".to_owned(),
            ]),
            "NVIDIA GeForce RTX 4090 × 2, AMD Radeon RX 7900 XTX"
        );
        assert_eq!(format_gpu_name_list(&[]), "None");
        assert_eq!(
            format_gpu_name_list(&["  ".to_owned(), "Intel Arc A770".to_owned()]),
            "Intel Arc A770"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn lspci_parser_prioritises_real_gpus_and_drops_virtual_ones() {
        let output = "\
00:00.0 Host bridge: Intel Corporation Device 1234
00:01.0 VGA compatible controller: VMware SVGA II Adapter
00:02.0 VGA compatible controller: Intel Corporation AlderLake-S GT1 (rev 0c)
01:00.0 3D controller: NVIDIA Corporation GA102 [GeForce RTX 3090] (rev a1)
02:00.0 VGA compatible controller: Cirrus Logic GD 5446
";
        assert_eq!(
            parse_lspci(output),
            "Intel Corporation AlderLake-S GT1, NVIDIA Corporation GA102 [GeForce RTX 3090]"
        );
        assert_eq!(parse_lspci("00:00.0 Host bridge: Intel\n"), "None");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn soc_compatible_strings_map_to_friendly_names() {
        assert_eq!(
            parse_soc_model("msm", b"qcom,adreno-750.1\0qcom,adreno").as_deref(),
            Some("Qualcomm Adreno 750")
        );
        assert_eq!(
            parse_soc_model("panfrost", b"rockchip,rk3588-mali\0arm,mali-g610").as_deref(),
            Some("ARM Mali G610")
        );
        assert_eq!(
            parse_soc_model("panfrost", b"arm,mali-450").as_deref(),
            Some("ARM Mali")
        );
        assert_eq!(
            parse_soc_model("vc4-drm", b"brcm,bcm2711-vc5").as_deref(),
            Some("Broadcom VideoCore VI (Pi 4)")
        );
        assert_eq!(parse_soc_model("i915", b""), None);
    }
}
