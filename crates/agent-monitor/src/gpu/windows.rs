//! DXGI adapter enumeration (Windows only).
//!
//! Mirrors `monitoring/unit/gpu_windows.go`: `CreateDXGIFactory1` followed by
//! `IDXGIFactory1::EnumAdapters1` until `DXGI_ERROR_NOT_FOUND`, reading each
//! adapter with `IDXGIAdapter1::GetDesc1`.
//!
//! Go uses the same enumeration for `GpuName` (software adapters filtered,
//! deduplicated by LUID). The adapter description also carries dedicated and
//! shared memory totals, which are exposed through [`enumerate_gpu_adapters`]
//! for callers that need them; the v1 report protocol has no GPU memory field,
//! so they are not uploaded.

use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ERROR_NOT_FOUND, IDXGIFactory1,
};

use super::dxgi::{GpuAdapter, decode_description, format_adapter_names};

/// Enumerate DXGI adapters in enumeration order.
///
/// Returns an error when DXGI itself cannot be loaded or an adapter
/// description cannot be read; an empty list means "no adapters".
pub fn enumerate_gpu_adapters() -> Result<Vec<GpuAdapter>, String> {
    // SAFETY: the factory and adapters are COM objects owned by this function
    // and released when their wrappers are dropped. `index` is incremented
    // until DXGI reports DXGI_ERROR_NOT_FOUND.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|error| {
            format!(
                "CreateDXGIFactory1 failed with HRESULT: 0x{:08X}",
                error.code().0 as u32
            )
        })?;

        let mut adapters = Vec::new();
        let mut index = 0u32;

        loop {
            match factory.EnumAdapters1(index) {
                Ok(adapter) => {
                    let desc = adapter.GetDesc1().map_err(|error| {
                        format!(
                            "GetDesc1 failed at index {index} with HRESULT: 0x{:08X}",
                            error.code().0 as u32
                        )
                    })?;
                    adapters.push(adapter_from_desc(index, &desc));
                }
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(error) => {
                    return Err(format!(
                        "EnumAdapters1 failed at index {index} with HRESULT: 0x{:08X}",
                        error.code().0 as u32
                    ));
                }
            }
            index = index.saturating_add(1);
        }

        Ok(adapters)
    }
}

/// Basic-info GPU name, matching Go's Windows `GpuName`.
pub(crate) fn gpu_name() -> String {
    match enumerate_gpu_adapters() {
        Ok(adapters) => format_adapter_names(&adapters),
        Err(error) => {
            tracing::debug!(%error, "DXGI GPU enumeration failed");
            "Unknown".to_owned()
        }
    }
}

fn adapter_from_desc(index: u32, desc: &DXGI_ADAPTER_DESC1) -> GpuAdapter {
    GpuAdapter {
        index,
        name: decode_description(&desc.Description),
        vendor_id: desc.VendorId,
        device_id: desc.DeviceId,
        subsys_id: desc.SubSysId,
        revision: desc.Revision,
        dedicated_video_memory: desc.DedicatedVideoMemory as u64,
        dedicated_system_memory: desc.DedicatedSystemMemory as u64,
        shared_system_memory: desc.SharedSystemMemory as u64,
        luid_high: desc.AdapterLuid.HighPart,
        luid_low: desc.AdapterLuid.LowPart,
        flags: desc.Flags,
    }
}
