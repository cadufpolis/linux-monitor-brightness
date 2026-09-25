# OA Monitor Brightness Plugin

A DDC/CI monitor-brightness control plugin for Stream Deck using OpenDeck on Linux, scaffolded from the same structure as [opendeck-volume-controller](../opendeck-volume-controller).

## Overview

Control the brightness of one or more monitors straight from a Stream Deck+ dial. Pick which monitor(s) a dial drives in its settings panel; rotating adjusts brightness, pressing (or tapping the touchscreen) toggles a quick dim.

This is a **v1 skeleton**, built dial-only on purpose — see [Status & next steps](#status--next-steps).

## Requirements

- Monitor(s) with DDC/CI support (most external monitors; laptop panels usually don't expose it).
- Linux `i2c-dev` access:
  ```sh
  sudo modprobe i2c-dev
  ```
  and a udev rule granting your user read/write access to `/dev/i2c-*`, e.g. `/etc/udev/rules.d/45-ddcutil-i2c.rules`:
  ```
  KERNEL=="i2c-[0-9]*", GROUP="i2c", MODE="0660"
  ```
  then `sudo groupadd -f i2c && sudo usermod -aG i2c $USER` and re-login. (Same setup [`ddcutil`](https://www.ddcutil.com/) documents — installing it is a good way to sanity-check DDC/CI works on your monitor(s) before trying the plugin.)

Heads up: the very first brightness read for a given monitor after the plugin starts (or after **Rescan monitors**) can take a few seconds — `ddc-hi`'s `Display::enumerate()` scans *every* `/dev/i2c-*` bus on the system probing for a readable EDID, not just the ones with a monitor on them, and a desktop motherboard can easily expose a couple dozen unrelated ones (SMBus, RAM SPD, sensors, ...). The plugin only pays that cost once and keeps the handle open after — see `DISPLAY_REGISTRY` in `src/monitors.rs` — so every read/write after the first is fast regardless of the rotate delay setting below. That first scan starts in the background as soon as the plugin launches (and again in the background the moment a dial appears), so a dial shows *something* — its last-known value, or a `…` placeholder while the scan is still running — right away instead of the screen freezing until the scan finishes.

## Usage

### Dial (Stream Deck+)

Drag the `Monitor Brightness Dial` action onto a dial. Open its settings panel (property inspector) and check the monitor(s) it should control — the list is populated live from a DDC/CI scan; use **Rescan monitors** if you plug one in after opening the panel.

- **Rotate** the dial to adjust brightness (all selected monitors move together, each from its own current value). The bar/percentage on screen updates on every tick, but the actual DDC/CI write is debounced — see **Rotate delay** below — since most monitors' DDC firmware is slow and can lock up if commands are fired faster than that (one write per tick was the cause of early lockups during testing).
- **Press** the dial, or **tap** the touchscreen segment, to toggle a quick dim (drops to **Quick dim level** below, default `5%`, remembers where to restore to). This one applies immediately, no debounce.
- **Press-and-hold** the touchscreen (long touch) to rescan monitors and refresh the display.

Settings panel fields:

- **Dial name** — overrides the title shown on screen. Left blank, it defaults to the selected monitor's own name (single selection) or "N displays" (more than one).
- **Monitors controlled by this dial** — the checkbox list.
- **Rotate delay** — how long, in ms, the dial has to sit still after the last tick before the brightness change is actually sent (default `1000`). Raise it if a monitor still struggles to keep up; lower it for snappier response on a monitor that handles DDC/CI fine.
- **Quick dim level** — brightness (%) the press/tap toggle drops to (default `5`).

## Project layout

Mirrors `opendeck-volume-controller`'s structure:

- `src/main.rs` — entrypoint, logger setup, hands off to `plugin::init()`.
- `src/plugin.rs` — the `Action` implementation (dial events, settings, property-inspector messaging).
- `src/monitors.rs` — thin `ddc-hi` wrapper: enumerate monitors, get/set brightness (VCP `0x10`).
- `src/gfx.rs` — procedurally-generated placeholder icons (swap for real assets in `img/` + reference them in the feedback JSON whenever you'd rather ship bitmaps).
- `manifest.json`, `pi.html` — plugin manifest and settings UI, same wire protocol as the volume controller (`setSettings`/`didReceiveSettings`, plus `sendToPlugin`/`sendToPropertyInspector` for the live monitor list).
- `.github/workflows/build.yml` — same build → package → (draft) release pipeline, renamed.

Notably **absent** compared to the volume controller: there's no dynamic "column → channel" mapping layer (`COLUMN_TO_CHANNEL_MAP`/`MIXER_CHANNELS`). That machinery exists there because *which app* lives on *which button* is discovered at runtime from PulseAudio. Here, each dial owns its monitor selection directly in its own persisted settings, so there's nothing to reconcile — a real simplification worth keeping in mind before copying that pattern into a new plugin that doesn't need it.

## Status & next steps

Deliberately out of scope for v1, in rough order of usefulness:

- **Keypad support.** Only `Controllers: ["Encoder"]` is declared in the manifest; add `"Keypad"` plus `ControllerKind`-style handling (see the volume controller's `src/utils.rs`) if you want a 3-row grid variant too.
- **Real icons.** `src/gfx.rs` draws a sun-with-rays glyph at runtime as a placeholder (`cargo run --example render_icons -- /tmp/icons` dumps every variant to PNG for a quick look); swap in real PNG/SVG assets under `img/` once you have some.
- **Reflect external changes.** Brightness shown on the dial is only ever what the plugin itself last set — an OSD button on the monitor or another app changing it won't be picked up until the next interaction (`will_appear`/rotate/tap). A periodic re-poll (careful: DDC/CI over I2C is slow, don't poll too often) would fix that.
- **Per-monitor grouping polish.** Multiple monitors on one dial currently show/average brightness and nudge each independently by the same delta — fine for monitors kept roughly in sync, but there's no per-monitor curve/gamma matching.
