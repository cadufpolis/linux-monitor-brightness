use openaction::*;

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, sync::LazyLock, time::Duration};
use tokio::sync::Mutex;

use crate::{gfx, monitors};

/// Brightness adjustment applied per dial tick, in percentage points.
const BRIGHTNESS_STEP_PERCENT: i32 = 5;

/// Default for [`DialSettings::dim_target_percent`] when not set from the
/// property inspector.
const DEFAULT_DIM_TARGET_PERCENT: u16 = 5;

/// Default for [`DialSettings::debounce_ms`] when not set from the property
/// inspector. Each rotate tick used to trigger its own blocking I2C
/// read+write; spinning the dial fired those faster than a monitor's DDC
/// firmware could keep up, which is what was causing the lockups — so ticks
/// now only update an in-memory value + the on-screen bar, and the real
/// hardware write is debounced to once per "the user stopped turning it".
const DEFAULT_DEBOUNCE_MS: u32 = 1000;

/// Per-instance settings, configured from the property inspector.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct DialSettings {
    /// Which monitor(s) (by [`monitors::MonitorInfo::key`]-style stable id)
    /// this dial controls. A dial can drive more than one monitor at once —
    /// rotating nudges every selected monitor by the same delta.
    pub selected_monitor_keys: Vec<String>,
    /// Overrides the auto-generated title. Empty means "auto": the selected
    /// monitor's name for a single selection, or "N displays" for more.
    pub custom_name: String,
    /// How long, in milliseconds, the dial has to sit still after the last
    /// rotate tick before the brightness change is actually sent over
    /// DDC/CI. See [`DEFAULT_DEBOUNCE_MS`].
    pub debounce_ms: u32,
    /// Brightness (0-100) the dial drops to when "dimmed" (press / short
    /// touch toggle). See [`DEFAULT_DIM_TARGET_PERCENT`].
    pub dim_target_percent: u16,
}

impl Default for DialSettings {
    fn default() -> Self {
        DialSettings {
            selected_monitor_keys: Vec::new(),
            custom_name: String::new(),
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            dim_target_percent: DEFAULT_DIM_TARGET_PERCENT,
        }
    }
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

/// Last known (or optimistically assumed, while a rotation is still
/// debouncing) brightness per monitor key. Shared across every dial, since
/// brightness is a property of the monitor, not of any one dial instance.
/// This is what lets rotation update the bar on every tick without hitting
/// the hardware on every tick.
static BRIGHTNESS_CACHE: LazyLock<Mutex<HashMap<String, u16>>> =
    LazyLock::new(|| Mutex::const_new(HashMap::new()));

/// Human-readable label per monitor key (e.g. "Dell U2720Q (#ABC123)"),
/// used for the auto-generated single-monitor title. Populated whenever the
/// property inspector asks for the monitor list, and lazily on first need
/// otherwise (see [`ensure_labels_cached`]) — labels never change on their
/// own, so there's no need to ever refresh an entry once seen.
static MONITOR_LABEL_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::const_new(HashMap::new()));

/// Bumped on every rotation of a given dial; a debounce task only commits
/// its write if the generation it captured is still current when it wakes
/// up, i.e. nobody rotated again in the meantime.
static ROTATE_GENERATION: LazyLock<Mutex<HashMap<String, u64>>> =
    LazyLock::new(|| Mutex::const_new(HashMap::new()));

pub struct MonitorBrightnessAction;

#[async_trait]
impl Action for MonitorBrightnessAction {
    const UUID: ActionUuid = "com.cadufpolis.monitor-brightness.dial";
    type Settings = DialSettings;

    async fn will_appear(&self, instance: &Instance, settings: &Self::Settings) -> OpenActionResult<()> {
        // Paint once, immediately, from whatever's already cached. See
        // `push_feedback_quick`'s doc comment for why this can't just call
        // the full `push_feedback` inline here — and why this handler tries
        // hard to never send more than this one `setFeedback` call.
        let (quick_avg, quick_title) = push_feedback_quick(instance, settings).await;

        if !settings.selected_monitor_keys.is_empty() {
            let instance_id = instance.instance_id.clone();
            let settings = settings.clone();
            tokio::spawn(async move {
                if let Some(instance) = get_instance(instance_id).await {
                    let avg = cached_avg_brightness(&settings.selected_monitor_keys).await;
                    let title = dial_title(&settings).await;
                    // Only repaint if the real, hardware-backed data
                    // actually differs from the quick paint above — see
                    // `push_feedback_quick`'s doc comment for why a
                    // no-op resend here is worth avoiding, not just wasteful.
                    if avg != quick_avg || title != quick_title {
                        render_feedback(&instance, &settings, avg, title).await;
                    }
                }
            });
        }

        Ok(())
    }

    async fn will_disappear(&self, instance: &Instance, _settings: &Self::Settings) -> OpenActionResult<()> {
        DIM_STATE.lock().await.remove(&instance.instance_id);
        ROTATE_GENERATION.lock().await.remove(&instance.instance_id);
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

            cache_labels(&monitors).await;

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

        // A manual nudge overrides any pending "dimmed" bookkeeping — the
        // next press should dim from here, not restore to a stale value.
        DIM_STATE.lock().await.remove(&instance.instance_id);

        let delta = BRIGHTNESS_STEP_PERCENT * ticks as i32;
        bump_cached_brightness(settings.selected_monitor_keys.clone(), delta).await;

        // Reflect the change immediately — this is all in-memory, no hardware I/O.
        push_feedback(instance, settings).await;

        // Actually writing to the monitor(s) is debounced; see `debounce_ms`.
        schedule_brightness_commit(
            instance.instance_id.clone(),
            settings.selected_monitor_keys.clone(),
            settings.debounce_ms,
        )
        .await;

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

/// Toggle between the current brightness and `settings.dim_target_percent`
/// for every monitor this dial controls, mirroring the mute gesture on the
/// sibling volume-controller plugin's dial. Unlike rotation this applies
/// immediately (no debounce) — it's a single, deliberate press.
async fn toggle_dim(instance: &Instance, settings: &DialSettings) {
    let keys = settings.selected_monitor_keys.clone();
    if keys.is_empty() {
        return;
    }

    let dim_target = settings.dim_target_percent.min(100);

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
        let avg = cached_avg_brightness(&keys).await.unwrap_or(100);
        // Never "restore" back into the dim range if it was already low.
        let restore_percent = avg.max(dim_target + 5).min(100);

        DIM_STATE.lock().await.insert(
            instance.instance_id.clone(),
            DimState { dimmed: true, restore_percent },
        );
        set_selected_brightness(keys, dim_target).await;
    }

    push_feedback(instance, settings).await;
}

/// Push the dial's icon/title/value/bar to its touchscreen segment, from
/// whatever [`BRIGHTNESS_CACHE`]/[`MONITOR_LABEL_CACHE`] currently believe —
/// reading hardware (a brightness read, or a monitor rescan for the title's
/// label) for whatever isn't cached yet. See [`push_feedback_quick`] for a
/// variant that never does that, used where blocking would stall other
/// instances' events.
async fn push_feedback(instance: &Instance, settings: &DialSettings) {
    let avg = cached_avg_brightness(&settings.selected_monitor_keys).await;
    let title = dial_title(settings).await;
    render_feedback(instance, settings, avg, title).await;
}

/// Same as [`push_feedback`], but only ever reads what's already cached —
/// never touches DDC/CI or rescans the I2C bus for a monitor's label. A
/// cold start's first scan can take several seconds (see
/// `monitors::DISPLAY_REGISTRY`'s doc comment), and every inbound event —
/// including every *other* instance's `dial_rotate` — is handled on one
/// sequential loop (see the crate's `while let Some(message) = ...` event
/// loop), so a `will_appear` that blocked on that scan used to stall
/// rotation feedback for the whole plugin, not just its own dial. So
/// `will_appear` calls this for an instant first paint, then kicks off a
/// background task that calls the real [`push_feedback`] once the hardware
/// answers — but only if the answer actually differs (returned here for
/// that comparison): each `setFeedback` costs OpenDeck a full async
/// render-and-write-to-device cycle, queued per dial, so firing a second,
/// identical one right behind the first just delays whatever the user does
/// next (e.g. the first rotate tick) behind a pointless extra queue entry.
async fn push_feedback_quick(instance: &Instance, settings: &DialSettings) -> (Option<u16>, String) {
    let avg = peek_avg_brightness(&settings.selected_monitor_keys).await;
    let title = peek_dial_title(settings).await;
    render_feedback(instance, settings, avg, title.clone()).await;
    (avg, title)
}

async fn render_feedback(instance: &Instance, settings: &DialSettings, avg: Option<u16>, title: String) {
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
        // Sent as an object (not a bare string) so `enabled: true` always
        // rides along — the idle branch above disables this same item, and
        // a bare string value only updates the text, never re-enabling it.
        "title": { "value": title, "enabled": true },
        // Still waiting on the very first hardware read (see
        // `push_feedback_quick`): show a neutral placeholder rather than a
        // misleading "0%".
        "value": avg.map(|v| format!("{v}%")).unwrap_or_else(|| "…".to_string()),
        "indicator": { "value": avg.unwrap_or(0) },
    });
    let _ = instance.set_feedback(&feedback).await;
}

/// The dial's display name: the user's custom name if set, else the
/// selected monitor's own label for a single selection, else "N displays".
async fn dial_title(settings: &DialSettings) -> String {
    let custom = settings.custom_name.trim();
    if !custom.is_empty() {
        return custom.to_string();
    }

    match settings.selected_monitor_keys.as_slice() {
        [key] => {
            ensure_labels_cached(std::slice::from_ref(key)).await;
            MONITOR_LABEL_CACHE
                .lock()
                .await
                .get(key)
                .cloned()
                .unwrap_or_else(|| "1 monitor".to_string())
        }
        keys => format!("{} displays", keys.len()),
    }
}

/// Same as [`dial_title`], but only reads [`MONITOR_LABEL_CACHE`] as it
/// currently stands — never triggers [`ensure_labels_cached`]'s rescan.
async fn peek_dial_title(settings: &DialSettings) -> String {
    let custom = settings.custom_name.trim();
    if !custom.is_empty() {
        return custom.to_string();
    }

    match settings.selected_monitor_keys.as_slice() {
        [key] => MONITOR_LABEL_CACHE
            .lock()
            .await
            .get(key)
            .cloned()
            .unwrap_or_else(|| "1 monitor".to_string()),
        keys => format!("{} displays", keys.len()),
    }
}

/// Make sure every given monitor has an entry in [`MONITOR_LABEL_CACHE`],
/// doing a single fresh scan (covers every connected monitor, not just the
/// ones asked for) if any of them is missing.
async fn ensure_labels_cached(keys: &[String]) {
    let missing = {
        let cache = MONITOR_LABEL_CACHE.lock().await;
        keys.iter().any(|key| !cache.contains_key(key))
    };
    if !missing {
        return;
    }

    let monitors = tokio::task::spawn_blocking(monitors::list_monitors).await.unwrap_or_default();
    cache_labels(&monitors).await;
}

async fn cache_labels(found: &[monitors::MonitorInfo]) {
    let mut cache = MONITOR_LABEL_CACHE.lock().await;
    for monitor in found {
        cache.insert(monitor.key.clone(), monitor.label.clone());
    }
}

/// Make sure every given monitor has an entry in [`BRIGHTNESS_CACHE`],
/// reading hardware (once per monitor, ever, until the plugin restarts) for
/// whichever ones don't yet.
async fn ensure_cached(keys: &[String]) {
    let to_fetch: Vec<String> = {
        let cache = BRIGHTNESS_CACHE.lock().await;
        keys.iter().filter(|k| !cache.contains_key(*k)).cloned().collect()
    };
    if to_fetch.is_empty() {
        return;
    }

    let fetched = tokio::task::spawn_blocking(move || {
        to_fetch
            .into_iter()
            .filter_map(|key| monitors::get_brightness(&key).ok().map(|v| (key, v)))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();

    let mut cache = BRIGHTNESS_CACHE.lock().await;
    for (key, value) in fetched {
        cache.entry(key).or_insert(value);
    }
}

/// Same as [`cached_avg_brightness`], but only reads [`BRIGHTNESS_CACHE`]
/// as it currently stands — never triggers [`ensure_cached`]'s hardware
/// fetch for a monitor it hasn't seen yet.
async fn peek_avg_brightness(keys: &[String]) -> Option<u16> {
    if keys.is_empty() {
        return None;
    }

    let cache = BRIGHTNESS_CACHE.lock().await;
    let values: Vec<u16> = keys.iter().filter_map(|key| cache.get(key).copied()).collect();
    drop(cache);

    if values.is_empty() {
        None
    } else {
        Some((values.iter().map(|&v| v as u32).sum::<u32>() / values.len() as u32) as u16)
    }
}

/// Average cached brightness across the given monitors (missing/unreachable
/// ones — never successfully read — are skipped).
async fn cached_avg_brightness(keys: &[String]) -> Option<u16> {
    if keys.is_empty() {
        return None;
    }

    ensure_cached(keys).await;

    let cache = BRIGHTNESS_CACHE.lock().await;
    let values: Vec<u16> = keys.iter().filter_map(|key| cache.get(key).copied()).collect();
    drop(cache);

    if values.is_empty() {
        None
    } else {
        Some((values.iter().map(|&v| v as u32).sum::<u32>() / values.len() as u32) as u16)
    }
}

/// Nudge every given monitor's cached brightness by `delta` percentage
/// points (each from its own current value, clamped to 0-100). Pure
/// in-memory bookkeeping — does not touch the hardware.
async fn bump_cached_brightness(keys: Vec<String>, delta: i32) {
    ensure_cached(&keys).await;

    let mut cache = BRIGHTNESS_CACHE.lock().await;
    for key in &keys {
        let current = cache.get(key).copied().unwrap_or(50);
        let next = (current as i32 + delta).clamp(0, 100) as u16;
        cache.insert(key.clone(), next);
    }
}

/// Set every given monitor to the same absolute brightness, immediately
/// (blocking I2C write), and record the result in the cache.
async fn set_selected_brightness(keys: Vec<String>, percent: u16) {
    let write_keys = keys.clone();
    tokio::task::spawn_blocking(move || {
        for key in &write_keys {
            if let Err(e) = monitors::set_brightness(key, percent) {
                println!("Warning: failed to set brightness for {key}: {e}");
            }
        }
    })
    .await
    .ok();

    let mut cache = BRIGHTNESS_CACHE.lock().await;
    for key in keys {
        cache.insert(key, percent);
    }
}

/// Debounce a dial's pending rotation: wait for `debounce_ms` of silence,
/// then — only if nothing rotated it again in the meantime — write every
/// selected monitor's cached (already-adjusted) brightness for real.
async fn schedule_brightness_commit(instance_id: String, keys: Vec<String>, debounce_ms: u32) {
    let generation = {
        let mut generations = ROTATE_GENERATION.lock().await;
        let next = generations.get(&instance_id).copied().unwrap_or(0) + 1;
        generations.insert(instance_id.clone(), next);
        next
    };

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(debounce_ms as u64)).await;

        let still_current = ROTATE_GENERATION.lock().await.get(&instance_id).copied() == Some(generation);
        if !still_current {
            return; // Superseded by a later tick; that task will commit instead.
        }

        let targets: Vec<(String, u16)> = {
            let cache = BRIGHTNESS_CACHE.lock().await;
            keys.iter().filter_map(|key| cache.get(key).map(|v| (key.clone(), *v))).collect()
        };

        tokio::task::spawn_blocking(move || {
            for (key, percent) in &targets {
                if let Err(e) = monitors::set_brightness(key, *percent) {
                    println!("Warning: failed to set brightness for {key}: {e}");
                }
            }
        })
        .await
        .ok();
    });
}

pub async fn init() -> OpenActionResult<()> {
    println!("Stream Deck connected - Monitor Brightness plugin ready");

    // Kick off the (possibly several-second) first I2C bus scan right away
    // in the background, so it's likely already done — or at least already
    // running, instead of starting from scratch — by the time any dial's
    // `will_appear` needs it.
    tokio::spawn(async {
        let _ = tokio::task::spawn_blocking(monitors::list_monitors).await;
    });

    register_action(MonitorBrightnessAction).await;

    run(std::env::args().collect()).await
}
