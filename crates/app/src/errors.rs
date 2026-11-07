//! Plain-language failure text. Toast bodies and inline error lines say what happened and
//! what to do in one or two sentences; the raw error chain (paths, HRESULTs, OS messages)
//! goes to the log only. Use `report` / `report_str` at every user-facing failure site.

/// Appended once, at the end of the body, when the cause is not one we recognise. Settings >
/// About has the Open Log button it points to.
pub const LOG_HINT: &str = "Details are in the log (Settings > About > Open Log).";

/// Failure causes common enough to explain in plain words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    DiskFull,
    AccessDenied,
    PathTooLong,
    NotFound,
    InUse,
    NetworkPath,
    ClipboardBusy,
    Offline,
    Unknown,
}

impl Cause {
    /// What happened and what to do, or `None` when the caller's fallback should be used.
    pub fn sentence(self) -> Option<&'static str> {
        Some(match self {
            Cause::DiskFull => "The disk is full. Free up some space, then try again.",
            Cause::AccessDenied => {
                "Windows blocked access to the file or folder. Check that it is not read-only or protected, or choose another folder."
            }
            Cause::PathTooLong => {
                "The file path is too long. Choose a shorter folder or file name pattern."
            }
            Cause::NotFound => {
                "The file or folder could not be found. It may have been moved or deleted."
            }
            Cause::InUse => "Another program is using the file. Close it, then try again.",
            Cause::NetworkPath => {
                "The network location is not reachable. Check the connection, then try again."
            }
            Cause::ClipboardBusy => "Another app is using the clipboard. Try again in a moment.",
            Cause::Offline => {
                "Stinkshot could not reach the server. Check your internet connection and try again later."
            }
            Cause::Unknown => return None,
        })
    }
}

/// Map a Win32 error code (also the low word of a `0x8007xxxx` HRESULT) to a cause.
fn cause_of_code(code: i32) -> Cause {
    match code {
        39 | 112 => Cause::DiskFull,
        5 => Cause::AccessDenied,
        111 | 206 => Cause::PathTooLong,
        2 | 3 => Cause::NotFound,
        32 | 33 => Cause::InUse,
        53 | 64 | 67 | 1231 => Cause::NetworkPath,
        _ => Cause::Unknown,
    }
}

/// Classify from the text of an error chain (OS messages and HRESULTs as Windows prints them).
/// Never returns `Cause::Offline`: local errors also say "timed out" (capture, recorder start),
/// so only the update check, the one network call, uses `classify_network_text`.
pub fn classify_text(chain: &str) -> Cause {
    let t = chain.to_ascii_lowercase();
    if t.contains("openclipboard") || t.contains("clipboard is busy") {
        return Cause::ClipboardBusy;
    }
    // HRESULT_FROM_WIN32 codes, as `windows::core::Error` prints them.
    if let Some(i) = t.find("0x8007") {
        let hex: String = t[i + 6..]
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .take(4)
            .collect();
        if let Ok(code) = i32::from_str_radix(&hex, 16) {
            let c = cause_of_code(code);
            if c != Cause::Unknown {
                return c;
            }
        }
    }
    if let Some(i) = t.find("os error ") {
        let digits: String = t[i + 9..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(code) = digits.parse::<i32>() {
            let c = cause_of_code(code);
            if c != Cause::Unknown {
                return c;
            }
        }
    }
    const PHRASES: [(&str, Cause); 10] = [
        ("not enough space", Cause::DiskFull),
        ("disk is full", Cause::DiskFull),
        ("no space left", Cause::DiskFull),
        ("access is denied", Cause::AccessDenied),
        ("permission denied", Cause::AccessDenied),
        ("filename or extension is too long", Cause::PathTooLong),
        ("cannot find the file", Cause::NotFound),
        ("cannot find the path", Cause::NotFound),
        ("being used by another process", Cause::InUse),
        ("network path was not found", Cause::NetworkPath),
    ];
    for (phrase, cause) in PHRASES {
        if t.contains(phrase) {
            return cause;
        }
    }
    Cause::Unknown
}

/// `classify_text` for errors of a network request (the update check): connection and
/// lookup failures and timeouts also read as `Cause::Offline`.
pub fn classify_network_text(chain: &str) -> Cause {
    let c = classify_text(chain);
    if c != Cause::Unknown {
        return c;
    }
    let t = chain.to_ascii_lowercase();
    const OFFLINE: [&str; 6] = [
        "dns failed",
        "connection refused",
        "timed out",
        "failed to lookup",
        "connection reset",
        "no such host",
    ];
    if OFFLINE.iter().any(|p| t.contains(p)) {
        Cause::Offline
    } else {
        Cause::Unknown
    }
}

/// Classify an error: OS codes of `io::Error`s in the chain first, then the chain's text.
pub fn classify(e: &anyhow::Error) -> Cause {
    for cause in e.chain() {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            if let Some(code) = io.raw_os_error() {
                let c = cause_of_code(code);
                if c != Cause::Unknown {
                    return c;
                }
            }
            match io.kind() {
                std::io::ErrorKind::PermissionDenied => return Cause::AccessDenied,
                std::io::ErrorKind::NotFound => return Cause::NotFound,
                std::io::ErrorKind::StorageFull => return Cause::DiskFull,
                _ => {}
            }
        }
    }
    classify_text(&format!("{e:#}"))
}

/// The plain sentence for `cause`, or `fallback`; then `tail` (if any); then, only when the
/// cause is unknown, the log hint, so it always ends the body.
fn compose(cause: Cause, fallback: &str, tail: &str) -> String {
    let mut out = cause.sentence().unwrap_or(fallback).to_string();
    if !tail.is_empty() {
        out.push(' ');
        out.push_str(tail);
    }
    if cause.sentence().is_none() {
        out.push(' ');
        out.push_str(LOG_HINT);
    }
    out
}

fn sentence_or(cause: Cause, fallback: &str) -> String {
    compose(cause, fallback, "")
}

/// Plain text for `e` without logging (callers that already logged).
pub fn explain(e: &anyhow::Error, fallback: &str) -> String {
    sentence_or(classify(e), fallback)
}

/// Plain text for an error that crossed a thread as a string.
pub fn explain_str(e: &str, fallback: &str) -> String {
    sentence_or(classify_text(e), fallback)
}

/// Log the full chain under `what` and return the plain sentence for the user.
pub fn report(what: &str, e: &anyhow::Error, fallback: &str) -> String {
    log::warn!("{what}: {e:#}");
    explain(e, fallback)
}

/// `report` for string errors.
pub fn report_str(what: &str, e: &str, fallback: &str) -> String {
    log::warn!("{what}: {e}");
    explain_str(e, fallback)
}

/// `report` with a closing sentence (what Stinkshot did about it) placed before the log hint.
pub fn report_with(what: &str, e: &anyhow::Error, fallback: &str, tail: &str) -> String {
    log::warn!("{what}: {e:#}");
    compose(classify(e), fallback, tail)
}

/// `report_with` for string errors.
pub fn report_str_with(what: &str, e: &str, fallback: &str, tail: &str) -> String {
    log::warn!("{what}: {e}");
    compose(classify_text(e), fallback, tail)
}

/// `report_str` for errors of a network request: offline causes are recognised.
pub fn report_network_str(what: &str, e: &str, fallback: &str) -> String {
    log::warn!("{what}: {e}");
    sentence_or(classify_network_text(e), fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_codes_map_to_causes() {
        let e = anyhow::Error::new(std::io::Error::from_raw_os_error(112)).context("write x");
        assert_eq!(classify(&e), Cause::DiskFull);
        let e = anyhow::Error::new(std::io::Error::from_raw_os_error(5));
        assert_eq!(classify(&e), Cause::AccessDenied);
        let e = anyhow::Error::new(std::io::Error::from_raw_os_error(206));
        assert_eq!(classify(&e), Cause::PathTooLong);
        let e = anyhow::Error::new(std::io::Error::from_raw_os_error(32));
        assert_eq!(classify(&e), Cause::InUse);
        let e = anyhow::Error::new(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        assert_eq!(classify(&e), Cause::NotFound);
    }

    #[test]
    fn hresults_and_messages_map_to_causes() {
        assert_eq!(
            classify_text("OpenClipboard failed: Access is denied. (0x80070005)"),
            Cause::ClipboardBusy
        );
        assert_eq!(
            classify_text("encode: There is not enough space on the disk. (0x80070070)"),
            Cause::DiskFull
        );
        assert_eq!(classify_text("x (0x800700CE)"), Cause::PathTooLong);
        assert_eq!(
            classify_text("open C:\\a: Access is denied. (os error 5)"),
            Cause::AccessDenied
        );
        assert_eq!(
            classify_network_text("https://code.example: Dns Failed: resolve"),
            Cause::Offline
        );
        assert_eq!(
            classify_network_text("io: connection timed out"),
            Cause::Offline
        );
        assert_eq!(classify_text("decoder returned garbage"), Cause::Unknown);
    }

    #[test]
    fn local_timeouts_are_not_offline() {
        for e in [
            "recording initialisation timed out",
            "timed out waiting for a window frame (is the window minimized?)",
            "Windows.Graphics.Capture timed out",
            "audio initialisation timed out",
            "https://code.example: Dns Failed: resolve",
            "connection reset by peer",
        ] {
            assert_eq!(classify_text(e), Cause::Unknown, "{e}");
        }
        let s = explain_str("recording initialisation timed out", "Fallback.");
        assert!(s.starts_with("Fallback."));
        assert!(!s.contains("internet"));
    }

    #[test]
    fn log_hint_ends_combined_bodies() {
        let e = anyhow::anyhow!("weird");
        let s = report_with("t", &e, "It failed.", "Kept what was recorded.");
        assert_eq!(s, format!("It failed. Kept what was recorded. {LOG_HINT}"));
        let e = anyhow::Error::new(std::io::Error::from_raw_os_error(112));
        let s = report_with("t", &e, "It failed.", "Kept what was recorded.");
        assert!(s.starts_with("The disk is full."));
        assert!(s.ends_with("Kept what was recorded."));
        let s = report_str_with("t", "weird", "It failed.", "");
        assert_eq!(s, format!("It failed. {LOG_HINT}"));
    }

    #[test]
    fn messages_never_leak_the_chain() {
        let e = anyhow::anyhow!("weird failure 0xC00D36B4 in C:\\Users\\x\\file.mp4");
        let s = explain(&e, "The recording could not be saved.");
        assert!(s.starts_with("The recording could not be saved."));
        assert!(s.ends_with(LOG_HINT));
        assert!(!s.contains("0xC00D"));
        assert!(!s.contains("C:\\"));
        let s = explain_str("Access is denied. (os error 5) C:\\x", "Fallback.");
        assert!(!s.contains("C:\\"));
        assert!(s.contains("blocked access"));
    }

    #[test]
    fn every_known_cause_has_a_sentence_with_advice() {
        for c in [
            Cause::DiskFull,
            Cause::AccessDenied,
            Cause::PathTooLong,
            Cause::NotFound,
            Cause::InUse,
            Cause::NetworkPath,
            Cause::ClipboardBusy,
            Cause::Offline,
        ] {
            let s = c.sentence().unwrap();
            assert!(s.ends_with('.'), "{s}");
            assert!(s.split(". ").count() >= 2, "needs what + what to do: {s}");
        }
        assert!(Cause::Unknown.sentence().is_none());
    }
}
