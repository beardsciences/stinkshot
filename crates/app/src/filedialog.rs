//! Native Save As dialog (IFileSaveDialog). Call from a worker thread: it initializes COM
//! (STA) for the calling thread and blocks until the user chooses or cancels.

use std::path::{Path, PathBuf};

use anyhow::Context;
use stinkshot_core::image::ImageFormat;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, FileSaveDialog,
    IFileSaveDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::core::{HRESULT, HSTRING, PCWSTR, w};

use crate::l10n::tr;

/// HRESULT_FROM_WIN32(ERROR_CANCELLED)
const CANCELLED: HRESULT = HRESULT(0x800704C7u32 as i32);

/// Show a Save As dialog for an image. `default_stem` is the suggested file name without
/// extension; `format` selects the initial filter. Returns None if the user cancelled. The
/// returned path always carries an extension matching one of the offered filters.
pub fn save_image(
    default_stem: &str,
    format: ImageFormat,
    start_dir: Option<&Path>,
) -> anyhow::Result<Option<PathBuf>> {
    // SAFETY: COM calls on a thread we initialized; all string buffers outlive the calls.
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let uninit = init.is_ok();
        let result = show(default_stem, format, start_dir);
        if uninit {
            CoUninitialize();
        }
        result
    }
}

unsafe fn show(
    default_stem: &str,
    format: ImageFormat,
    start_dir: Option<&Path>,
) -> anyhow::Result<Option<PathBuf>> {
    unsafe {
        let dlg: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
            .context("create save dialog")?;
        let names = [
            HSTRING::from(tr!("shell.save_as.png")),
            HSTRING::from(tr!("shell.save_as.jpeg")),
            HSTRING::from(tr!("shell.save_as.webp")),
        ];
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: PCWSTR(names[0].as_ptr()),
                pszSpec: w!("*.png"),
            },
            COMDLG_FILTERSPEC {
                pszName: PCWSTR(names[1].as_ptr()),
                pszSpec: w!("*.jpg;*.jpeg"),
            },
            COMDLG_FILTERSPEC {
                pszName: PCWSTR(names[2].as_ptr()),
                pszSpec: w!("*.webp"),
            },
        ];
        dlg.SetFileTypes(&filters)?;
        let (index, ext): (u32, PCWSTR) = match format {
            ImageFormat::Png => (1, w!("png")),
            ImageFormat::Jpeg => (2, w!("jpg")),
            ImageFormat::Webp => (3, w!("webp")),
        };
        dlg.SetFileTypeIndex(index)?;
        dlg.SetDefaultExtension(ext)?;
        dlg.SetTitle(&HSTRING::from(tr!("shell.save_as.title")))?;
        dlg.SetFileName(&HSTRING::from(default_stem))?;
        let opts = dlg.GetOptions()?;
        dlg.SetOptions(
            opts | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR,
        )?;
        if let Some(dir) = start_dir
            && dir.is_dir()
            && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(
                &HSTRING::from(dir.as_os_str()),
                None,
            )
        {
            let _ = dlg.SetFolder(&item);
        }
        match dlg.Show(Some(HWND::default())) {
            Ok(()) => {}
            Err(e) if e.code() == CANCELLED => return Ok(None),
            Err(e) => return Err(e).context("show save dialog"),
        }
        let item = dlg.GetResult()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = name
            .to_string()
            .context("save dialog returned a non-UTF-16 path");
        CoTaskMemFree(Some(name.0 as *const _));
        Ok(Some(PathBuf::from(path?)))
    }
}

/// The image format implied by a file extension (png default).
pub fn format_for_path(path: &Path) -> ImageFormat {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => ImageFormat::Jpeg,
        Some("webp") => ImageFormat::Webp,
        _ => ImageFormat::Png,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_maps_to_format() {
        assert_eq!(format_for_path(Path::new("C:/x/a.PNG")), ImageFormat::Png);
        assert_eq!(format_for_path(Path::new("a.jpeg")), ImageFormat::Jpeg);
        assert_eq!(format_for_path(Path::new("a.webp")), ImageFormat::Webp);
        assert_eq!(format_for_path(Path::new("a")), ImageFormat::Png);
    }
}
