//! Procedurally-generated icons for the dial's touchscreen segment, so the
//! plugin doesn't need to ship bitmap assets for every state. Same approach
//! as the sibling `opendeck-volume-controller` project's `TRANSPARENT_ICON`.

use base64::{Engine as _, engine::general_purpose};
use image::{Rgba, RgbaImage};
use std::f32::consts::FRAC_PI_4;
use std::io::Cursor;
use std::sync::LazyLock;

const ICON_SIZE: u32 = 144;

/// Bright sun-with-rays icon (the conventional "brightness" glyph), shown
/// while a dial's monitor(s) are at their normal (non-dimmed) brightness.
/// Swap this for a real asset whenever.
pub static BRIGHTNESS_ICON: LazyLock<String> =
    LazyLock::new(|| encode_icon(sun_icon(Rgba([245, 197, 66, 255]))));

/// Same icon, faded, shown while the dial is in the "dimmed" toggle state.
pub static BRIGHTNESS_ICON_DIMMED: LazyLock<String> =
    LazyLock::new(|| encode_icon(sun_icon(Rgba([245, 197, 66, 90]))));

/// Faint gray placeholder shown on a dial with no monitor selected yet
/// (nothing configured in the property inspector).
pub static IDLE_ICON: LazyLock<String> =
    LazyLock::new(|| encode_icon(sun_icon(Rgba([160, 160, 160, 90]))));

/// Draws the classic "brightness" glyph: a filled circle (the sun) with 8
/// short rays radiating out from it.
fn sun_icon(color: Rgba<u8>) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(ICON_SIZE, ICON_SIZE, Rgba([0, 0, 0, 0]));
    let center = ICON_SIZE as f32 / 2.0;

    let sun_radius = center * 0.42;
    let ray_inner = center * 0.6;
    let ray_outer = center * 0.88;
    let ray_thickness = center * 0.16;

    fill_circle(&mut img, center, center, sun_radius, color);

    for i in 0..8 {
        let angle = i as f32 * FRAC_PI_4;
        let (sin, cos) = angle.sin_cos();
        let x1 = center + cos * ray_inner;
        let y1 = center + sin * ray_inner;
        let x2 = center + cos * ray_outer;
        let y2 = center + sin * ray_outer;
        draw_thick_segment(&mut img, (x1, y1), (x2, y2), ray_thickness, color);
    }

    img
}

fn fill_circle(img: &mut RgbaImage, cx: f32, cy: f32, radius: f32, color: Rgba<u8>) {
    let min_x = (cx - radius).max(0.0) as u32;
    let max_x = (cx + radius).min(img.width() as f32 - 1.0) as u32;
    let min_y = (cy - radius).max(0.0) as u32;
    let max_y = (cy + radius).min(img.height() as f32 - 1.0) as u32;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy <= radius * radius {
                img.put_pixel(x, y, color);
            }
        }
    }
}

/// Paints every pixel within `thickness / 2` of the line segment `a..b`
/// (rounded ends, since it's a true point-to-segment distance test).
fn draw_thick_segment(img: &mut RgbaImage, a: (f32, f32), b: (f32, f32), thickness: f32, color: Rgba<u8>) {
    let half = thickness / 2.0;
    let min_x = (a.0.min(b.0) - half).max(0.0) as u32;
    let max_x = (a.0.max(b.0) + half).min(img.width() as f32 - 1.0) as u32;
    let min_y = (a.1.min(b.1) - half).max(0.0) as u32;
    let max_y = (a.1.max(b.1) + half).min(img.height() as f32 - 1.0) as u32;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if distance_to_segment((x as f32, y as f32), a, b) <= half {
                img.put_pixel(x, y, color);
            }
        }
    }
}

fn distance_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (proj_x, proj_y) = (a.0 + t * dx, a.1 + t * dy);
    let (ddx, ddy) = (p.0 - proj_x, p.1 - proj_y);
    (ddx * ddx + ddy * ddy).sqrt()
}

fn encode_icon(img: RgbaImage) -> String {
    let mut buffer = Vec::new();
    let mut cursor = Cursor::new(&mut buffer);
    img.write_to(&mut cursor, image::ImageFormat::Png)
        .expect("Failed to encode icon");

    let base64 = general_purpose::STANDARD.encode(&buffer);
    format!("data:image/png;base64,{}", base64)
}
