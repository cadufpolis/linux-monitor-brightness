//! Dev utility: dumps every procedurally-generated icon to PNG files so
//! they can be eyeballed without running the whole plugin. Not part of the
//! plugin binary itself.
//!
//! Usage: `cargo run --example render_icons -- /tmp/icons`

use base64::{Engine as _, engine::general_purpose};

#[path = "../src/gfx.rs"]
mod gfx;

fn main() {
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
