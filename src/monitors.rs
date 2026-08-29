//! Thin wrapper around `ddc-hi` for enumerating DDC/CI-capable monitors and
//! reading/writing their brightness (MCCS VCP feature 0x10).

use anyhow::{Context, Result};
use ddc_hi::{Ddc, Display, DisplayInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

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

/// Already-open DDC/CI handles, keyed by [`monitor_key`]. `Display::enumerate()`
/// scans *every* I2C bus on the system — not just monitors, everything: SMBus
/// controllers, GPU sensor buses, RAM SPD, whatever else shows up under
/// `/dev/i2c-*` — probing each one for a readable EDID, which on a desktop
/// motherboard with a couple dozen unrelated buses can take several seconds
/// on its own. Re-running that scan on every single brightness read/write
/// (as an earlier version of this file did) was the actual cause of a fixed,
/// multi-second delay regardless of any debounce setting. Everything here is
/// built around scanning once and reusing the handle from then on.
static DISPLAY_REGISTRY: LazyLock<Mutex<HashMap<String, Display>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock_registry() -> std::sync::MutexGuard<'static, HashMap<String, Display>> {
    // Recover from a poisoned lock rather than taking the whole plugin down
    // if a previous DDC call ever panicked mid-transaction.
    DISPLAY_REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// Re-scan every I2C bus and (re)populate [`DISPLAY_REGISTRY`]. This is the
/// slow operation everything else tries to only pay for once — explicitly
/// for the property inspector's list / the dial's "rescan" gesture, and
/// implicitly the first time a monitor key is looked up and isn't found yet.
fn rescan() -> Vec<MonitorInfo> {
    let displays = Display::enumerate();

    let infos = displays
        .iter()
        .map(|display| MonitorInfo {
            key: monitor_key(&display.info),
            label: monitor_label(&display.info),
        })
        .collect();

    let mut registry = lock_registry();
    registry.clear();
    for display in displays {
        registry.insert(monitor_key(&display.info), display);
    }

    infos
}

/// Enumerate every DDC/CI-capable monitor currently detected. Always does a
/// fresh scan — this is the one place in this module that's expected to be
/// slow-ish (see [`DISPLAY_REGISTRY`]'s doc comment).
///
/// Requires read/write access to `/dev/i2c-*` (see the README for udev rule
/// setup) — a monitor that doesn't support DDC/CI, or that the current user
/// can't reach, simply won't show up here.
pub fn list_monitors() -> Vec<MonitorInfo> {
    rescan()
}

/// Read the current brightness (0-100) of a monitor by its stable key.
pub fn get_brightness(key: &str) -> Result<u16> {
    with_display(key, |display| {
        let value = display
            .handle
            .get_vcp_feature(VCP_BRIGHTNESS)
            .context("Failed to read brightness (VCP 0x10)")?;
        Ok(value.value())
    })
}

/// Set the brightness (0-100) of a monitor by its stable key.
pub fn set_brightness(key: &str, percent: u16) -> Result<()> {
    with_display(key, |display| {
        display
            .handle
            .set_vcp_feature(VCP_BRIGHTNESS, percent.min(100))
            .context("Failed to set brightness (VCP 0x10)")
    })
}

/// Run `f` against the registered handle for `key`, triggering a rescan
/// first only if the registry doesn't have it yet — a fresh plugin start,
/// or a monitor that appeared after the last scan.
fn with_display<T>(key: &str, f: impl FnOnce(&mut Display) -> Result<T>) -> Result<T> {
    if let Some(display) = lock_registry().get_mut(key) {
        return f(display);
    }

    rescan();

    let mut registry = lock_registry();
    let display = registry
        .get_mut(key)
        .with_context(|| format!("Monitor '{key}' not found (unplugged, or hot-plug order changed)"))?;
    f(display)
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
