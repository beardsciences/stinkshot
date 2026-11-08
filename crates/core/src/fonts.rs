//! Fonts for rasterizing annotation text. Loaded from the Windows font folder at runtime
//! (Segoe UI Variable -> Segoe UI -> Arial), never bundled. Tests may use `from_bytes`.
//!
//! The font files are memory-mapped, not read: mapped pages belong to the file and are not
//! private memory, and the same bytes serve ab_glyph (annotation text) and egui (the UI) with
//! no copy. Reading them cost about 8 MB of private bytes (Segoe UI Variable alone is 1.8 MB
//! and was held three times; measured Nov 8 2026, docs/notes/app-shell.md).
//!
//! Characters the primary face lacks (CJK, Hangul, emoji, symbols) fall back per character
//! to system faces: Microsoft YaHei, Yu Gothic, Malgun Gothic, Microsoft JhengHei, Segoe UI
//! Emoji, Segoe UI Symbol (whichever are installed). Fallback files are mapped only when a
//! character first needs them (2-20 MB each) and stay loaded for the process, shared with
//! egui through `Fonts::fallback_for_char` so the editor UI and the render agree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ab_glyph::{Font, FontArc, FontRef, GlyphId, OutlinedGlyph, PxScale, ScaleFont, point};
use anyhow::Context;

/// Line height as a multiple of the font size.
pub const LINE_HEIGHT: f32 = 1.25;

#[derive(Clone)]
pub struct Fonts {
    /// Regular weight for text annotations.
    pub regular: Arc<ab_glyph::FontArc>,
    /// Semibold/bold for step numbers.
    pub bold: Arc<ab_glyph::FontArc>,
    /// Raw bytes of `regular`, so the UI can register the same face with egui
    /// (`egui::FontData::from_static`). Mapped or leaked once; they live for the process.
    pub regular_bytes: &'static [u8],
    pub bold_bytes: &'static [u8],
    /// Per-character fallback faces (empty for `from_bytes`).
    fallback: Arc<FallbackSet>,
}

/// Map a font file for the rest of the process (falls back to reading it). None when the
/// file is missing, empty or not a font ab_glyph can parse.
pub fn map_font_file(path: &std::path::Path) -> Option<&'static [u8]> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            log::debug!("font {} not available: {e}", path.display());
            return None;
        }
    };
    // SAFETY: the mapping is read-only and never unmapped. System font files are not
    // rewritten in place while mapped (Windows replaces fonts by rename on update).
    let bytes: &'static [u8] = match unsafe { memmap2::Mmap::map(&file) } {
        Ok(m) => {
            let m: &'static memmap2::Mmap = Box::leak(Box::new(m));
            &m[..]
        }
        Err(e) => {
            log::debug!("font {} not mappable ({e}); reading it", path.display());
            let v = std::fs::read(path).ok()?;
            Box::leak(v.into_boxed_slice())
        }
    };
    if bytes.is_empty() || ab_glyph::FontRef::try_from_slice(bytes).is_err() {
        log::warn!("font {} could not be parsed", path.display());
        return None;
    }
    Some(bytes)
}

/// A fallback face for egui: font file bytes (kept for the whole process) and face index.
#[derive(Clone, Copy, Debug)]
pub struct FallbackFont {
    /// Stable name (egui font key).
    pub name: &'static str,
    pub data: &'static [u8],
    pub index: u32,
}

/// One fallback family: regular and bold file candidates, loaded on first use.
struct FallbackFace {
    name: &'static str,
    regular: &'static [&'static str],
    bold: &'static [&'static str],
    loaded: [OnceLock<Option<Loaded>>; 2],
}

struct Loaded {
    font: FontArc,
    data: &'static [u8],
}

#[derive(Default)]
struct FallbackSet {
    dir: Option<PathBuf>,
    faces: Vec<FallbackFace>,
}

type FallbackEntry = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

/// Fallback families in default order (name, regular files, bold files).
const FALLBACKS: [FallbackEntry; 6] = [
    ("Microsoft YaHei", &["msyh.ttc"], &["msyhbd.ttc"]),
    (
        "Yu Gothic",
        &["YuGothR.ttc", "YuGothM.ttc"],
        &["YuGothB.ttc"],
    ),
    ("Malgun Gothic", &["malgun.ttf"], &["malgunbd.ttf"]),
    ("Microsoft JhengHei", &["msjh.ttc"], &["msjhbd.ttc"]),
    ("Segoe UI Emoji", &["seguiemj.ttf"], &[]),
    ("Segoe UI Symbol", &["seguisym.ttf"], &[]),
];
/// Indexes into `FALLBACKS`.
const YU_GOTHIC: usize = 1;
const MALGUN: usize = 2;

type FileCache = Mutex<HashMap<PathBuf, Option<&'static [u8]>>>;

/// Font files mapped so far, shared by every `Fonts` in the process (each file is mapped once).
fn file_cache() -> &'static FileCache {
    static CACHE: OnceLock<FileCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn read_cached(path: &Path) -> Option<&'static [u8]> {
    let mut cache = file_cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = cache.get(path) {
        return *v;
    }
    // Mapped for the life of the process: the renderer and egui both borrow it.
    let v = map_font_file(path);
    if v.is_some() {
        log::info!("fallback font loaded: {}", path.display());
    }
    cache.insert(path.to_path_buf(), v);
    v
}

impl FallbackSet {
    fn system(dir: PathBuf) -> FallbackSet {
        FallbackSet {
            dir: Some(dir),
            faces: FALLBACKS
                .iter()
                .map(|(name, regular, bold)| FallbackFace {
                    name,
                    regular,
                    bold,
                    loaded: [OnceLock::new(), OnceLock::new()],
                })
                .collect(),
        }
    }

    fn face(&self, i: usize, bold: bool) -> Option<&Loaded> {
        let f = self.faces.get(i)?;
        let dir = self.dir.as_ref()?;
        let load = |files: &[&str]| {
            files.iter().find_map(|name| {
                let data = read_cached(&dir.join(name))?;
                let font = FontRef::try_from_slice(data).ok()?;
                Some(Loaded {
                    font: FontArc::new(font),
                    data,
                })
            })
        };
        if bold && !f.bold.is_empty() {
            let b = f.loaded[1].get_or_init(|| load(f.bold)).as_ref();
            if b.is_some() {
                return b;
            }
        }
        f.loaded[0].get_or_init(|| load(f.regular)).as_ref()
    }

    /// Fallback order for a line: Japanese text (kana present) prefers Yu Gothic for shared
    /// Han characters, Korean text prefers Malgun Gothic.
    fn order(&self, pref: Script) -> impl Iterator<Item = usize> + '_ {
        let first = match pref {
            Script::Japanese => Some(YU_GOTHIC),
            Script::Korean => Some(MALGUN),
            Script::Other => None,
        }
        .filter(|i| *i < self.faces.len());
        first
            .into_iter()
            .chain((0..self.faces.len()).filter(move |i| Some(*i) != first))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Script {
    Japanese,
    Korean,
    Other,
}

fn script_of(text: &str) -> Script {
    for c in text.chars() {
        match c as u32 {
            0x3040..=0x30ff | 0x31f0..=0x31ff | 0xff66..=0xff9f => return Script::Japanese,
            0xac00..=0xd7af | 0x1100..=0x11ff | 0x3130..=0x318f => return Script::Korean,
            _ => {}
        }
    }
    Script::Other
}

/// Characters that only modify their neighbours (joiners, variation selectors, tags, emoji
/// skin tones): never drawn, never a missing glyph. Glyphs are drawn one per character in
/// one color, so a skin tone would otherwise show as a separate hatched swatch.
pub fn is_ignorable(c: char) -> bool {
    matches!(c as u32,
        0x200b..=0x200f | 0x2060..=0x2064 | 0xfe00..=0xfe0f | 0xfeff
        | 0x1f3fb..=0x1f3ff | 0xe0000..=0xe007f | 0xe0100..=0xe01ef)
}

/// A glyph chosen for one character: which face (None = primary) and its id.
#[derive(Clone, Copy)]
struct Pick {
    face: Option<usize>,
    id: GlyphId,
}

impl Fonts {
    /// Load from `%WINDIR%\Fonts`. Regular: Segoe UI Variable, then Segoe UI, then Arial.
    /// Bold: Segoe UI Bold, then Arial Bold, then the regular face. CJK / emoji fallbacks
    /// load on first use.
    pub fn load_system() -> anyhow::Result<Fonts> {
        let dir = windows_fonts_dir();
        let regular = first_loadable(&dir, &["SegUIVar.ttf", "segoeui.ttf", "arial.ttf"])
            .with_context(|| format!("no usable regular font in {}", dir.display()))?;
        let bold = first_loadable(&dir, &["segoeuib.ttf", "arialbd.ttf"]).unwrap_or(regular);
        let mut f = Self::from_static(regular, bold)?;
        f.fallback = Arc::new(FallbackSet::system(dir));
        Ok(f)
    }

    /// From owned font data (tests), without fallbacks. The bytes are leaked: fonts live for
    /// the process.
    pub fn from_bytes(regular: Vec<u8>, bold: Vec<u8>) -> anyhow::Result<Fonts> {
        Self::from_static(
            Box::leak(regular.into_boxed_slice()),
            Box::leak(bold.into_boxed_slice()),
        )
    }

    pub fn from_static(regular: &'static [u8], bold: &'static [u8]) -> anyhow::Result<Fonts> {
        let r = ab_glyph::FontRef::try_from_slice(regular).context("invalid regular font data")?;
        let b = ab_glyph::FontRef::try_from_slice(bold).context("invalid bold font data")?;
        Ok(Fonts {
            regular: Arc::new(FontArc::new(r)),
            bold: Arc::new(FontArc::new(b)),
            regular_bytes: regular,
            bold_bytes: bold,
            fallback: Arc::new(FallbackSet::default()),
        })
    }

    fn face(&self, bold: bool) -> &FontArc {
        if bold { &self.bold } else { &self.regular }
    }

    /// The face that draws `c`: the primary when it has the glyph, else the first fallback
    /// that does, else the primary's missing-glyph box.
    fn pick(&self, c: char, bold: bool, script: Script) -> Pick {
        let id = self.face(bold).glyph_id(c);
        if id.0 != 0 {
            return Pick { face: None, id };
        }
        for i in self.fallback.order(script) {
            if let Some(l) = self.fallback.face(i, bold) {
                let fid = l.font.glyph_id(c);
                if fid.0 != 0 {
                    return Pick {
                        face: Some(i),
                        id: fid,
                    };
                }
            }
        }
        Pick { face: None, id }
    }

    /// The font and scale for `pick` at `size`. Fallback faces are scaled so their em square
    /// matches the primary's.
    fn scaled_face(&self, pick: Pick, bold: bool, size: f32) -> (&FontArc, PxScale) {
        let primary = self.face(bold);
        let size = size.max(1.0);
        match pick.face.and_then(|i| self.fallback.face(i, bold)) {
            None => (primary, PxScale::from(size)),
            Some(l) => {
                let em = |f: &FontArc| f.height_unscaled() / f.units_per_em().unwrap_or(1000.0);
                let mul = em(&l.font) / em(primary).max(1e-3);
                (&l.font, PxScale::from(size * mul))
            }
        }
    }

    /// Walk one line's glyphs: `f(font, glyph id, scale, pen x)` for each drawn glyph.
    /// Returns the pen position after the line. Tabs are four spaces.
    fn walk_line(
        &self,
        line: &str,
        size: f32,
        bold: bool,
        x: f32,
        mut f: impl FnMut(&FontArc, GlyphId, PxScale, f32),
    ) -> f32 {
        let script = if self.fallback.faces.is_empty() || line.is_ascii() {
            Script::Other
        } else {
            script_of(line)
        };
        let primary = self.face(bold).as_scaled(PxScale::from(size.max(1.0)));
        let mut pen = x;
        let mut prev: Option<(Option<usize>, GlyphId)> = None;
        for c in line.chars() {
            if c == '\r' || is_ignorable(c) {
                continue;
            }
            if c == '\t' {
                pen += 4.0 * primary.h_advance(primary.glyph_id(' '));
                prev = None;
                continue;
            }
            if c.is_control() {
                continue;
            }
            let pick = self.pick(c, bold, script);
            let (font, scale) = self.scaled_face(pick, bold, size);
            let scaled = font.as_scaled(scale);
            if let Some((pf, pid)) = prev
                && pf == pick.face
            {
                pen += scaled.kern(pid, pick.id);
            }
            f(font, pick.id, scale, pen);
            pen += scaled.h_advance(pick.id);
            prev = Some((pick.face, pick.id));
        }
        pen
    }

    /// Pixel size of laid-out multi-line text (line height = 1.25 * size).
    pub fn measure(&self, text: &str, size: f32, bold: bool) -> (f32, f32) {
        let mut w = 0f32;
        let mut lines = 0usize;
        for line in text.split('\n') {
            lines += 1;
            w = w.max(self.line_width(line, size, bold));
        }
        (w, lines as f32 * LINE_HEIGHT * size)
    }

    /// Advance width of a single line (no newlines), including kerning.
    pub fn line_width(&self, line: &str, size: f32, bold: bool) -> f32 {
        self.walk_line(line, size, bold, 0.0, |_, _, _, _| {})
    }

    /// Ascent in px at `size` (distance from the top of the em box to the baseline).
    pub fn ascent(&self, size: f32, bold: bool) -> f32 {
        self.face(bold)
            .as_scaled(PxScale::from(size.max(1.0)))
            .ascent()
    }

    /// Outlined glyphs for one line with its pen starting at (`x`, `baseline`).
    pub fn outline_line(
        &self,
        line: &str,
        size: f32,
        bold: bool,
        x: f32,
        baseline: f32,
    ) -> Vec<OutlinedGlyph> {
        let mut out = Vec::with_capacity(line.len());
        self.walk_line(line, size, bold, x, |font, id, scale, pen| {
            let glyph = id.with_scale_and_position(scale, point(pen, baseline));
            if let Some(o) = font.outline_glyph(glyph) {
                out.push(o);
            }
        });
        out
    }

    /// Characters of `text` that no available face can draw (whitespace, controls and
    /// zero-width modifiers excluded).
    pub fn missing_glyphs(&self, text: &str) -> usize {
        let script = script_of(text);
        text.chars()
            .filter(|c| !c.is_whitespace() && !c.is_control() && !is_ignorable(*c))
            .filter(|c| {
                let p = self.pick(*c, false, script);
                p.face.is_none() && p.id.0 == 0
            })
            .count()
    }

    /// Name of the face that draws `c` in `context` ("primary" for the main face).
    pub fn face_name_for(&self, c: char, context: &str) -> &'static str {
        match self.pick(c, false, script_of(context)).face {
            Some(i) => self.fallback.faces[i].name,
            None => "primary",
        }
    }

    /// The fallback face egui should use for `c` when the UI font lacks it (loads it on
    /// first use). None when the primary face has it or nothing does.
    pub fn fallback_for_char(&self, c: char) -> Option<FallbackFont> {
        let i = self.pick(c, false, Script::Other).face?;
        let l = self.fallback.face(i, false)?;
        Some(FallbackFont {
            name: self.fallback.faces[i].name,
            data: l.data,
            index: 0,
        })
    }
}

fn windows_fonts_dir() -> PathBuf {
    let win = std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    win.join("Fonts")
}

fn first_loadable(dir: &std::path::Path, names: &[&str]) -> Option<&'static [u8]> {
    names.iter().find_map(|name| map_font_file(&dir.join(name)))
}

#[cfg(test)]
pub(crate) fn test_fonts() -> Fonts {
    Fonts::load_system().expect("system fonts (Windows) are required for these tests")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_system_fonts() {
        let f = test_fonts();
        assert!(!f.regular_bytes.is_empty() && !f.bold_bytes.is_empty());
    }

    #[test]
    fn measure_scales_with_size_and_lines() {
        let f = test_fonts();
        let (w1, h1) = f.measure("Hello", 20.0, false);
        let (w2, h2) = f.measure("Hello", 40.0, false);
        assert!(w1 > 20.0 && w2 > w1 * 1.8 && w2 < w1 * 2.2, "{w1} {w2}");
        assert!((h1 - 25.0).abs() < 1e-3 && (h2 - 50.0).abs() < 1e-3);
        let (w3, h3) = f.measure("Hello\nHi there, wider", 20.0, false);
        assert!(w3 > w1);
        assert!((h3 - 50.0).abs() < 1e-3);
        let (we, he) = f.measure("", 20.0, false);
        assert_eq!(we, 0.0);
        assert!((he - 25.0).abs() < 1e-3);
    }

    #[test]
    fn bold_is_not_narrower_for_text() {
        let f = test_fonts();
        let (r, _) = f.measure("Number 42", 24.0, false);
        let (b, _) = f.measure("Number 42", 24.0, true);
        assert!(b >= r * 0.95);
    }

    #[test]
    fn outline_matches_measure() {
        let f = test_fonts();
        let glyphs = f.outline_line("Hi", 30.0, false, 10.0, 40.0);
        assert_eq!(glyphs.len(), 2);
        let right = glyphs
            .iter()
            .map(|g| g.px_bounds().max.x)
            .fold(0.0, f32::max);
        assert!(right <= 10.0 + f.line_width("Hi", 30.0, false) + 1.0);
        assert!(glyphs[0].px_bounds().min.x >= 9.0);
    }

    #[test]
    fn tabs_and_carriage_returns_do_not_panic() {
        let f = test_fonts();
        let (w, _) = f.measure("a\tb\r", 20.0, false);
        let (w0, _) = f.measure("ab", 20.0, false);
        assert!(w > w0);
    }

    #[test]
    fn rejects_garbage_font_bytes() {
        assert!(Fonts::from_bytes(vec![1, 2, 3], vec![4, 5, 6]).is_err());
    }

    fn installed(file: &str) -> bool {
        windows_fonts_dir().join(file).exists()
    }

    /// Mixed scripts: every character has a glyph once the fallbacks are on, and none of the
    /// CJK ones do without them.
    #[test]
    fn cjk_and_emoji_fall_back_to_system_faces() {
        let all = ["msyh.ttc", "YuGothR.ttc", "malgun.ttf", "seguiemj.ttf"];
        if !all.iter().all(|f| installed(f)) {
            eprintln!("skipped: CJK / emoji fonts not installed");
            return;
        }
        let f = test_fonts();
        let samples = [
            "Hello \u{4e16}\u{754c}",                                         // Chinese
            "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f} \u{65e5}\u{672c}",      // Japanese
            "\u{c548}\u{b155}\u{d558}\u{c138}\u{c694}",                       // Korean
            "Ship it \u{1f680}\u{1f600}",                                     // emoji
            "\u{1f44d}\u{1f3fd} \u{2764}\u{fe0f} \u{1f468}\u{200d}\u{1f4bb}", // modifiers, VS16, ZWJ
        ];
        for s in samples {
            assert_eq!(f.missing_glyphs(s), 0, "missing glyphs in {s:?}");
        }
        let bare = Fonts::from_bytes(f.regular_bytes.to_vec(), f.bold_bytes.to_vec()).unwrap();
        assert!(bare.missing_glyphs("\u{4e16}\u{754c}") == 2);
        assert!(bare.missing_glyphs("\u{c548}\u{b155}") == 2);
    }

    #[test]
    fn fallback_face_choice_follows_script() {
        if !["msyh.ttc", "YuGothR.ttc", "malgun.ttf", "seguiemj.ttf"]
            .iter()
            .all(|f| installed(f))
        {
            eprintln!("skipped: CJK / emoji fonts not installed");
            return;
        }
        let f = test_fonts();
        assert_eq!(f.face_name_for('A', "A"), "primary");
        // Han in Chinese text: YaHei; the same character in Japanese text: Yu Gothic.
        assert_eq!(
            f.face_name_for('\u{65e5}', "\u{65e5}\u{672c}"),
            "Microsoft YaHei"
        );
        assert_eq!(
            f.face_name_for('\u{65e5}', "\u{65e5}\u{672c}\u{306e}"),
            "Yu Gothic"
        );
        assert_eq!(f.face_name_for('\u{c548}', "\u{c548}"), "Malgun Gothic");
        assert_eq!(f.face_name_for('\u{1f680}', ""), "Segoe UI Emoji");
        let fb = f.fallback_for_char('\u{4e16}').expect("YaHei for egui");
        assert_eq!(fb.name, "Microsoft YaHei");
        assert!(fb.data.len() > 1_000_000);
        assert!(f.fallback_for_char('a').is_none());
    }

    #[test]
    fn mixed_line_layout_is_consistent() {
        if !installed("msyh.ttc") {
            eprintln!("skipped: Microsoft YaHei not installed");
            return;
        }
        let f = test_fonts();
        let line = "Price \u{4ef7}\u{683c} 42";
        let w = f.line_width(line, 24.0, false);
        let glyphs = f.outline_line(line, 24.0, false, 0.0, 30.0);
        // 11 visible glyphs minus the two spaces that have no outline.
        assert_eq!(glyphs.len(), line.chars().filter(|c| *c != ' ').count());
        let right = glyphs
            .iter()
            .map(|g| g.px_bounds().max.x)
            .fold(0.0, f32::max);
        assert!(right <= w + 2.0, "{right} > {w}");
        // A CJK glyph is about one em wide, like the primary's em at the same size.
        let cjk = f.line_width("\u{4ef7}", 24.0, false);
        assert!((18.0..=30.0).contains(&cjk), "{cjk}");
        // Bold CJK resolves too.
        assert!(!f.outline_line("\u{4ef7}", 24.0, true, 0.0, 30.0).is_empty());
    }
}
