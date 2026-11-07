//! Renders the logo (`stinkshot-logo`) once: the multi-resolution exe icon, plus raw RGBA
//! bitmaps of the tray and window sizes that `src/icon_gen.rs` embeds, so the app never ray
//! marches an icon at runtime. Then embeds the application manifest (PerMonitorV2 DPI,
//! Common Controls v6, asInvoker), the icon and version info into the exe.

use std::path::PathBuf;

/// Sizes `src/icon_gen.rs` embeds (tray 16-32, window 64). All are also in the .ico.
const EMBEDDED_SIZES: [u32; 5] = [16, 20, 24, 32, 64];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/stinkshot.manifest");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
    );

    // About a second with the renderer optimized (see the workspace profile overrides).
    let frames: Vec<(u32, Vec<u8>)> = stinkshot_logo::ICO_SIZES
        .iter()
        .map(|s| (*s, stinkshot_logo::render_icon(*s)))
        .collect();
    for (size, rgba) in &frames {
        if EMBEDDED_SIZES.contains(size) {
            std::fs::write(out_dir.join(format!("logo-{size}.rgba")), rgba)
                .expect("write logo bitmap");
        }
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let ico_path = out_dir.join("stinkshot.ico");
    std::fs::write(&ico_path, stinkshot_logo::ico_from(&frames)).expect("write generated icon");

    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".into());
    let mut parts = version.split('.').map(|p| {
        p.split('-')
            .next()
            .unwrap_or("0")
            .parse::<u32>()
            .unwrap_or(0)
    });
    let (major, minor, patch) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );

    let slash = |p: PathBuf| p.to_string_lossy().replace('\\', "/");
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{ico}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "ProductName", "Stinkshot"
      VALUE "FileDescription", "Stinkshot: local-first screenshot and screen recording"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "stinkshot.exe"
      VALUE "LegalCopyright", "GPL-3.0-or-later"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 0x04B0
  END
END
"#,
        ico = slash(ico_path),
        manifest = slash(manifest_dir.join("assets").join("stinkshot.manifest")),
    );
    let rc_path = out_dir.join("stinkshot.rc");
    std::fs::write(&rc_path, rc).expect("write resource script");

    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_required()
        .expect("embed Windows resources (needs the Windows SDK rc.exe)");
}
