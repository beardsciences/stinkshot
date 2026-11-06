//! Embeds the application manifest (PerMonitorV2 DPI, Common Controls v6, asInvoker), the
//! generated icon and version info into the exe.

#[path = "src/icon_gen.rs"]
mod icon_gen;

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/icon_gen.rs");
    println!("cargo:rerun-if-changed=assets/stinkshot.manifest");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"));

    let ico_path = out_dir.join("stinkshot.ico");
    std::fs::write(&ico_path, icon_gen::ico_bytes([0x7f, 0xd3, 0x6a])).expect("write generated icon");

    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".into());
    let mut parts = version.split('.').map(|p| p.split('-').next().unwrap_or("0").parse::<u32>().unwrap_or(0));
    let (major, minor, patch) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));

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
      VALUE "ProductName", "StinkShot"
      VALUE "FileDescription", "StinkShot: local-first screenshot and screen recording"
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
