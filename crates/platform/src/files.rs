//! File helpers for background jobs: swap a file for a re-written copy only when nobody else
//! has it open or changed it, and run a thread at background priority.

use std::os::windows::fs::{FileTimesExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::time::SystemTime;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FILE_RENAME_INFO, FileRenameInfoEx, SetFileInformationByHandle,
};

const FILE_READ_DATA: u32 = 0x0001;
const FILE_READ_ATTRIBUTES: u32 = 0x0080;
const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
const DELETE: u32 = 0x0001_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const FILE_SHARE_DELETE: u32 = 0x0000_0004;
const FILE_RENAME_FLAG_REPLACE_IF_EXISTS: u32 = 0x1;
const FILE_RENAME_FLAG_POSIX_SEMANTICS: u32 = 0x2;
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;

/// What `replace_if_unchanged` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replace {
    /// `original` now has the replacement's contents (and its own timestamps).
    Replaced,
    /// Another process has `original` open (or the volume refused the swap); left alone.
    InUse,
    /// `original` is gone or no longer has the expected size and modification time.
    Changed,
}

/// Replace `original` with `replacement` (a finished file in the same folder) when
/// `original` still has `expect_len` bytes and was last written at `expect_modified`, and no
/// other process has it open. `original` keeps its creation and modification times.
///
/// `original` is held open without read or write sharing while it is checked and swapped, so
/// no other program can open it in between. The swap is a POSIX-semantics rename of
/// `replacement` over it (NTFS, Windows 10 1809 and later): atomic, and a reader that opened
/// the file with delete sharing keeps reading the old data. Volumes without that rename
/// report `InUse`. On anything but `Replaced` the caller still owns `replacement`.
pub fn replace_if_unchanged(
    original: &Path,
    replacement: &Path,
    expect_len: u64,
    expect_modified: SystemTime,
) -> anyhow::Result<Replace> {
    let held = match std::fs::OpenOptions::new()
        // Read access takes part in sharing checks (attribute-only opens do not), so other
        // handles make this fail and new ones fail while it is held.
        .access_mode(FILE_READ_DATA | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_DELETE)
        .open(original)
    {
        Ok(f) => f,
        Err(e)
            if matches!(
                e.raw_os_error(),
                Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
            ) =>
        {
            return Ok(Replace::InUse);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Replace::Changed),
        Err(e) => return Err(e.into()),
    };
    let md = held.metadata()?;
    if md.len() != expect_len || md.modified()? != expect_modified {
        return Ok(Replace::Changed);
    }
    let created = md.created().ok();
    let modified = md.modified()?;

    // A virus scanner may still be reading the fresh replacement: share everything.
    let temp = match std::fs::OpenOptions::new()
        .access_mode(DELETE | FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(replacement)
    {
        Ok(f) => f,
        Err(e) => {
            log::debug!("replace: open {}: {e}", replacement.display());
            return Ok(Replace::InUse);
        }
    };
    let mut times = std::fs::FileTimes::new().set_modified(modified);
    if let Some(c) = created {
        times = times.set_created(c);
    }
    temp.set_times(times)?;

    let target: Vec<u16> = std::path::absolute(original)?.as_os_str().encode_wide_vec();
    // FILE_RENAME_INFO with the name inline; u64 backing keeps the struct aligned.
    let header = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let bytes = header + target.len() * 2 + 2;
    let mut buf = vec![0u64; bytes.div_ceil(8)];
    // SAFETY: `buf` is large enough and aligned for FILE_RENAME_INFO plus the name; the
    // handle is live for the call.
    let swapped = unsafe {
        let info = buf.as_mut_ptr() as *mut FILE_RENAME_INFO;
        (*info).Anonymous.Flags =
            FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS;
        (*info).RootDirectory = HANDLE::default();
        (*info).FileNameLength = (target.len() * 2) as u32;
        let name = (buf.as_mut_ptr() as *mut u8).add(header) as *mut u16;
        std::ptr::copy_nonoverlapping(target.as_ptr(), name, target.len());
        SetFileInformationByHandle(
            HANDLE(temp.as_raw_handle()),
            FileRenameInfoEx,
            buf.as_ptr() as *const core::ffi::c_void,
            (buf.len() * 8) as u32,
        )
    };
    drop(temp);
    drop(held);
    match swapped {
        Ok(()) => Ok(Replace::Replaced),
        Err(e) => {
            log::debug!("replace {}: {e}", original.display());
            Ok(Replace::InUse)
        }
    }
}

trait EncodeWideVec {
    fn encode_wide_vec(&self) -> Vec<u16>;
}

impl EncodeWideVec for std::ffi::OsStr {
    fn encode_wide_vec(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().collect()
    }
}

/// Run the calling thread in background mode: lowest CPU scheduling, very low I/O and memory
/// priority. For work nobody waits for (re-compressing saved files).
pub fn background_thread() {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
    };
    // SAFETY: the pseudo handle of the calling thread.
    if let Err(e) = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) } {
        log::debug!("background thread priority: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "stinkshot-files-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn replaces_only_unchanged_unopened_files() {
        let d = dir();
        let orig = d.join("a.png");
        let new = d.join("a.png.tmp");
        std::fs::write(&orig, b"original contents").unwrap();
        let md = std::fs::metadata(&orig).unwrap();
        let (len, modified) = (md.len(), md.modified().unwrap());

        // Wrong expectations: changed.
        std::fs::write(&new, b"new").unwrap();
        assert_eq!(
            replace_if_unchanged(&orig, &new, len + 1, modified).unwrap(),
            Replace::Changed
        );
        // Open elsewhere without delete sharing: in use.
        {
            let _other = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(1) // FILE_SHARE_READ
                .open(&orig)
                .unwrap();
            assert_eq!(
                replace_if_unchanged(&orig, &new, len, modified).unwrap(),
                Replace::InUse
            );
        }
        // Open elsewhere with every sharing flag (a viewer, a scanner): still in use.
        {
            let _other = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(7)
                .open(&orig)
                .unwrap();
            assert_eq!(
                replace_if_unchanged(&orig, &new, len, modified).unwrap(),
                Replace::InUse
            );
        }
        assert_eq!(std::fs::read(&orig).unwrap(), b"original contents");
        // Unchanged and closed: replaced, times kept.
        assert_eq!(
            replace_if_unchanged(&orig, &new, len, modified).unwrap(),
            Replace::Replaced
        );
        assert_eq!(std::fs::read(&orig).unwrap(), b"new");
        assert!(!new.exists());
        assert_eq!(
            std::fs::metadata(&orig).unwrap().modified().unwrap(),
            modified
        );
        // Gone: changed.
        std::fs::write(&new, b"x").unwrap();
        std::fs::remove_file(&orig).unwrap();
        assert_eq!(
            replace_if_unchanged(&orig, &new, len, modified).unwrap(),
            Replace::Changed
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
