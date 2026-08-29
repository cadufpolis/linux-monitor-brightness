use openaction::*;

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, sync::LazyLock};
use tokio::sync::Mutex;

use crate::{gfx, monitors};

/// Brightness adjustment applied per dial tick, in percentage points.
const BRIGHTNESS_STEP_PERCENT: i32 = 5;

/// Brightness the dial drops to when "dimmed" (press / short touch toggle).
const DIM_TARGET_PERCENT: u16 = 5;

/// Per-instance settings, configured from the property inspector: which
/// monitor(s) (by [`monitors::monitor_key`]-style stable id) this dial
/// controls. A dial can drive more than one monitor at once — rotating
/// nudges every selected monitor by the same delta.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct DialSettings {
    pub selected_monitor_keys: Vec<String>,
}

struct DimState {
    dimmed: bool,
    /// Brightness to restore to when un-dimming.
    restore_percent: u16,
}

/// Dim/restore state per dial instance. Not persisted — a plugin restart
/// just forgets any in-progress dim, which is a reasonable default.
static DIM_STATE: LazyLock<Mutex<HashMap<String, DimState>>> =
    LazyLock::new(|| Mutex::const_new(HashMap::new()));

pub struct MonitorBrightnessAction;

#[async_trait]
impl Action for MonitorBrightnessAction {
    const UUID: ActionUuid = "com.cadufpolis.monitor-brightness.dial";
    type Settings = DialSettings;

    async fn will_appear(&self, instance: &Instance, settings: &Self::Settings) -> OpenActionResult<()> {
        push_feedback(instance, settings).await;
        Ok(())
    }

    async fn will_disappear(&self, instance: &Instance, _settings: &Self::Settings) -> OpenActionResult<()> {
        DIM_STATE.lock().await.remove(&instance.instance_id);
        Ok(())
    }

    async fn did_receive_settings(&self, instance: &Instance, settings: &Self::Settings) -> OpenActionResult<()> {
        // The monitor selection changed in the property inspector; reflect it.
        push_feedback(instance, settings).await;
        Ok(())
    }

    /// Property inspector asking for the current monitor list (sent on load,
    /// and whenever its "Rescan" button is used).
    async fn send_to_plugin(
        &self,
        instance: &Instance,
        _settings: &Self::Settings,
        payload: &serde_json::Value,
    ) -> OpenActionResult<()> {
        if payload.get("event").and_then(|v| v.as_str()) == Some("getMonitors") {
            let monitors = tokio::task::spawn_blocking(monitors::list_monitors)
                .await
                .unwrap_or_default();
            let _ = instance
                .send_to_property_inspector(json!({ "event": "monitors", "monitors": monitors }))
                .await;
        }
        Ok(())
    }

    async fn dial_rotate(
        &self,
        instance: &Instance,
        settings: &Self::Settings,
        ticks: i16,
        _pressed: bool,
    ) -> OpenActionResult<()> {
        if ticks == 0 || settings.selected_monitor_keys.is_empty() {
            return Ok(());
        }

        let delta = BRIGHTNESS_STEP_PERCENT * ticks as i32;
        adjust_selected_brightness(settings.selected_monitor_keys.clone(), delta).await;
        push_feedback(instance, settings).await;

        Ok(())
    }

    async fn dial_down(&self, instance: &Instance, settings: &Self::Settings) -> OpenActionResult<()> {
        toggle_dim(instance, settings).await;
        Ok(())
    }

    async fn touch_tap(
        &self,
        instance: &Instance,
        settings: &Self::Settings,
        _position: (u16, u16),
        hold: bool,
    ) -> OpenActionResult<()> {
        if hold {
            // Long touch: re-enumerate and refresh, in case a monitor was
            // just plugged in or turned on.
            push_feedback(instance, settings).await;
        } else {
            // Short tap: same as pressing the dial.
            toggle_dim(instance, settings).await;
        }
        Ok(())
    }
}

/// Toggle between the current brightness and [`DIM_TARGET_PERCENT`] for
/// every monitor this dial controls, mirroring the mute gesture on the
/// sibling volume-controller plugin's dial.
async fn toggle_dim(instance: &Instance, settings: &DialSettings) {
    let keys = settings.selected_monitor_keys.clone();
    if keys.is_empty() {
        return;
    }

    let currently_dimmed = DIM_STATE
        .lock()
        .await
        .get(&instance.instance_id)
        .map(|s| s.dimmed)
        .unwrap_or(false);

    if currently_dimmed {
        let restore = DIM_STATE
            .lock()
            .await
            .get(&instance.instance_id)
            .map(|s| s.restore_percent)
            .unwrap_or(100);

        DIM_STATE.lock().await.insert(
            instance.instance_id.clone(),
            DimState { dimmed: false, restore_percent: restore },
        );
        set_selected_brightness(keys, restore).await;
    } else {
        let avg = selected_monitors_avg_brightness(keys.clone())
            .await
            .unwrap_or(100);
        // Never "restore" back into the dim range if it was already low.
        let restore_percent = avg.max(DIM_TARGET_PERCENT + 5);

        DIM_STATE.lock().await.insert(
            instance.instance_id.clone(),
            DimState { dimmed: true, restore_percent },
        );
        set_selected_brightness(keys, DIM_TARGET_PERCENT).await;
    }

    push_feedback(instance, settings).await;
}

/// Push the dial's icon/title/value/bar to its touchscreen segment.
async fn push_feedback(instance: &Instance, settings: &DialSettings) {
    if settings.selected_monitor_keys.is_empty() {
        let feedback = json!({
            "icon": gfx::IDLE_ICON.as_str(),
            // An empty title falls back to showing the action's own name
            // ("Monitor Brightness Dial") instead of just disappearing, so
            // it has to be disabled outright rather than set to "".
            "title": { "enabled": false },
            "value": "",
            "indicator": { "value": 0 },
        });
        let _ = instance.set_feedback(&feedback).await;
        return;
    }

    let avg = selected_monitors_avg_brightness(settings.selected_monitor_keys.clone())
        .await
        .unwrap_or(0);

    let dimmed = DIM_STATE
        .lock()
        .await
        .get(&instance.instance_id)
        .map(|s| s.dimmed)
        .unwrap_or(false);

    let icon = if dimmed {
        gfx::BRIGHTNESS_ICON_DIMMED.as_str()
    } else {
        gfx::BRIGHTNESS_ICON.as_str()
    };

    let feedback = json!({
        "icon": icon,
        "title": monitor_count_label(settings.selected_monitor_keys.len()),
        "value": format!("{avg}%"),
        "indicator": { "value": avg },
    });
    let _ = instance.set_feedback(&feedback).await;
}

fn monitor_count_label(count: usize) -> String {
    match count {
        1 => "1 monitor".to_string(),
        n => format!("{n} monitors"),
    }
}

/// Average current brightness across the given monitors (missing/unreachable
/// ones are skipped). `ddc-hi` calls are blocking I2C I/O, so they're run on
/// a blocking-friendly thread rather than the async runtime.
async fn selected_monitors_avg_brightness(keys: Vec<String>) -> Option<u16> {
    if keys.is_empty() {
        return None;
    }

    let values = tokio::task::spawn_blocking(move || {
        keys.iter()
            .filter_map(|key| monitors::get_brightness(key).ok())
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();

    if values.is_empty() {
        None
    } else {
        Some((values.iter().map(|&v| v as u32).sum::<u32>() / values.len() as u32) as u16)
    }
}

/// Nudge every given monitor's brightness by `delta` percentage points
/// (each from its own current value, clamped to 0-100).
async fn adjust_selected_brightness(keys: Vec<String>, delta: i32) {
    tokio::task::spawn_blocking(move || {
        for key in &keys {
            let Ok(current) = monitors::get_brightness(key) else {
                println!("Warning: failed to read brightness for {key}");
                continue;
            };
            let next = (current as i32 + delta).clamp(0, 100) as u16;
            if let Err(e) = monitors::set_brightness(key, next) {
                println!("Warning: failed to set brightness for {key}: {e}");
            }
        }
    })
    .await
    .ok();
}

/// Set every given monitor to the same absolute brightness.
async fn set_selected_brightness(keys: Vec<String>, percent: u16) {
    tokio::task::spawn_blocking(move || {
        for key in &keys {
            if let Err(e) = monitors::set_brightness(key, percent) {
                println!("Warning: failed to set brightness for {key}: {e}");
            }
        }
    })
    .await
    .ok();
}

pub async fn init() -> OpenActionResult<()> {
    println!("Stream Deck connected - Monitor Brightness plugin ready");

    register_action(MonitorBrightnessAction).await;

    run(std::env::args().collect()).await
}
