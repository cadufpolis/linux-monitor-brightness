//! Procedurally-generated icons for the dial's touchscreen segment, so the
//! plugin doesn't need to ship bitmap assets for every state. Same approach
//! as the sibling `opendeck-volume-controller` project's `TRANSPARENT_ICON`.

use base64::{Engine as _, engine::general_purpose};
use image::{Rgba, RgbaImage};
use std::io::Cursor;
use std::sync::LazyLock;

const ICON_SIZE: u32 = 144;

/// Bright, full-color sun icon shown while a dial's monitor(s) are at their
/// normal (non-dimmed) brightness. Swap this for a real asset whenever.
pub static BRIGHTNESS_ICON: LazyLock<String> =
    LazyLock::new(|| encode_icon(filled_circle(Rgba([245, 197, 66, 255]))));

/// Same icon, faded, shown while the dial is in the "dimmed" toggle state.
pub static BRIGHTNESS_ICON_DIMMED: LazyLock<String> =
    LazyLock::new(|| encode_icon(filled_circle(Rgba([245, 197, 66, 90]))));

/// Faint gray placeholder shown on a dial with no monitor selected yet
/// (nothing configured in the property inspector).
pub static IDLE_ICON: LazyLock<String> =
    LazyLock::new(|| encode_icon(filled_circle(Rgba([160, 160, 160, 90]))));

fn filled_circle(color: Rgba<u8>) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(ICON_SIZE, ICON_SIZE, Rgba([0, 0, 0, 0]));
    let center = ICON_SIZE as f32 / 2.0;
    let radius = center * 0.8;

    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            if dx * dx + dy * dy <= radius * radius {
                img.put_pixel(x, y, color);
            }
        }
    }

    img
}

fn encode_icon(img: RgbaImage) -> String {
    let mut buffer = Vec::new();
    let mut cursor = Cursor::new(&mut buffer);
    img.write_to(&mut cursor, image::ImageFormat::Png)
        .expect("Failed to encode icon");

    let base64 = general_purpose::STANDARD.encode(&buffer);
    format!("data:image/png;base64,{}", base64)
}
