//! UI string lookup (docs/notes/l10n.md). English is the base and the fallback; keys are
//! dotted and start with their group (`settings.`, `history.`, `overlay.`, `shell.`), and
//! each group's strings live in its own file under `l10n/<lang>/` so parallel edits never
//! touch the same file.
//!
//! `tr!("group.key")` gives a `&'static str`; `tr!("group.key", name = value)` fills
//! `{name}` placeholders and gives a `String`. Missing keys fall back to English, then to
//! the key itself (logged in debug builds) so a typo shows on screen instead of crashing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

mod en;

/// A UI language. Add a variant per translation (see docs/notes/l10n.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Lang {
    En = 0,
    /// Accented and ~35% longer English for spotting hard-coded strings and clipping.
    Pseudo = 1,
}

impl Lang {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Lang::Pseudo,
            _ => Lang::En,
        }
    }

    /// BCP 47 primary subtag a system locale must start with to pick this language.
    fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Pseudo => "pseudo",
        }
    }
}

/// Real translations a "system" setting can resolve to (Pseudo is never picked by locale).
const SHIPPED: &[Lang] = &[Lang::En];

/// `(config value, label key)` for the Language setting. "system" comes first.
pub const CHOICES: &[(&str, &str)] = &[
    ("system", "settings.language.system"),
    ("en", "settings.language.en"),
    ("pseudo", "settings.language.pseudo"),
];

static LANG: AtomicU8 = AtomicU8::new(Lang::En as u8);

/// Current language.
pub fn lang() -> Lang {
    Lang::from_u8(LANG.load(Ordering::Relaxed))
}

/// Applies the `general.language` config value ("system", "en", "pseudo", ...).
/// `STINKSHOT_PSEUDO=1` forces pseudo-localization regardless of the setting.
pub fn apply(setting: &str) {
    let l = resolve(setting, pseudo_env(), system_locale);
    LANG.store(l as u8, Ordering::Relaxed);
}

fn pseudo_env() -> bool {
    std::env::var("STINKSHOT_PSEUDO").is_ok_and(|v| v == "1")
}

fn resolve(setting: &str, pseudo_env: bool, locale: impl FnOnce() -> String) -> Lang {
    if pseudo_env || setting.eq_ignore_ascii_case("pseudo") {
        return Lang::Pseudo;
    }
    let want = if setting.is_empty() || setting.eq_ignore_ascii_case("system") {
        locale()
    } else {
        setting.to_string()
    };
    let want = want.to_ascii_lowercase();
    let primary = want.split(['-', '_']).next().unwrap_or("");
    SHIPPED
        .iter()
        .copied()
        .find(|l| l.code() == primary)
        .unwrap_or(Lang::En)
}

/// The user's Windows locale name, e.g. "en-US". Empty when unknown.
#[cfg(windows)]
fn system_locale() -> String {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    // LOCALE_NAME_MAX_LENGTH wide chars, as the API requires.
    let mut buf = [0u16; 85];
    // SAFETY: the buffer is valid for writes of its full length.
    let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
    if n <= 1 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..(n as usize - 1)])
}

#[cfg(not(windows))]
fn system_locale() -> String {
    std::env::var("LANG").unwrap_or_default()
}

/// English base string for `key`, dispatched by group prefix.
fn english(key: &str) -> Option<&'static str> {
    let (group, _) = key.split_once('.')?;
    match group {
        "settings" => en::settings::get(key),
        "history" => en::history::get(key),
        "overlay" => en::overlay::get(key),
        "shell" => en::shell::get(key),
        _ => None,
    }
}

/// Looks `key` up in the current language. Prefer the `tr!` macro.
pub fn lookup(key: &'static str) -> &'static str {
    let base = english(key).unwrap_or_else(|| {
        #[cfg(debug_assertions)]
        log::warn!("l10n: missing key {key}");
        key
    });
    match lang() {
        Lang::En => base,
        Lang::Pseudo => pseudo_cached(key, base),
    }
}

/// Fills `{name}` placeholders in a looked-up string.
// Reached through `tr!(key, name = value)`; unused until a converter needs placeholders.
#[allow(dead_code)]
pub fn fill(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

/// Pseudo strings are built once per key and leaked: the key set is fixed and small.
fn pseudo_cached(key: &'static str, base: &'static str) -> &'static str {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, &'static str>>> = OnceLock::new();
    let mut map = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.entry(key)
        .or_insert_with(|| Box::leak(pseudo(base).into_boxed_str()))
}

/// Accents ASCII letters, keeps `{placeholders}` intact, pads to ~135% length, brackets.
pub fn pseudo(s: &str) -> String {
    const FROM: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const TO: [char; 52] = [
        'á', 'ƀ', 'ç', 'đ', 'é', 'ƒ', 'ĝ', 'ĥ', 'í', 'ĵ', 'ķ', 'ĺ', 'ɱ', 'ñ', 'ó', 'þ', 'ǫ', 'ŕ',
        'š', 'ŧ', 'ú', 'ṽ', 'ŵ', 'ẋ', 'ý', 'ž', 'Á', 'Ɓ', 'Ç', 'Đ', 'É', 'Ƒ', 'Ĝ', 'Ĥ', 'Í', 'Ĵ',
        'Ķ', 'Ĺ', 'Ṁ', 'Ñ', 'Ó', 'Þ', 'Ǫ', 'Ŕ', 'Š', 'Ŧ', 'Ú', 'Ṽ', 'Ŵ', 'Ẋ', 'Ý', 'Ž',
    ];
    let mut out = String::with_capacity(s.len() * 2 + 4);
    out.push('[');
    let mut in_placeholder = false;
    let mut visible = 0usize;
    for c in s.chars() {
        if c == '{' {
            in_placeholder = true;
        }
        match FROM.find(c) {
            Some(i) if !in_placeholder => out.push(TO[i]),
            _ => out.push(c),
        }
        if !in_placeholder {
            visible += 1;
        }
        if c == '}' {
            in_placeholder = false;
        }
    }
    let pad = (visible * 35).div_ceil(100);
    out.push(' ');
    out.extend(std::iter::repeat_n('~', pad.saturating_sub(1)));
    out.push(']');
    out
}

/// `tr!("group.key")` -> `&'static str`; `tr!("group.key", n = 3)` -> `String` with `{n}`
/// filled. Keys live in `l10n/en/<group>.rs`.
macro_rules! tr {
    ($key:literal) => {
        $crate::l10n::lookup($key)
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::l10n::fill(
            $crate::l10n::lookup($key),
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+],
        )
    };
}
pub(crate) use tr;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_settings() {
        assert_eq!(resolve("system", false, || "en-US".into()), Lang::En);
        assert_eq!(resolve("system", false, || "xx-YY".into()), Lang::En);
        assert_eq!(resolve("", false, String::new), Lang::En);
        assert_eq!(resolve("pseudo", false, || "en-US".into()), Lang::Pseudo);
        assert_eq!(resolve("en", true, || "en-US".into()), Lang::Pseudo);
    }

    #[test]
    fn pseudo_is_longer_and_keeps_placeholders() {
        let src = "Press {key} to capture";
        let p = pseudo(src);
        assert!(p.contains("{key}"), "{p}");
        assert!(p.starts_with('[') && p.ends_with(']'));
        assert!(!p.contains("Press"));
        assert!(
            p.chars().count() as f32 >= src.chars().count() as f32 * 1.3,
            "{p}"
        );
    }

    #[test]
    fn english_lookup_and_fallback() {
        assert_eq!(english("settings.language"), Some("Language"));
        assert_eq!(english("nope.key"), None);
        assert_eq!(lookup("shell.does-not-exist"), "shell.does-not-exist");
        assert_eq!(tr!("history.empty.count", n = 7), "7 captures");
    }

    /// Every English key starts with its file's group, is unique, and resolves.
    #[test]
    fn keys_are_grouped_and_unique() {
        for (group, keys) in [
            ("settings", en::settings::KEYS),
            ("history", en::history::KEYS),
            ("overlay", en::overlay::KEYS),
            ("shell", en::shell::KEYS),
        ] {
            let mut seen = std::collections::HashSet::new();
            for k in keys {
                assert!(k.starts_with(&format!("{group}.")), "{k} not in {group}");
                assert!(seen.insert(*k), "duplicate key {k}");
                assert!(english(k).is_some(), "{k} has no English string");
            }
        }
        for (_, label) in CHOICES {
            assert!(english(label).is_some(), "{label}");
        }
    }
}
