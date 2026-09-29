//! Branch colors. Ports of IntelliJ `GraphColorManagerImpl` and `DefaultColorGenerator`.

pub const DEFAULT_COLOR: i32 = 0;

/// Java `String.hashCode()`, used by IntelliJ to derive the color of a named branch.
pub fn java_string_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

/// Color id of a node: the named color of the head branch for the head's own
/// fragment, otherwise the fragment's layout index.
pub fn color_id(head_ref_name: Option<&str>, head_layout_index: u32, layout_index: u32) -> i32 {
    if head_layout_index == layout_index {
        return head_ref_name.map(java_string_hash).unwrap_or(DEFAULT_COLOR);
    }
    layout_index as i32
}

/// `0xRRGGBB` for a color id, as IntelliJ's default generator computes it.
pub fn rgb_for_color_id(color_id: i32) -> u32 {
    rgb_for_color_id_with(color_id, 0.4, 0.65)
}

pub fn rgb_for_color_id_with(color_id: i32, saturation: f32, brightness: f32) -> u32 {
    let fix = |n: i32| (n % 100).abs() + 70;
    let r = fix(color_id.wrapping_mul(200).wrapping_add(30));
    let g = fix(color_id.wrapping_mul(130).wrapping_add(50));
    let b = fix(color_id.wrapping_mul(90).wrapping_add(100));
    let hue = rgb_to_hue(r, g, b);
    hsb_to_rgb(hue, saturation, brightness) & 0xFF_FFFF
}

/// Hue part of `java.awt.Color.RGBtoHSB`.
fn rgb_to_hue(r: i32, g: i32, b: i32) -> f32 {
    let cmax = r.max(g).max(b);
    let cmin = r.min(g).min(b);
    if cmax == 0 || cmax == cmin {
        return 0.0;
    }
    let range = (cmax - cmin) as f32;
    let rc = (cmax - r) as f32 / range;
    let gc = (cmax - g) as f32 / range;
    let bc = (cmax - b) as f32 / range;
    let mut hue = if r == cmax {
        bc - gc
    } else if g == cmax {
        2.0 + rc - bc
    } else {
        4.0 + gc - rc
    } / 6.0;
    if hue < 0.0 {
        hue += 1.0;
    }
    hue
}

/// `java.awt.Color.HSBtoRGB`.
fn hsb_to_rgb(hue: f32, saturation: f32, brightness: f32) -> u32 {
    let to = |v: f32| (v * 255.0 + 0.5) as u32;
    let (r, g, b) = if saturation == 0.0 {
        let v = to(brightness);
        (v, v, v)
    } else {
        let h = (hue - hue.floor()) * 6.0;
        let f = h - h.floor();
        let p = brightness * (1.0 - saturation);
        let q = brightness * (1.0 - saturation * f);
        let t = brightness * (1.0 - saturation * (1.0 - f));
        match h as i32 {
            0 => (to(brightness), to(t), to(p)),
            1 => (to(q), to(brightness), to(p)),
            2 => (to(p), to(brightness), to(t)),
            3 => (to(p), to(q), to(brightness)),
            4 => (to(t), to(p), to(brightness)),
            _ => (to(brightness), to(p), to(q)),
        }
    };
    (r << 16) | (g << 8) | b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_hash() {
        assert_eq!(java_string_hash(""), 0);
        assert_eq!(java_string_hash("master"), -1081267614);
    }
}
