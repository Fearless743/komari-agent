#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
//! `rocm-smi --showallinfo --json` parsing.
//!
//! Mirrors `gpu_amd_rocm_smi.go`: cards are top-level keys starting with
//! `card`, and all metrics are JSON strings.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::types::GpuInfo;

/// Parse ROCm JSON into device details and model names.
pub fn parse_json(json: &str) -> Result<(Vec<GpuInfo>, Vec<String>), String> {
    let value: Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let Some(cards) = value.as_object() else {
        return Err("ROCm response is not a JSON object".to_owned());
    };

    // Sort keys so repeated runs are deterministic; Go iterates a map, so the
    // order is unspecified there anyway.
    let cards: BTreeMap<&String, &Value> = cards.iter().collect();

    let mut devices = Vec::new();
    let mut models = Vec::new();
    for (key, value) in cards {
        if !key.starts_with("card") {
            continue;
        }
        let Some(card) = value.as_object() else {
            continue;
        };

        let name = string_field(card, "Card series").unwrap_or_default();
        let utilization = string_field(card, "GPU use (%)")
            .and_then(parse_percentage)
            .unwrap_or(0.0);
        let memory_total = string_field(card, "VRAM Total Memory (B)")
            .and_then(parse_u64)
            .unwrap_or(0);
        let memory_used = string_field(card, "VRAM Total Used Memory (B)")
            .and_then(parse_u64)
            .unwrap_or(0);
        let temperature = string_field(card, "Temperature (Sensor junction) (C)")
            .and_then(parse_u64)
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

fn string_field<'a>(card: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    card.get(key).and_then(Value::as_str)
}

/// Parse `"25"` or `"25%"` into `25.0`.
pub fn parse_percentage(value: &str) -> Option<f64> {
    let cleaned = value.trim().trim_end_matches('%').trim();
    if cleaned.is_empty() {
        return Some(0.0);
    }
    cleaned.parse().ok()
}

fn parse_u64(value: &str) -> Option<u64> {
    let cleaned = value.trim();
    if cleaned.is_empty() {
        return Some(0);
    }
    cleaned.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROCM_JSON: &str = r#"{
      "card0": {
        "Card series": "AMD Radeon RX 7900 XTX",
        "GPU use (%)": "42",
        "VRAM Total Memory (B)": "25753026560",
        "VRAM Total Used Memory (B)": "1073741824",
        "Temperature (Sensor junction) (C)": "61"
      },
      "card1": {
        "Card series": "AMD Radeon RX 7900 XTX",
        "GPU use (%)": "0",
        "VRAM Total Memory (B)": "25753026560",
        "VRAM Total Used Memory (B)": "0"
      },
      "system": {"Driver version": "6.8.0"}
    }"#;

    #[test]
    fn parses_cards_and_skips_non_card_keys() {
        let (devices, models) = parse_json(ROCM_JSON).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(models.len(), 2);
        assert_eq!(devices[0].name, "AMD Radeon RX 7900 XTX");
        assert_eq!(devices[0].utilization, 42.0);
        assert_eq!(devices[0].memory_total, 25_753_026_560);
        assert_eq!(devices[0].memory_used, 1_073_741_824);
        assert_eq!(devices[0].temperature, 61);
        assert_eq!(devices[1].temperature, 0);
    }

    #[test]
    fn rejects_non_object_payloads() {
        assert!(parse_json("[]").is_err());
        assert!(parse_json("not json").is_err());
        assert!(parse_json(r#"{"card0": "nope"}"#).unwrap().0.is_empty());
    }
}
