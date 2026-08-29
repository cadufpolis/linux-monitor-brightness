//! Thin wrapper around `ddc-hi` for enumerating DDC/CI-capable monitors and
//! reading/writing their brightness (MCCS VCP feature 0x10).

use anyhow::{Context, Result};
use ddc_hi::{Ddc, Display, DisplayInfo};
use serde::{Deserialize, Serialize};

/// VCP feature code for luminance/brightness, per the MCCS spec.
const VCP_BRIGHTNESS: u8 = 0x10;

/// A monitor as shown to the user (property inspector list, logs, etc.).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Stable identity to persist in an instance's settings. See [`monitor_key`].
    pub key: String,
    /// Human-readable label, e.g. "Dell U2720Q (#ABC123)".
    pub label: String,
}

/// Enumerate every DDC/CI-capable monitor currently detected.
///
/// Requires read/write access to `/dev/i2c-*` (see the README for udev rule
/// setup) — a monitor that doesn't support DDC/CI, or that the current user
/// can't reach, simply won't show up here.
pub fn list_monitors() -> Vec<MonitorInfo> {
    Display::enumerate()
        .into_iter()
        .map(|display| MonitorInfo {
            key: monitor_key(&display.info),
            label: monitor_label(&display.info),
        })
        .collect()
}

/// Read the current brightness (0-100) of a monitor by its stable key.
pub fn get_brightness(key: &str) -> Result<u16> {
    let mut display = find_display(key)?;
    let value = display
        .handle
        .get_vcp_feature(VCP_BRIGHTNESS)
        .context("Failed to read brightness (VCP 0x10)")?;
    Ok(value.value())
}

/// Set the brightness (0-100) of a monitor by its stable key.
pub fn set_brightness(key: &str, percent: u16) -> Result<()> {
    let mut display = find_display(key)?;
    display
        .handle
        .set_vcp_feature(VCP_BRIGHTNESS, percent.min(100))
        .context("Failed to set brightness (VCP 0x10)")
}

fn find_display(key: &str) -> Result<Display> {
    Display::enumerate()
        .into_iter()
        .find(|display| monitor_key(&display.info) == key)
        .with_context(|| format!("Monitor '{key}' not found (unplugged, or hot-plug order changed)"))
}

/// A stable identity for a monitor across plugin restarts / device rescans.
///
/// `DisplayInfo::id` is a raw device node number for the `ddc-i2c` backend
/// and can shift across reboots or when ports are hot-plugged in a different
/// order, so persisted selections (in an instance's settings) key on the
/// EDID-derived manufacturer/model/serial instead, which stays constant for
/// as long as it's the same physical monitor.
fn monitor_key(info: &DisplayInfo) -> String {
    format!(
        "{}|{}|{}",
        info.manufacturer_id.as_deref().unwrap_or(""),
        info.model_name.as_deref().unwrap_or(""),
        info.serial_number
            .clone()
            .or_else(|| info.serial.map(|s| s.to_string()))
            .unwrap_or_default(),
    )
}

fn monitor_label(info: &DisplayInfo) -> String {
    let model = info.model_name.as_deref().unwrap_or("Unknown monitor");
    match (&info.manufacturer_id, &info.serial_number) {
        (Some(mfg), Some(serial)) => format!("{mfg} {model} (#{serial})"),
        (Some(mfg), None) => format!("{mfg} {model}"),
        _ => model.to_string(),
    }
}
