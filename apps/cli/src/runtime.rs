//! Runtime capability checks for optional DynDOLOD DLL features.

use std::path::Path;

#[cfg(windows)]
use anyhow::Context;
use anyhow::{Result, ensure};

pub fn supports_base_lod(version: [u16; 4]) -> bool {
    version >= [1, 6, 1130, 0]
}

pub fn base_lod_runtime(
    setting: &str,
    has_dll: bool,
    read_version: impl FnOnce() -> Result<[u16; 4]>,
) -> Result<Option<[u16; 4]>> {
    let setting = setting.trim().to_ascii_lowercase();
    ensure!(
        matches!(setting.as_str(), "" | "auto" | "0" | "1"),
        "BaseLODDLL must be auto, 0 or 1, got {setting:?}"
    );
    if setting == "0" {
        return Ok(None);
    }
    if !has_dll {
        ensure!(
            setting != "1",
            "BaseLODDLL=1 requires DynDOLOD DLL NG in the profile"
        );
        eprintln!("note: BaseLODDLL disabled: no DynDOLOD DLL in the profile");
        return Ok(None);
    }
    let version = read_version()?;
    if !supports_base_lod(version) {
        ensure!(
            setting != "1",
            "BaseLODDLL=1 requires Skyrim >= 1.6.1130; found {version:?}"
        );
        eprintln!("note: BaseLODDLL disabled: Skyrim {version:?} predates 1.6.1130");
        return Ok(None);
    }
    Ok(Some(version))
}

#[cfg(windows)]
pub fn file_version(path: &Path) -> Result<[u16; 4]> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };

    let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
    ensure!(!name.contains(&0), "NUL in version resource path");
    name.push(0);
    let error = || std::io::Error::last_os_error();
    // The path is terminated, and buffers remain live throughout the version API calls.
    let size = unsafe { GetFileVersionInfoSizeW(name.as_ptr(), std::ptr::null_mut()) };
    if size == 0 {
        return Err(error()).with_context(|| format!("reading version size of {}", path.display()));
    }
    let mut data = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(name.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return Err(error()).with_context(|| format!("reading version of {}", path.display()));
    }
    let mut value = std::ptr::null_mut();
    let mut len = 0;
    let root = [b'\\' as u16, 0];
    let ok = unsafe { VerQueryValueW(data.as_ptr().cast(), root.as_ptr(), &mut value, &mut len) };
    ensure!(
        ok != 0 && !value.is_null() && len as usize >= std::mem::size_of::<VS_FIXEDFILEINFO>(),
        "missing fixed version in {}",
        path.display()
    );
    // VerQueryValue returns a structure in `data`; do not assume byte-buffer alignment.
    let info = unsafe { value.cast::<VS_FIXEDFILEINFO>().read_unaligned() };
    ensure!(
        info.dwSignature == 0xfeef04bd,
        "invalid fixed version in {}",
        path.display()
    );
    Ok([
        (info.dwFileVersionMS >> 16) as u16,
        info.dwFileVersionMS as u16,
        (info.dwFileVersionLS >> 16) as u16,
        info.dwFileVersionLS as u16,
    ])
}

#[cfg(not(windows))]
pub fn file_version(path: &Path) -> Result<[u16; 4]> {
    anyhow::bail!(
        "runtime version detection requires Windows: {}; set BaseLODDLL=0 to disable this feature",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_lod_runtime_boundary() {
        assert!(!supports_base_lod([1, 5, 97, 0]));
        assert!(!supports_base_lod([1, 6, 640, 0]));
        assert!(!supports_base_lod([1, 6, 1129, 65535]));
        assert!(supports_base_lod([1, 6, 1130, 0]));
        assert!(supports_base_lod([1, 6, 1170, 0]));
        assert!(supports_base_lod([1, 7, 104, 0]));
    }

    #[test]
    fn missing_executable_is_an_error() {
        assert!(file_version(Path::new("lodforge-missing-runtime.exe")).is_err());
    }

    #[test]
    fn base_lod_configuration_and_dependencies() {
        let unread = || -> Result<[u16; 4]> { panic!("runtime should not be read") };
        assert_eq!(base_lod_runtime("0", true, unread).unwrap(), None);
        assert_eq!(base_lod_runtime("auto", false, unread).unwrap(), None);
        assert!(base_lod_runtime("1", false, unread).is_err());
        assert!(base_lod_runtime("invalid", true, unread).is_err());
        assert_eq!(
            base_lod_runtime("auto", true, || Ok([1, 6, 640, 0])).unwrap(),
            None
        );
        assert!(base_lod_runtime("1", true, || Ok([1, 6, 640, 0])).is_err());
        for setting in ["", "auto", "1", " AUTO "] {
            assert_eq!(
                base_lod_runtime(setting, true, || Ok([1, 7, 104, 0])).unwrap(),
                Some([1, 7, 104, 0])
            );
        }
        assert!(base_lod_runtime("auto", true, || anyhow::bail!("unreadable version")).is_err());
    }
}
