#[cfg(any(windows, test))]
use std::path::PathBuf;
#[cfg(any(windows, test))]
use viia_core::MediaUrl;

#[cfg(any(windows, test))]
fn local_image_path(source: &str) -> Result<PathBuf, String> {
    let source = MediaUrl::parse_url(source).map_err(|_| "Invalid image URL".to_string())?;
    let path = source
        .to_file_path()
        .ok_or_else(|| "Only local images can be revealed in File Explorer".to_string())?;
    if !path.is_file() {
        return Err("The image file no longer exists or cannot be accessed".to_string());
    }
    Ok(path)
}

#[cfg(windows)]
pub(super) fn reveal(source: &str) -> Result<(), String> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
        UI::Shell::{Common::ITEMIDLIST, SHOpenFolderAndSelectItems, SHParseDisplayName},
    };

    struct Apartment(bool);
    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: Balance this function's successful CoInitializeEx on the
                // same thread, including the S_FALSE/already-initialized case.
                unsafe { CoUninitialize() };
            }
        }
    }

    struct ShellItem(*mut ITEMIDLIST);
    impl Drop for ShellItem {
        fn drop(&mut self) {
            // SAFETY: SHParseDisplayName allocates this item with the COM allocator.
            // It stays alive until SHOpenFolderAndSelectItems has returned.
            unsafe { CoTaskMemFree(self.0.cast()) };
        }
    }

    let path = local_image_path(source)?;
    // SAFETY: No reserved pointer is supplied. Both guards remain local to this
    // synchronous call, so COM cleanup cannot migrate to another thread.
    let initialized = unsafe { CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    if initialized < 0 && initialized != RPC_E_CHANGED_MODE {
        return Err(format!(
            "Could not initialize Windows shell (0x{initialized:08X})"
        ));
    }
    // If the caller already uses another COM apartment, keep its existing setup.
    let _apartment = Apartment(initialized >= 0);
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut item = ptr::null_mut();
    // SAFETY: name is null-terminated UTF-16, item is a valid output pointer, and
    // neither optional binding context nor attribute output is requested.
    let parsed = unsafe {
        SHParseDisplayName(
            name.as_ptr(),
            ptr::null_mut(),
            &mut item,
            0,
            ptr::null_mut(),
        )
    };
    if parsed < 0 || item.is_null() {
        return Err(format!(
            "Could not locate the image in Windows shell (0x{parsed:08X})"
        ));
    }
    let item = ShellItem(item);
    // SAFETY: With zero child items, the absolute PIDL names the single file to
    // select. The shell opens Explorer and focuses that file without parsing a
    // command line, preserving commas and other characters in the filename.
    let opened = unsafe { SHOpenFolderAndSelectItems(item.0, 0, ptr::null(), 0) };
    if opened < 0 {
        return Err(format!("Could not open File Explorer (0x{opened:08X})"));
    }
    tracing::info!(path = %path.display(), "Revealed image in File Explorer");
    Ok(())
}

#[cfg(not(windows))]
pub(super) fn reveal(_source: &str) -> Result<(), String> {
    Err("Reveal in File Explorer is available on Windows only".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_paths_preserve_unicode_and_shell_metacharacters() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("图像 [1], $name & 'photo'.png");
        std::fs::write(&path, []).unwrap();
        let source = MediaUrl::from_abs_path(&path).unwrap();
        assert_eq!(local_image_path(source.as_str()).unwrap(), path);
        assert_eq!(source.file_name().unwrap(), "图像 [1], $name & 'photo'.png");
    }

    #[test]
    fn remote_invalid_missing_and_directory_sources_are_rejected() {
        assert!(local_image_path("sftp://host/path/image.png").is_err());
        assert!(local_image_path("invalid URL").is_err());
        let dir = tempfile::tempdir().unwrap();
        let missing = MediaUrl::from_abs_path(&dir.path().join("missing.png")).unwrap();
        assert!(local_image_path(missing.as_str()).is_err());
        let directory = MediaUrl::from_abs_path(dir.path()).unwrap();
        assert!(local_image_path(directory.as_str()).is_err());
    }
}
