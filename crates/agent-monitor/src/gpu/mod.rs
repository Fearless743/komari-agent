//! GPU collection.
//!
//! Corresponds to `monitoring/unit/gpu*.go`:
//!
//! * [`name`] detects the model string for basic info (`lspci` -> sysfs on
//!   Linux, `system_profiler` / `pciconf` elsewhere, DXGI on Windows);
//! * detailed probing prefers `nvidia-smi`, falls back to `rocm-smi`, and
//!   emits Go's two wire shapes (detailed info, or a `models` list only);
//! * a missing tool or a failed probe degrades to no `gpu` object without
//!   affecting other metrics.
//!
//! [`windows`] exposes the raw DXGI adapter list (name, vendor/device IDs and
//! dedicated/shared memory) used by the basic-info name.

pub mod amd;
#[cfg(any(windows, test))]
pub(crate) mod dxgi;
pub mod name;
pub mod nvidia;
#[cfg(windows)]
pub mod windows;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::ffi::OsString;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::process::Command;

use crate::options::MonitorOptions;
use crate::types::{GpuInfo, GpuReport};

/// Collect the `gpu` report section.
///
/// Returns the report and an optional diagnostic message, mirroring Go's
/// `failed to get detailed GPU info: ...` entry.
pub(crate) fn collect_report(options: &MonitorOptions) -> (Option<GpuReport>, Option<String>) {
    if !options.enable_gpu {
        return (None, None);
    }

    let (detailed, models) = probe(options);
    report_from_results(detailed, models)
}

fn report_from_results(
    detailed: Result<Vec<GpuInfo>, String>,
    models: Result<Vec<String>, String>,
) -> (Option<GpuReport>, Option<String>) {
    match detailed {
        Ok(devices) if !devices.is_empty() => (GpuReport::detailed(devices), None),
        Ok(_) => (GpuReport::models(models.unwrap_or_default()), None),
        Err(error) => {
            let report = models.ok().and_then(GpuReport::models);
            (
                report,
                Some(format!("failed to get detailed GPU info: {error}\n")),
            )
        }
    }
}

#[cfg(target_os = "linux")]
fn probe(options: &MonitorOptions) -> (Result<Vec<GpuInfo>, String>, Result<Vec<String>, String>) {
    // Go's vendor probe tries nvidia-smi first and treats AMD as the fallback.
    if let Some(binary) = locate_tool(
        options,
        Some(options.nvidia_smi_path.as_path()),
        "nvidia-smi",
    ) && let Some(output) = run_binary(&binary, &["-q", "-x"])
        && let Ok((devices, models)) = nvidia::parse_xml(&output)
    {
        return (Ok(devices), Ok(models));
    }

    if let Some(binary) = locate_tool(options, Some(options.rocm_smi_path.as_path()), "rocm-smi") {
        if let Some(output) = run_binary(&binary, &["--showallinfo", "--json"]) {
            return match amd::parse_json(&output) {
                Ok((devices, models)) => (Ok(devices), Ok(models)),
                Err(error) => (
                    Err(format!("failed to parse rocm-smi output: {error}")),
                    Err(error),
                ),
            };
        }
        return (
            Err("failed to run rocm-smi".to_owned()),
            Err("failed to run rocm-smi".to_owned()),
        );
    }

    (
        Err("nvidia-smi and rocm-smi tools not found".to_owned()),
        Err("nvidia-smi and rocm-smi tools not found".to_owned()),
    )
}

#[cfg(not(target_os = "linux"))]
fn probe(_options: &MonitorOptions) -> (Result<Vec<GpuInfo>, String>, Result<Vec<String>, String>) {
    let error = "detailed GPU monitoring not supported on this platform".to_owned();
    (Err(error.clone()), Err(error))
}

/// Locate a tool: an explicit path wins, then `gpu_tool_path` (or the process
/// `PATH` when no override is configured).
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
pub(crate) fn locate_tool(
    options: &MonitorOptions,
    explicit: Option<&Path>,
    tool: &str,
) -> Option<PathBuf> {
    if let Some(path) = explicit
        && path.is_file()
    {
        return Some(path.to_path_buf());
    }

    let search_path: Option<OsString> = match &options.gpu_tool_path {
        Some(path) => Some(path.clone()),
        None => std::env::var_os("PATH"),
    };
    let search_path = search_path?;

    for directory in std::env::split_paths(&search_path) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        let candidate = directory.join(tool);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let candidate = directory.join(format!("{tool}.exe"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Run a command and return its output on success.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
pub(crate) fn run_tool(
    options: &MonitorOptions,
    explicit: Option<&Path>,
    tool: &str,
    args: &[&str],
) -> Option<String> {
    let binary = locate_tool(options, explicit, tool)?;
    run_binary(&binary, args)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
fn run_binary(binary: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new(binary).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        Some(String::from_utf8_lossy(&output.stderr).into_owned())
    } else {
        Some(stdout.into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, utilization: f64) -> GpuInfo {
        GpuInfo {
            name: name.to_owned(),
            memory_total: 1024,
            memory_used: 512,
            utilization,
            temperature: 50,
        }
    }

    #[test]
    fn disabled_gpu_switch_produces_no_section() {
        let options = MonitorOptions::default();
        let (report, message) = collect_report(&options);
        assert!(report.is_none());
        assert!(message.is_none());
    }

    #[test]
    fn detailed_shape_computes_average_usage() {
        let (report, message) = report_from_results(
            Ok(vec![device("GPU A", 20.0), device("GPU B", 40.0)]),
            Ok(vec!["GPU A".to_owned()]),
        );
        let report = report.expect("detailed report");
        assert_eq!(report.count, Some(2));
        assert_eq!(report.average_usage, Some(30.0));
        assert_eq!(report.detailed_info.as_ref().unwrap().len(), 2);
        assert!(report.models.is_none());
        assert!(message.is_none());
    }

    #[test]
    fn model_only_fallback_keeps_models_shape() {
        let (report, message) =
            report_from_results(Err("tool failed".to_owned()), Ok(vec!["GPU A".to_owned()]));
        let report = report.expect("models report");
        assert!(report.count.is_none());
        assert!(report.average_usage.is_none());
        assert!(report.detailed_info.is_none());
        assert_eq!(report.models, Some(vec!["GPU A".to_owned()]));
        assert_eq!(
            message.as_deref(),
            Some("failed to get detailed GPU info: tool failed\n")
        );
    }

    #[test]
    fn empty_detailed_result_falls_back_to_models() {
        let (report, _) =
            report_from_results(Ok(Vec::new()), Ok(vec!["GPU A".to_owned(), " ".to_owned()]));
        assert_eq!(report.unwrap().models, Some(vec!["GPU A".to_owned()]));
    }

    #[test]
    fn all_failures_degrade_to_no_gpu_section() {
        let (report, message) =
            report_from_results(Err("no tool".to_owned()), Err("no tool".to_owned()));
        assert!(report.is_none());
        assert!(message.is_some());
    }
}
