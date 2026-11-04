//! Pure DXGI adapter model and name formatting.
//!
//! Kept free of Win32 calls so the formatting rules used by Go's
//! `gpu_windows.go` can be tested on every platform. [`super::windows`] fills
//! [`GpuAdapter`] from `IDXGIAdapter1::GetDesc1`.

#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::HashSet;

use crate::gpu::name::format_gpu_name_list;

/// `DXGI_ADAPTER_FLAG_SOFTWARE` from `dxgi.h`.
pub const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 1 << 1;

/// One DXGI adapter, mirroring the fields of Go's `GPUInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuAdapter {
    /// Zero-based enumeration index.
    pub index: u32,
    /// `DXGI_ADAPTER_DESC1::Description`.
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub subsys_id: u32,
    pub revision: u32,
    /// Dedicated video memory in bytes.
    pub dedicated_video_memory: u64,
    /// Dedicated system memory in bytes.
    pub dedicated_system_memory: u64,
    /// Shared system memory in bytes.
    pub shared_system_memory: u64,
    pub luid_high: i32,
    pub luid_low: u32,
    pub flags: u32,
}

impl GpuAdapter {
    /// Dedicated video memory in MiB.
    pub fn dedicated_video_memory_mb(&self) -> u64 {
        self.dedicated_video_memory / (1024 * 1024)
    }

    /// Dedicated system memory in MiB.
    pub fn dedicated_system_memory_mb(&self) -> u64 {
        self.dedicated_system_memory / (1024 * 1024)
    }

    /// Shared system memory in MiB.
    pub fn shared_system_memory_mb(&self) -> u64 {
        self.shared_system_memory / (1024 * 1024)
    }

    /// Whether DXGI marks this as a software (WARP/remote) adapter.
    pub fn is_software(&self) -> bool {
        self.flags & DXGI_ADAPTER_FLAG_SOFTWARE != 0
    }

    /// Local unique identifier as `(high, low)`, used for deduplication.
    pub fn luid(&self) -> (i32, u32) {
        (self.luid_high, self.luid_low)
    }
}

/// Format adapter names like Go's Windows `GpuName`:
///
/// * software adapters are skipped;
/// * adapters sharing a LUID are reported once;
/// * empty names are skipped;
/// * duplicate names collapse to `"<name> × <count>"`;
/// * no usable adapter becomes `"None"`.
pub fn format_adapter_names(adapters: &[GpuAdapter]) -> String {
    let mut seen = HashSet::new();
    let mut names = Vec::new();

    for adapter in adapters {
        if adapter.is_software() {
            continue;
        }
        if !seen.insert(adapter.luid()) {
            continue;
        }
        if !adapter.name.trim().is_empty() {
            names.push(adapter.name.clone());
        }
    }

    format_gpu_name_list(&names)
}

/// Decode a fixed-size, NUL-terminated UTF-16 description.
pub(crate) fn decode_description(raw: &[u16]) -> String {
    let end = raw.iter().position(|&unit| unit == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(index: u32, name: &str, luid_low: u32, flags: u32) -> GpuAdapter {
        GpuAdapter {
            index,
            name: name.to_owned(),
            vendor_id: 0x10DE,
            device_id: 0x2684,
            subsys_id: 0,
            revision: 0xA1,
            dedicated_video_memory: 24 * 1024 * 1024 * 1024,
            dedicated_system_memory: 0,
            shared_system_memory: 16 * 1024 * 1024 * 1024,
            luid_high: 0,
            luid_low,
            flags,
        }
    }

    #[test]
    fn software_adapters_and_duplicate_luids_are_skipped() {
        let adapters = [
            adapter(0, "NVIDIA GeForce RTX 4090", 1, 0),
            adapter(
                1,
                "Microsoft Basic Render Driver",
                2,
                DXGI_ADAPTER_FLAG_SOFTWARE,
            ),
            adapter(2, "NVIDIA GeForce RTX 4090", 1, 0),
            adapter(3, "Intel(R) UHD Graphics 770", 3, 0),
        ];
        assert_eq!(
            format_adapter_names(&adapters),
            "NVIDIA GeForce RTX 4090, Intel(R) UHD Graphics 770"
        );
    }

    #[test]
    fn duplicate_names_collapse_and_empty_input_is_none() {
        let adapters = [
            adapter(0, "AMD Radeon RX 7900 XTX", 1, 0),
            adapter(1, "AMD Radeon RX 7900 XTX", 2, 0),
            adapter(2, "  ", 3, 0),
        ];
        assert_eq!(
            format_adapter_names(&adapters),
            "AMD Radeon RX 7900 XTX × 2"
        );
        assert_eq!(format_adapter_names(&[]), "None");
    }

    #[test]
    fn memory_helpers_report_mebibytes() {
        let gpu = adapter(0, "GPU", 1, 0);
        assert_eq!(gpu.dedicated_video_memory_mb(), 24 * 1024);
        assert_eq!(gpu.dedicated_system_memory_mb(), 0);
        assert_eq!(gpu.shared_system_memory_mb(), 16 * 1024);
    }

    #[test]
    fn description_stops_at_the_first_nul() {
        let mut raw = vec![0u16; 128];
        for (index, unit) in "NVIDIA GeForce RTX 4090".encode_utf16().enumerate() {
            raw[index] = unit;
        }
        assert_eq!(decode_description(&raw), "NVIDIA GeForce RTX 4090");
        assert_eq!(decode_description(&[0x41, 0, 0x42]), "A");
        assert_eq!(decode_description(&[]), "");
    }
}
