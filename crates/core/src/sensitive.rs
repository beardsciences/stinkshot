//! Smart Redact detection: find sensitive text (email addresses, phone numbers, card
//! numbers, US Social Security numbers, IP addresses, API keys, tokens and passwords) in OCR
//! output and return image rects to redact. Pure and platform independent: the caller runs
//! OCR and passes words with their boxes. See docs/notes/smart-redact.md.
//!
//! Matching is deliberately simple and local (no network, no model): hand-written scanners
//! over each OCR line, joined from its words with single spaces. A match is mapped back to
//! every word it overlaps, so a box always covers whole words (over-redacting by a little is
//! safe, leaving a fragment visible is not).

use std::ops::Range;

use crate::geom::PxRect;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SensitiveKind {
    Email,
    Phone,
    Card,
    Ssn,
    IpAddress,
    Secret,
    Password,
}

impl SensitiveKind {
    /// Title Case label for the review list.
    pub fn label(self) -> &'static str {
        match self {
            SensitiveKind::Email => "Email Address",
            SensitiveKind::Phone => "Phone Number",
            SensitiveKind::Card => "Card Number",
            SensitiveKind::Ssn => "Social Security Number",
            SensitiveKind::IpAddress => "IP Address",
            SensitiveKind::Secret => "Key or Token",
            SensitiveKind::Password => "Password",
        }
    }
}

/// One OCR word and its box in image pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct TextWord {
    pub text: String,
    pub rect: PxRect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub kind: SensitiveKind,
    /// Masked text safe to show in the review list ("j\u{2022}\u{2022}\u{2022}@example.com").
    pub preview: String,
    /// Union of the matched words' boxes, image pixels.
    pub rect: PxRect,
}

const MASK: char = '\u{2022}';

/// Findings for OCR lines (each a list of words in reading order), top to bottom.
pub fn find_in_lines(lines: &[Vec<TextWord>]) -> Vec<Finding> {
    let mut out = Vec::new();
    for words in lines {
        // Join with single spaces, remembering each word's byte span.
        let mut line = String::new();
        let mut spans: Vec<Range<usize>> = Vec::with_capacity(words.len());
        for w in words {
            let t = w.text.trim();
            if t.is_empty() {
                spans.push(line.len()..line.len());
                continue;
            }
            if !line.is_empty() {
                line.push(' ');
            }
            let start = line.len();
            line.push_str(t);
            spans.push(start..line.len());
        }
        for (kind, range) in find_in_text(&line) {
            let rect = words
                .iter()
                .zip(&spans)
                .filter(|(_, s)| !s.is_empty() && s.start < range.end && range.start < s.end)
                .map(|(w, _)| w.rect)
                .filter(|r| r.w > 0 && r.h > 0)
                .reduce(union);
            if let Some(rect) = rect {
                out.push(Finding {
                    kind,
                    preview: mask(kind, &line[range]),
                    rect,
                });
            }
        }
    }
    out
}

fn union(a: PxRect, b: PxRect) -> PxRect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.w).max(b.x + b.w);
    let y1 = (a.y + a.h).max(b.y + b.h);
    PxRect::new(x0, y0, x1 - x0, y1 - y0)
}

/// Sensitive spans (byte ranges) in one line of text, in text order, never overlapping.
pub fn find_in_text(s: &str) -> Vec<(SensitiveKind, Range<usize>)> {
    // Higher-priority detectors claim their spans first.
    let mut found: Vec<(SensitiveKind, Range<usize>)> = Vec::new();
    let mut claim = |kind: SensitiveKind, r: Range<usize>| {
        if r.is_empty()
            || found
                .iter()
                .any(|(_, f)| f.start < r.end && r.start < f.end)
        {
            return;
        }
        found.push((kind, r));
    };
    let toks = tokens(s);
    for t in &toks {
        if is_prefixed_secret(&s[t.clone()]) {
            claim(SensitiveKind::Secret, t.clone());
        }
    }
    for t in &toks {
        if is_email(&s[t.clone()]) {
            claim(SensitiveKind::Email, t.clone());
        }
    }
    for (kind, r) in numbers(s) {
        claim(kind, r);
    }
    for t in &toks {
        if is_ipv4(&s[t.clone()]) {
            claim(SensitiveKind::IpAddress, t.clone());
        }
    }
    for t in &toks {
        if is_generic_secret(&s[t.clone()]) {
            claim(SensitiveKind::Secret, t.clone());
        }
    }
    for r in password_values(s, &toks) {
        claim(SensitiveKind::Password, r);
    }
    found.sort_by_key(|(_, r)| r.start);
    found
}

/// Whitespace-separated tokens with surrounding punctuation trimmed.
fn tokens(s: &str) -> Vec<Range<usize>> {
    const TRIM: &[char] = &[
        '(', ')', '[', ']', '{', '}', '<', '>', ',', ';', '"', '\'', '`', '\u{201c}', '\u{201d}',
        '\u{2018}', '\u{2019}',
    ];
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices().chain(std::iter::once((s.len(), ' '))) {
        if c.is_whitespace() {
            if let Some(st) = start.take() {
                let raw = &s[st..i];
                let lead = raw.len() - raw.trim_start_matches(TRIM).len();
                let mut t = raw.trim_matches(TRIM);
                // A sentence-ending period or colon is not part of an address or key.
                t = t.trim_end_matches(['.', ':', '!', '?']);
                if !t.is_empty() {
                    out.push(st + lead..st + lead + t.len());
                }
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    out
}

fn is_email(t: &str) -> bool {
    let Some((local, domain)) = t.split_once('@') else {
        return false;
    };
    if local.is_empty() || domain.contains('@') {
        return false;
    }
    let local_ok = local
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._%+-".contains(c));
    let labels: Vec<&str> = domain.split('.').collect();
    let tld = labels.last().copied().unwrap_or("");
    local_ok
        && labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !l.starts_with('-')
        })
        && tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
}

fn is_ipv4(t: &str) -> bool {
    // Optional :port.
    let host = match t.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => t,
    };
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u16>().is_ok_and(|v| v <= 255)
        })
}

/// Well-known key and token formats.
fn is_prefixed_secret(t: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "sk-",
        "sk_live_",
        "sk_test_",
        "rk_live_",
        "pk_live_",
        "ghp_",
        "gho_",
        "ghs_",
        "ghu_",
        "github_pat_",
        "glpat-",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "AKIA",
        "ASIA",
        "AIza",
        "hf_",
        "npm_",
    ];
    let key_chars = |s: &str| {
        s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
    };
    if PREFIXES
        .iter()
        .any(|p| t.starts_with(p) && t.len() >= p.len() + 12)
        && key_chars(t)
    {
        return true;
    }
    // JSON Web Token: three base64url parts, the first a JSON header.
    let parts: Vec<&str> = t.split('.').collect();
    parts.len() == 3 && t.starts_with("eyJ") && parts.iter().all(|p| p.len() >= 8 && key_chars(p))
}

/// Long random-looking tokens (hex digests excluded only when short).
fn is_generic_secret(t: &str) -> bool {
    if t.len() < 32 || t.len() > 256 {
        return false;
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_+/=".contains(c))
    {
        return false;
    }
    let digits = t.bytes().filter(u8::is_ascii_digit).count();
    let letters = t.bytes().filter(u8::is_ascii_alphabetic).count();
    digits >= 2 && letters >= 8 && entropy(t) >= 3.5
}

/// Shannon entropy in bits per character.
fn entropy(t: &str) -> f32 {
    let mut counts = [0u32; 256];
    for b in t.bytes() {
        counts[b as usize] += 1;
    }
    let n = t.len() as f32;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f32 / n;
            -p * p.log2()
        })
        .sum()
}

/// Values after a "password:"-style label ("Password: hunter2", "pwd=abc").
fn password_values(s: &str, toks: &[Range<usize>]) -> Vec<Range<usize>> {
    const KEYS: &[&str] = &[
        "password", "passwd", "pwd", "passcode", "pin", "secret", "token", "apikey", "api_key",
        "api-key",
    ];
    let key_of = |t: &str| {
        let k = t.trim_end_matches([':', '=']).to_ascii_lowercase();
        KEYS.contains(&k.as_str())
    };
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let text = &s[t.clone()];
        // "password=abc" in one token.
        if let Some((k, v)) = text.split_once(['=', ':'])
            && !v.is_empty()
            && key_of(k)
        {
            out.push(t.end - v.len()..t.end);
            continue;
        }
        // "Password: abc": the label ends with ':' or '=' in the raw text (tokens drop it).
        let raw_end = s[t.end..].chars().next();
        let labelled = key_of(text) && matches!(raw_end, Some(':') | Some('='));
        if labelled && let Some(next) = toks.get(i + 1) {
            let v = &s[next.clone()];
            if v.chars().count() >= 3 && !v.chars().all(|c| MASK == c || c == '*') {
                out.push(next.clone());
            }
        }
    }
    out
}

/// Maximal runs of digits and number separators ("+1 (555) 123-4567", "4242 4242 4242 4242").
fn digit_runs(s: &str) -> Vec<Range<usize>> {
    let ok = |c: char| c.is_ascii_digit() || " -().+".contains(c);
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let push = |out: &mut Vec<Range<usize>>, st: usize, end: usize| {
        let run = &s[st..end];
        // Trim to the first digit (keeping a leading '+' or '(') and the last digit.
        let Some(first) = run.find(|c: char| c.is_ascii_digit()) else {
            return;
        };
        let Some(last) = run.rfind(|c: char| c.is_ascii_digit()) else {
            return;
        };
        let mut a = first;
        if let Some(prev) = run[..first].chars().last()
            && (prev == '+' || prev == '(')
        {
            a -= 1;
            if prev == '(' && run[..a].ends_with('+') {
                a -= 1;
            }
        }
        out.push(st + a..st + last + 1);
    };
    for (i, c) in s.char_indices() {
        // A run must not start or end inside a word ("v2", "ID123x").
        let prev_alnum = s[..i].chars().last().is_some_and(|p| p.is_alphanumeric());
        if ok(c) {
            if start.is_none() && !(prev_alnum && c.is_ascii_digit()) {
                start = Some(i);
            }
        } else if let Some(st) = start.take() {
            // Digits running straight into a letter are part of a word ("4567x").
            let glued = c.is_alphanumeric() && s[..i].ends_with(|p: char| p.is_ascii_digit());
            if !glued {
                push(&mut out, st, i);
            }
        }
    }
    if let Some(st) = start {
        push(&mut out, st, s.len());
    }
    out
}

/// Numbers in `s`: each digit run is tried whole, then as shorter windows of its
/// space-separated chunks (longest first), so "555-123-4567 10.0.0.1" still yields the phone.
fn numbers(s: &str) -> Vec<(SensitiveKind, Range<usize>)> {
    let mut out: Vec<(SensitiveKind, Range<usize>)> = Vec::new();
    for run in digit_runs(s) {
        let mut chunks: Vec<Range<usize>> = Vec::new();
        let mut at = run.start;
        for part in s[run.clone()].split(' ') {
            if !part.is_empty() {
                chunks.push(at..at + part.len());
            }
            at += part.len() + 1;
        }
        let n = chunks.len();
        let mut taken: Vec<Range<usize>> = Vec::new();
        for len in (1..=n).rev() {
            for first in 0..=n - len {
                let r = chunks[first].start..chunks[first + len - 1].end;
                if taken.iter().any(|t| t.start < r.end && r.start < t.end) {
                    continue;
                }
                if let Some(kind) = classify_digits(&s[r.clone()]) {
                    taken.push(r.clone());
                    out.push((kind, r));
                }
            }
        }
    }
    out
}

fn classify_digits(run: &str) -> Option<SensitiveKind> {
    if !run.starts_with(|c: char| c.is_ascii_digit() || c == '+' || c == '(')
        || !run.ends_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    let digits: String = run.chars().filter(char::is_ascii_digit).collect();
    let n = digits.len();
    let has_sep = run.chars().any(|c| !c.is_ascii_digit());
    if run.contains('.') && !run.contains([' ', '-']) {
        // Dotted digits are an IP, a version or a decimal unless shaped 555.123.4567.
        let groups: Vec<usize> = run.split('.').map(str::len).collect();
        return (groups == [3, 3, 4] && run.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
            .then_some(SensitiveKind::Phone);
    }
    if is_ssn(run) {
        return Some(SensitiveKind::Ssn);
    }
    if (13..=19).contains(&n) && luhn(&digits) && card_shaped(run) {
        return Some(SensitiveKind::Card);
    }
    let plus = run.starts_with('+');
    if (10..=15).contains(&n) && (has_sep || plus) && phone_shaped(run) {
        return Some(SensitiveKind::Phone);
    }
    None
}

/// "123-45-6789" with valid area, group and serial numbers.
fn is_ssn(run: &str) -> bool {
    let b = run.as_bytes();
    if b.len() != 11 || b[3] != b'-' || b[6] != b'-' {
        return false;
    }
    let parts = [&run[0..3], &run[4..6], &run[7..11]];
    if !parts.iter().all(|p| p.bytes().all(|c| c.is_ascii_digit())) {
        return false;
    }
    let area: u32 = parts[0].parse().unwrap_or(0);
    area != 0 && area != 666 && area < 900 && parts[1] != "00" && parts[2] != "0000"
}

/// Card numbers are written plain or in groups of digits separated by single spaces/dashes.
fn card_shaped(run: &str) -> bool {
    run.bytes().all(|c| c.is_ascii_digit())
        || (!run.contains(['(', ')', '+', '.'])
            && run
                .split([' ', '-'])
                .all(|g| !g.is_empty() && g.len() <= 6 && g.bytes().all(|c| c.is_ascii_digit())))
}

/// Phone numbers: digit groups of 1-4 digits (the subscriber group up to 4), not one long
/// block with a stray separator, and not a date such as 2026-11-07.
fn phone_shaped(run: &str) -> bool {
    let groups: Vec<&str> = run
        .split(|c: char| !c.is_ascii_digit())
        .filter(|g| !g.is_empty())
        .collect();
    if groups.len() == 1 {
        // "+15551234567"
        return run.starts_with('+');
    }
    // Several four-digit groups read as a card or account number, not a phone.
    groups.iter().all(|g| g.len() <= 5)
        && groups.iter().filter(|g| g.len() == 4).count() <= 1
        && groups.last().is_some_and(|g| g.len() >= 3)
}

fn luhn(digits: &str) -> bool {
    let mut sum = 0;
    for (i, b) in digits.bytes().rev().enumerate() {
        let mut d = (b - b'0') as u32;
        if i % 2 == 1 {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
    }
    sum % 10 == 0
}

/// A preview that says what was found without showing it.
fn mask(kind: SensitiveKind, text: &str) -> String {
    let m = |n: usize| MASK.to_string().repeat(n);
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    let tail = |n: usize| digits[digits.len().saturating_sub(n)..].to_string();
    match kind {
        SensitiveKind::Email => match text.split_once('@') {
            Some((local, domain)) => {
                let first: String = local.chars().take(1).collect();
                format!("{first}{}@{domain}", m(3))
            }
            None => m(6),
        },
        SensitiveKind::Card => format!("{} {}", m(4), tail(4)),
        SensitiveKind::Phone => format!("{} {}", m(3), tail(4)),
        SensitiveKind::Ssn => format!("{}-{}-{}", m(3), m(2), tail(4)),
        SensitiveKind::IpAddress => {
            let parts: Vec<&str> = text.split('.').collect();
            match parts.as_slice() {
                [a, b, ..] => format!("{a}.{b}.{}.{}", m(1), m(1)),
                _ => m(6),
            }
        }
        SensitiveKind::Secret => {
            let head: String = text.chars().take(4).collect();
            format!("{head}{}", m(6))
        }
        SensitiveKind::Password => m(8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<(SensitiveKind, &str)> {
        find_in_text(s)
            .into_iter()
            .map(|(k, r)| (k, &s[r]))
            .collect()
    }

    #[test]
    fn emails() {
        assert_eq!(
            kinds("Contact: jane.doe+news@example.co.uk, thanks."),
            [(SensitiveKind::Email, "jane.doe+news@example.co.uk")]
        );
        assert!(kinds("@handle and a@b and foo@bar.c1").is_empty());
    }

    #[test]
    fn cards_need_luhn() {
        assert_eq!(
            kinds("Card 4242 4242 4242 4242 exp 12/30"),
            [(SensitiveKind::Card, "4242 4242 4242 4242")]
        );
        assert_eq!(
            kinds("5555-5555-5555-4444"),
            [(SensitiveKind::Card, "5555-5555-5555-4444")]
        );
        // Fails Luhn: not a card (and too long for a phone).
        assert!(kinds("4242 4242 4242 4241").is_empty());
    }

    #[test]
    fn phones() {
        assert_eq!(
            kinds("Call +1 (555) 123-4567 today"),
            [(SensitiveKind::Phone, "+1 (555) 123-4567")]
        );
        assert_eq!(
            kinds("tel 555.123.4567"),
            [(SensitiveKind::Phone, "555.123.4567")]
        );
        assert_eq!(
            kinds("+447911123456"),
            [(SensitiveKind::Phone, "+447911123456")]
        );
        // Dates, times, versions, plain counts and IDs glued to letters are not phones.
        for s in [
            "2026-11-07",
            "12:30:45",
            "v1.2.3",
            "1,234,567",
            "Order ID123456789012",
            "Total 1234567890",
        ] {
            assert!(kinds(s).is_empty(), "{s}: {:?}", kinds(s));
        }
    }

    #[test]
    fn ssn() {
        assert_eq!(
            kinds("SSN 123-45-6789"),
            [(SensitiveKind::Ssn, "123-45-6789")]
        );
        assert!(
            !kinds("000-12-3456")
                .iter()
                .any(|(k, _)| *k == SensitiveKind::Ssn)
        );
    }

    #[test]
    fn ip_addresses() {
        assert_eq!(
            kinds("server 192.168.12.201:8032 up"),
            [(SensitiveKind::IpAddress, "192.168.12.201:8032")]
        );
        assert!(kinds("10.0.300.1").is_empty());
    }

    #[test]
    fn secrets() {
        let gh = "ghp_aB3dE5fG7hI9jK1lM3nO5pQ7rS9tU1vW3xY5";
        assert_eq!(kinds(gh), [(SensitiveKind::Secret, gh)]);
        assert_eq!(
            kinds("key AKIAIOSFODNN7EXAMPLE"),
            [(SensitiveKind::Secret, "AKIAIOSFODNN7EXAMPLE")]
        );
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NSJ9.dBjftJeZ4CVPmB92K27uhbUJU1p1r";
        assert_eq!(kinds(jwt), [(SensitiveKind::Secret, jwt)]);
        let random = "Zx9Qm2Lp8Rt4Vb7Nc1Kd6Hs3Jf5Gw0Ye";
        assert_eq!(kinds(random), [(SensitiveKind::Secret, random)]);
        // Long ordinary words and repeated characters are not secrets.
        assert!(kinds("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1111").is_empty());
        assert!(kinds("internationalization_configuration_settings").is_empty());
    }

    #[test]
    fn passwords() {
        assert_eq!(
            kinds("Password: hunter22"),
            [(SensitiveKind::Password, "hunter22")]
        );
        assert_eq!(kinds("pwd=s3cret!"), [(SensitiveKind::Password, "s3cret")]);
        // Already masked, or no label.
        assert!(kinds("Password: \u{2022}\u{2022}\u{2022}\u{2022}").is_empty());
        assert!(kinds("Reset your password today").is_empty());
    }

    #[test]
    fn several_in_one_line_in_order_without_overlap() {
        let s = "jane@example.com 555-123-4567 10.0.0.1";
        let k: Vec<SensitiveKind> = find_in_text(s).into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            k,
            [
                SensitiveKind::Email,
                SensitiveKind::Phone,
                SensitiveKind::IpAddress
            ]
        );
    }

    #[test]
    fn findings_cover_whole_words_and_mask_the_text() {
        let w = |t: &str, x: i32| TextWord {
            text: t.into(),
            rect: PxRect::new(x, 10, 40, 12),
        };
        let lines = vec![
            vec![w("Card", 0), w("4242", 50), w("4242", 100)],
            vec![w("4242", 0), w("4242", 50)],
            vec![w("Mail:", 0), w("jane@example.com", 60)],
        ];
        let f = find_in_lines(&lines);
        // The card is split over two OCR lines: neither half is a valid number on its own.
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, SensitiveKind::Email);
        assert_eq!(f[0].rect, PxRect::new(60, 10, 40, 12));
        assert_eq!(f[0].preview, "j\u{2022}\u{2022}\u{2022}@example.com");

        let one = vec![vec![
            w("Card", 0),
            w("4242", 50),
            w("4242", 100),
            w("4242", 150),
            w("4242", 200),
        ]];
        let f = find_in_lines(&one);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, SensitiveKind::Card);
        assert_eq!(f[0].rect, PxRect::new(50, 10, 190, 12));
        assert_eq!(f[0].preview, "\u{2022}\u{2022}\u{2022}\u{2022} 4242");
        assert!(!f[0].preview.contains("4242 4242"));
    }

    #[test]
    fn masks_never_show_the_whole_value() {
        assert_eq!(
            mask(SensitiveKind::Ssn, "123-45-6789"),
            "\u{2022}\u{2022}\u{2022}-\u{2022}\u{2022}-6789"
        );
        assert_eq!(
            mask(SensitiveKind::IpAddress, "192.168.12.201"),
            "192.168.\u{2022}.\u{2022}"
        );
        assert!(!mask(SensitiveKind::Password, "hunter22").contains("hunter"));
        assert!(luhn("4242424242424242"));
        assert!(entropy("aaaa") < 0.01);
    }
}
