//! English strings for settings_ui.rs. Keys start with `settings.`.

pub fn get(key: &str) -> Option<&'static str> {
    Some(match key {
        "settings.language" => "Language",
        "settings.language.desc" => "Pseudo checks layout with longer, accented text.",
        "settings.language.system" => "System Default",
        "settings.language.en" => "English",
        "settings.language.pseudo" => "Pseudo",
        "settings.theme" => "Theme",
        "settings.theme.desc" => "Follow Windows or force dark or light.",
        _ => return None,
    })
}

/// Every key above, for the key test.
#[cfg(test)]
pub const KEYS: &[&str] = &[
    "settings.language",
    "settings.language.desc",
    "settings.language.system",
    "settings.language.en",
    "settings.language.pseudo",
    "settings.theme",
    "settings.theme.desc",
];
