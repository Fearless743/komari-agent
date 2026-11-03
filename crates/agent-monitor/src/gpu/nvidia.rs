#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
//! `nvidia-smi -q -x` parsing.
//!
//! Field extraction and unit conversions mirror `gpu_nvidia_smi.go`:
//! utilization is a percentage, framebuffer memory is reported in MiB and
//! converted to bytes, and temperature is an integer Celsius value.

use crate::types::GpuInfo;

/// Parse `nvidia-smi -q -x` XML into device details and model names.
pub fn parse_xml(xml: &str) -> Result<(Vec<GpuInfo>, Vec<String>), String> {
    let document = roxmltree::Document::parse(xml).map_err(|error| error.to_string())?;

    let mut devices = Vec::new();
    let mut models = Vec::new();

    for gpu in document
        .descendants()
        .filter(|node| node.is_element() && node.has_tag_name("gpu"))
    {
        let name = child_text(gpu, "product_name").unwrap_or_default();
        let utilization = descendant_text(gpu, "gpu_util")
            .and_then(parse_percentage)
            .unwrap_or(0.0);
        let memory_total = gpu
            .descendants()
            .find(|node| node.is_element() && node.has_tag_name("fb_memory_usage"))
            .and_then(|node| child_text(node, "total"))
            .map(parse_memory)
            .unwrap_or(0);
        let memory_used = gpu
            .descendants()
            .find(|node| node.is_element() && node.has_tag_name("fb_memory_usage"))
            .and_then(|node| child_text(node, "used"))
            .map(parse_memory)
            .unwrap_or(0);
        let temperature = descendant_text(gpu, "gpu_temp")
            .and_then(parse_temperature)
            .unwrap_or(0);

        if !name.trim().is_empty() {
            models.push(name.to_owned());
        }
        devices.push(GpuInfo {
            name: name.to_owned(),
            memory_total,
            memory_used,
            utilization,
            temperature,
        });
    }

    Ok((devices, models))
}

fn descendant_text<'a, 'input>(node: roxmltree::Node<'a, 'input>, tag: &str) -> Option<&'a str>
where
    'input: 'a,
{
    node.descendants()
        .find(|child| child.is_element() && child.has_tag_name(tag))
        .and_then(|child| child.text())
}

fn child_text<'a, 'input>(node: roxmltree::Node<'a, 'input>, tag: &str) -> Option<&'a str>
where
    'input: 'a,
{
    node.children()
        .find(|child| child.is_element() && child.has_tag_name(tag))
        .and_then(|child| child.text())
}

/// Parse `"25 %"` into `25.0`.
pub fn parse_percentage(value: &str) -> Option<f64> {
    let cleaned = value.trim().trim_end_matches('%').trim();
    if cleaned.is_empty() {
        return Some(0.0);
    }
    cleaned.parse().ok()
}

/// Parse `"1024 MiB"` into bytes.
pub fn parse_memory(value: &str) -> u64 {
    let cleaned = value.trim().trim_end_matches("MiB").trim();
    if cleaned.is_empty() {
        return 0;
    }
    cleaned
        .parse::<u64>()
        .ok()
        .and_then(|mib| mib.checked_mul(1024 * 1024))
        .unwrap_or(0)
}

/// Parse `"65 C"` into Celsius.
pub fn parse_temperature(value: &str) -> Option<u64> {
    let cleaned = value.trim().trim_end_matches('C').trim();
    if cleaned.is_empty() {
        return Some(0);
    }
    cleaned.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMI_XML: &str = r#"<?xml version="1.0" ?>
<nvidia_smi_log>
  <gpu id="00000000:01:00.0">
    <product_name>NVIDIA GeForce RTX 4090</product_name>
    <fb_memory_usage>
      <total>24564 MiB</total>
      <used>1024 MiB</used>
      <free>23540 MiB</free>
    </fb_memory_usage>
    <utilization>
      <gpu_util>37 %</gpu_util>
    </utilization>
    <temperature>
      <gpu_temp>65 C</gpu_temp>
    </temperature>
  </gpu>
  <gpu id="00000000:02:00.0">
    <product_name>NVIDIA GeForce RTX 4090</product_name>
    <fb_memory_usage>
      <total>24564 MiB</total>
      <used>2048 MiB</used>
    </fb_memory_usage>
    <utilization>
      <gpu_util>N/A</gpu_util>
    </utilization>
    <temperature>
      <gpu_temp>N/A</gpu_temp>
    </temperature>
  </gpu>
</nvidia_smi_log>
"#;

    #[test]
    fn parses_detailed_fields_and_models() {
        let (devices, models) = parse_xml(SMI_XML).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(models, vec!["NVIDIA GeForce RTX 4090"; 2]);
        assert_eq!(devices[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(devices[0].memory_total, 24564 * 1024 * 1024);
        assert_eq!(devices[0].memory_used, 1024 * 1024 * 1024);
        assert_eq!(devices[0].utilization, 37.0);
        assert_eq!(devices[0].temperature, 65);
        assert_eq!(devices[1].utilization, 0.0);
        assert_eq!(devices[1].temperature, 0);
    }

    #[test]
    fn value_parsers_tolerate_na_and_empty_values() {
        assert_eq!(parse_percentage(""), Some(0.0));
        assert_eq!(parse_percentage("N/A"), None);
        assert_eq!(parse_memory("N/A"), 0);
        assert_eq!(parse_memory(""), 0);
        assert_eq!(parse_temperature(""), Some(0));
        assert_eq!(parse_temperature("N/A"), None);
    }

    #[test]
    fn malformed_xml_is_an_error() {
        assert!(parse_xml("<nvidia_smi_log><gpu>").is_err());
    }
}
