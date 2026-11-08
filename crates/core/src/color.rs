//! Color picker text formats: hex, RGB and HSL, as pasted into CSS and design tools.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorFormat {
    /// `#1E90FF`
    #[default]
    Hex,
    /// `rgb(30, 144, 255)`
    Rgb,
    /// `hsl(210, 100%, 56%)`
    Hsl,
}

impl ColorFormat {
    pub const ALL: [ColorFormat; 3] = [ColorFormat::Hex, ColorFormat::Rgb, ColorFormat::Hsl];

    /// Button label.
    pub fn label(self) -> &'static str {
        match self {
            ColorFormat::Hex => "Hex",
            ColorFormat::Rgb => "RGB",
            ColorFormat::Hsl => "HSL",
        }
    }

    /// Config value (`[editor] picker_format`).
    pub fn config_name(self) -> &'static str {
        match self {
            ColorFormat::Hex => "hex",
            ColorFormat::Rgb => "rgb",
            ColorFormat::Hsl => "hsl",
        }
    }

    pub fn from_config(s: &str) -> ColorFormat {
        match s.trim().to_ascii_lowercase().as_str() {
            "rgb" => ColorFormat::Rgb,
            "hsl" => ColorFormat::Hsl,
            _ => ColorFormat::Hex,
        }
    }

    pub fn format(self, rgb: [u8; 3]) -> String {
        match self {
            ColorFormat::Hex => format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]),
            ColorFormat::Rgb => format!("rgb({}, {}, {})", rgb[0], rgb[1], rgb[2]),
            ColorFormat::Hsl => {
                let (h, s, l) = to_hsl(rgb);
                format!("hsl({h}, {s}%, {l}%)")
            }
        }
    }
}

/// Hue in degrees (0..360), saturation and lightness in percent, all rounded.
pub fn to_hsl(rgb: [u8; 3]) -> (u32, u32, u32) {
    let [r, g, b] = rgb.map(|v| v as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0, 0, (l * 100.0).round() as u32);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (
        (h.round() as u32) % 360,
        (s * 100.0).round() as u32,
        (l * 100.0).round() as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        let c = [30, 144, 255];
        assert_eq!(ColorFormat::Hex.format(c), "#1E90FF");
        assert_eq!(ColorFormat::Rgb.format(c), "rgb(30, 144, 255)");
        assert_eq!(ColorFormat::Hsl.format(c), "hsl(210, 100%, 56%)");
    }

    #[test]
    fn hsl_edge_cases() {
        assert_eq!(to_hsl([0, 0, 0]), (0, 0, 0));
        assert_eq!(to_hsl([255, 255, 255]), (0, 0, 100));
        assert_eq!(to_hsl([128, 128, 128]), (0, 0, 50));
        assert_eq!(to_hsl([255, 0, 0]), (0, 100, 50));
        assert_eq!(to_hsl([0, 255, 0]), (120, 100, 50));
        assert_eq!(to_hsl([0, 0, 255]), (240, 100, 50));
        assert_eq!(to_hsl([255, 0, 128]), (330, 100, 50));
        assert_eq!(to_hsl([255, 138, 61]), (24, 100, 62));
    }

    #[test]
    fn config_round_trip() {
        for f in ColorFormat::ALL {
            assert_eq!(ColorFormat::from_config(f.config_name()), f);
        }
        assert_eq!(ColorFormat::from_config("junk"), ColorFormat::Hex);
        assert_eq!(ColorFormat::from_config(" RGB "), ColorFormat::Rgb);
    }
}
