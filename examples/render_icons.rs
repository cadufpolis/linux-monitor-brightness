//! Dev utility: (re)generates `img/icon.png` and `img/grid-slider.png` from
//! the same procedural sun-with-rays glyph used on the dial itself (so the
//! plugin-list icon, the action-grid icon, and the dial icon never drift out
//! of sync with each other), and dumps every dial icon variant to PNG so
//! they can be eyeballed without running the whole plugin.
//!
//! Usage: `cargo run --example render_icons [-- /tmp/icons]`

use base64::{Engine as _, engine::general_purpose};
use std::path::Path;

#[path = "../src/gfx.rs"]
mod gfx;

/// Resolution for the project-level icons (`img/*.png`). Independent of the
/// dial's own on-device icon size.
const PROJECT_ICON_SIZE: u32 = 256;

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let img_dir = Path::new(manifest_dir).join("img");

    let icon_path = img_dir.join("icon.png");
    let grid_slider_path = img_dir.join("grid-slider.png");
    for path in [&icon_path, &grid_slider_path] {
        gfx::sun_icon(PROJECT_ICON_SIZE, gfx::BRIGHTNESS_COLOR)
            .save(path)
            .unwrap_or_else(|e| panic!("Failed to write {path:?}: {e}"));
        println!("Wrote {}", path.display());
    }

    let out_dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp/icons".to_string());
    std::fs::create_dir_all(&out_dir).unwrap();

    for (name, uri) in [
        ("brightness", &*gfx::BRIGHTNESS_ICON),
        ("brightness_dimmed", &*gfx::BRIGHTNESS_ICON_DIMMED),
        ("idle", &*gfx::IDLE_ICON),
    ] {
        let b64 = uri.strip_prefix("data:image/png;base64,").unwrap();
        let bytes = general_purpose::STANDARD.decode(b64).unwrap();
        let path = format!("{out_dir}/{name}.png");
        std::fs::write(&path, bytes).unwrap();
        println!("Wrote {path}");
    }
}
