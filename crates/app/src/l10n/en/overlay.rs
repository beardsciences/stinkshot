//! English strings for overlay/**. Keys start with `overlay.`.

pub fn get(key: &str) -> Option<&'static str> {
    Some(match key {
        "overlay.close" => "Close (Esc)",
        "overlay.color" => "Color (1-8)",
        "overlay.stroke_width" => "Stroke Width ([ and ])",
        _ => return None,
    })
}

/// Every key above, for the key test.
#[cfg(test)]
pub const KEYS: &[&str] = &["overlay.close", "overlay.color", "overlay.stroke_width"];
