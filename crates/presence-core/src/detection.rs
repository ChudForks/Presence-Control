//! Windowed application discovery through Windows APIs, without child processes.
use crate::model::DetectedApp;

/// Enumerate on a blocking worker so Windows metadata reads cannot block the
/// Tokio scheduler or native UI event loop. Protected/exited processes are skipped.
#[cfg(windows)]
pub async fn detect_apps() -> Result<Vec<DetectedApp>, String> {
    tokio::task::spawn_blocking(windows::enumerate)
        .await
        .map_err(|error| format!("Application detection failed: {error}"))?
}

#[cfg(not(windows))]
pub async fn detect_apps() -> Result<Vec<DetectedApp>, String> {
    Err("Application detection is currently available on Windows.".into())
}

#[cfg(any(windows, test))]
fn normalize_apps(mut apps: Vec<DetectedApp>, own_pid: u32) -> Vec<DetectedApp> {
    let mut seen = std::collections::HashSet::new();
    apps.retain(|app| {
        app.pid != 0
            && app.pid != own_pid
            && !app.title.is_empty()
            && !app.process.is_empty()
            && !app.process.eq_ignore_ascii_case("presence control.exe")
            && !app.process.eq_ignore_ascii_case("electron.exe")
            && seen.insert(app.pid)
    });
    apps.sort_by_cached_key(|app| (app.process.to_lowercase(), app.pid));
    apps
}

#[cfg(windows)]
mod windows {
    use super::{normalize_apps, DetectedApp};
    use std::{ffi::c_void, path::Path, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM},
        Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
        System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
            IsWindowVisible, GW_OWNER,
        },
    };

    struct ProcessHandle(HANDLE);
    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            // SAFETY: this wrapper owns the non-null OpenProcess handle.
            unsafe { CloseHandle(self.0) };
        }
    }

    pub fn enumerate() -> Result<Vec<DetectedApp>, String> {
        let mut apps = Vec::<DetectedApp>::new();
        // SAFETY: EnumWindows invokes the callback synchronously. apps remains
        // alive at a stable address and no other code accesses it during the call.
        let succeeded = unsafe { EnumWindows(Some(visit_window), (&mut apps as *mut _) as LPARAM) };
        if succeeded == 0 {
            return Err(format!(
                "Application detection failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(normalize_apps(apps, std::process::id()))
    }

    unsafe extern "system" fn visit_window(window: HWND, parameter: LPARAM) -> i32 {
        // Match the visible, ownerless main-window criteria used by the source
        // rewrite, including minimized windows. An inaccessible process is normal.
        if unsafe { IsWindowVisible(window) } == 0
            || !unsafe { GetWindow(window, GW_OWNER) }.is_null()
        {
            return 1;
        }
        let length = unsafe { GetWindowTextLengthW(window) };
        if length <= 0 {
            return 1;
        }
        let mut title = vec![0u16; (length as usize + 1).min(32_768)];
        let copied = unsafe { GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32) };
        if copied <= 0 {
            return 1;
        }
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        if pid == 0 || pid == std::process::id() {
            return 1;
        }
        let Some(path) = process_path(pid) else {
            return 1;
        };
        let Some(process) = Path::new(&path).file_name().and_then(|name| name.to_str()) else {
            return 1;
        };
        let name = product_name(&path).unwrap_or_else(|| {
            Path::new(process)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        let app = DetectedApp {
            pid,
            process: process.to_owned(),
            title: String::from_utf16_lossy(&title[..copied as usize]),
            name: Some(name),
        };
        // SAFETY: parameter is the exclusive pointer supplied by enumerate;
        // EnumWindows does not retain it or invoke callbacks concurrently.
        unsafe { &mut *(parameter as *mut Vec<DetectedApp>) }.push(app);
        1
    }

    pub(super) fn process_path(pid: u32) -> Option<String> {
        // Limited query access works across normal elevation boundaries and
        // avoids requesting memory-reading or administrative permissions.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let handle = ProcessHandle(handle);
        let mut path = vec![0u16; 32_768];
        let mut length = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut length) } == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&path[..length as usize]))
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn product_name(path: &str) -> Option<String> {
        let path = wide(path);
        let mut unused = 0;
        let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &mut unused) };
        // Malformed or unusually large resources should not consume unlimited
        // memory. Normal executables have much smaller version blocks.
        if size == 0 || size > 4 * 1024 * 1024 {
            return None;
        }
        let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
        if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, buffer.as_mut_ptr().cast()) } == 0 {
            return None;
        }
        let mut languages = Vec::new();
        let translation = wide("\\VarFileInfo\\Translation");
        let mut data: *mut c_void = ptr::null_mut();
        let mut length = 0;
        if unsafe {
            VerQueryValueW(
                buffer.as_ptr().cast(),
                translation.as_ptr(),
                &mut data,
                &mut length,
            )
        } != 0
            && in_buffer(&buffer, data, length as usize)
        {
            for offset in (0..length as usize / 4).take(32) {
                // SAFETY: range was checked against the owned resource buffer;
                // unaligned reads also accept unusual resource alignment.
                let pair = unsafe { data.cast::<u8>().add(offset * 4) };
                languages.push(
                    (unsafe { ptr::read_unaligned(pair.cast::<u16>()) }, unsafe {
                        ptr::read_unaligned(pair.add(2).cast::<u16>())
                    }),
                );
            }
        }
        // Some files omit the translation table but contain standard English
        // Unicode/ANSI string tables. Prefer the executable's declared locales.
        languages.extend([(0x0409, 0x04b0), (0x0409, 0x04e4)]);
        for (language, codepage) in languages {
            let query = wide(&format!(
                "\\StringFileInfo\\{language:04x}{codepage:04x}\\ProductName"
            ));
            let mut data: *mut c_void = ptr::null_mut();
            let mut length = 0;
            if unsafe {
                VerQueryValueW(
                    buffer.as_ptr().cast(),
                    query.as_ptr(),
                    &mut data,
                    &mut length,
                )
            } == 0
                || !in_buffer(&buffer, data, length as usize * 2)
            {
                continue;
            }
            let units: Vec<u16> = (0..length as usize)
                .map(|offset| unsafe { ptr::read_unaligned(data.cast::<u16>().add(offset)) })
                .take_while(|unit| *unit != 0)
                .collect();
            let value = String::from_utf16_lossy(&units).trim().to_owned();
            if !value.is_empty() {
                return Some(value);
            }
        }
        None
    }

    fn in_buffer(buffer: &[u32], pointer: *mut c_void, bytes: usize) -> bool {
        let start = buffer.as_ptr() as usize;
        let end = start.saturating_add(std::mem::size_of_val(buffer));
        let pointer = pointer as usize;
        pointer >= start && pointer <= end && bytes <= end - pointer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(pid: u32, process: &str, title: &str) -> DetectedApp {
        DetectedApp {
            pid,
            process: process.into(),
            title: title.into(),
            name: None,
        }
    }

    #[test]
    fn filters_self_legacy_hosts_empty_windows_and_duplicate_processes() {
        let apps = normalize_apps(
            vec![
                app(99, "z.exe", "Last"),
                app(10, "rust-app.exe", "Self"),
                app(11, "Electron.exe", "Legacy"),
                app(12, "Presence Control.exe", "Legacy"),
                app(13, "A.exe", "First"),
                app(13, "A.exe", "Second window"),
                app(14, "Background.exe", ""),
                app(15, "", "Unknown process"),
                app(0, "System.exe", "Unknown PID"),
            ],
            10,
        );
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].process, "A.exe");
        assert_eq!(apps[0].title, "First");
        assert_eq!(apps[1].process, "z.exe");
    }

    #[test]
    fn preserves_unicode_titles_product_names_and_case() {
        let mut original = app(42, "Code.exe", "日本語 🎵");
        original.name = Some("Visual Studio Code".into());
        let apps = normalize_apps(vec![original.clone()], 10);
        assert_eq!(apps, [original]);
        assert!(normalize_apps(Vec::new(), 10).is_empty());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn native_scan_returns_sorted_unique_windowed_processes_without_self() {
        let apps = detect_apps().await.unwrap();
        let mut seen = std::collections::HashSet::new();
        for app in &apps {
            assert_ne!(app.pid, std::process::id());
            assert!(seen.insert(app.pid));
            assert!(!app.title.is_empty());
            assert!(app.process.to_lowercase().ends_with(".exe"));
            assert!(!app.name.as_deref().unwrap_or_default().is_empty());
        }
        assert!(apps.windows(2).all(|pair| {
            (pair[0].process.to_lowercase(), pair[0].pid)
                <= (pair[1].process.to_lowercase(), pair[1].pid)
        }));
    }

    #[cfg(windows)]
    #[test]
    fn current_process_path_uses_native_api() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        fn identity(path: &std::path::Path) -> (u32, u32, u32) {
            let file = std::fs::File::open(path).unwrap();
            let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
            // SAFETY: file owns a live Windows file handle and info is writable
            // storage for the structure. The API initializes it on success.
            assert_ne!(
                unsafe {
                    GetFileInformationByHandle(file.as_raw_handle().cast(), info.as_mut_ptr())
                },
                0
            );
            let info = unsafe { info.assume_init() };
            (
                info.dwVolumeSerialNumber,
                info.nFileIndexHigh,
                info.nFileIndexLow,
            )
        }
        let detected = windows::process_path(std::process::id()).unwrap();
        let expected = std::env::current_exe().unwrap();
        // Packaged Windows apps can expose virtualized and physical paths for
        // the same executable. Compare the opened file's identity, not spelling.
        assert_eq!(
            identity(std::path::Path::new(&detected)),
            identity(&expected)
        );
    }
}
