//! Small native host services. No command shell or script runtime is involved.
use std::path::PathBuf;

/// Keep the first window and its title bar inside the usable desktop. The studio's
/// preferred 1470x1000 client area otherwise extends below a 1080p display when
/// Windows chooses a nonzero initial position.
#[cfg(windows)]
pub fn place_initial_window(window: &slint::Window) {
    use windows_sys::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA},
    };
    let mut work = std::mem::MaybeUninit::<RECT>::uninit();
    // SAFETY: SPI_GETWORKAREA initializes the writable RECT on success. No
    // pointers are retained; the request only reads the primary desktop bounds.
    if unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, work.as_mut_ptr().cast(), 0) } == 0 {
        return;
    }
    let work = unsafe { work.assume_init() };
    let work_width = work.right.saturating_sub(work.left).max(1) as u32;
    let work_height = work.bottom.saturating_sub(work.top).max(1) as u32;
    let scale = window.scale_factor();
    // Leave horizontal margins and extra vertical room for native title bar,
    // borders, and placement margins; the work area already excludes the taskbar.
    let width = ((1470.0 * scale).round() as u32).min(work_width.saturating_sub(32).max(1));
    let height = ((1000.0 * scale).round() as u32).min(work_height.saturating_sub(64).max(1));
    let x = work.left.saturating_add(((work_width - width) / 2) as i32);
    let y = work.top.saturating_add(((work_height - height) / 2) as i32);
    window.set_size(slint::PhysicalSize::new(width, height));
    window.set_position(slint::PhysicalPosition::new(x, y));
}

#[cfg(not(windows))]
pub fn place_initial_window(_: &slint::Window) {}

pub fn data_directory() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("PRESENCE_CONTROL_DATA_DIR") {
        if path.is_empty() {
            return Err("PRESENCE_CONTROL_DATA_DIR cannot be empty.".into());
        }
        return Ok(PathBuf::from(path));
    }
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    base.map(|base| base.join("dev.presence.control.rust.slint"))
        .ok_or_else(|| "Could not locate the application data directory.".into())
}

#[cfg(windows)]
pub struct InstanceGuard {
    mutex: windows_sys::Win32::Foundation::HANDLE,
    activation: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl InstanceGuard {
    pub fn acquire(root: &std::path::Path) -> Result<Option<Self>, String> {
        use std::hash::{Hash, Hasher};
        use windows_sys::Win32::{
            Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS},
            System::Threading::{CreateEventW, CreateMutexW, SetEvent},
        };
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        // A workspace override gets its own instance, useful for isolated verification.
        // The host supplies a canonical directory so path aliases share an instance.
        root.to_string_lossy().to_lowercase().hash(&mut hash);
        let id = hash.finish();
        let name: Vec<u16> = format!("Local\\PresenceControlSlint-{id:x}\0")
            .encode_utf16()
            .collect();
        let event_name: Vec<u16> = format!("Local\\PresenceControlSlintActivate-{id:x}\0")
            .encode_utf16()
            .collect();
        // SAFETY: the NUL-terminated name is valid throughout the call; the returned
        // handle is owned by this guard and released exactly once.
        unsafe {
            let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let already_running = GetLastError() == ERROR_ALREADY_EXISTS;
            // Auto-reset events coalesce requests without creating a waiting
            // thread. The native UI polls this event from its regular timer.
            let activation = CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr());
            if activation.is_null() {
                let error = std::io::Error::last_os_error().to_string();
                CloseHandle(handle);
                return Err(error);
            }
            if already_running {
                SetEvent(activation);
                CloseHandle(activation);
                CloseHandle(handle);
                return Ok(None);
            }
            Ok(Some(Self {
                mutex: handle,
                activation,
            }))
        }
    }

    /// Called on the UI thread's timer. Activation is scoped to this executable's
    /// PID, so a similarly titled app or another workspace cannot receive it.
    pub fn poll_activation(&self) -> bool {
        use windows_sys::Win32::{
            Foundation::{HWND, LPARAM, WAIT_OBJECT_0},
            System::Threading::WaitForSingleObject,
            UI::WindowsAndMessaging::{
                EnumWindows, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
                SetForegroundWindow, ShowWindow, GW_OWNER, SW_RESTORE,
            },
        };
        if unsafe { WaitForSingleObject(self.activation, 0) } != WAIT_OBJECT_0 {
            return false;
        }
        unsafe extern "system" fn activate(window: HWND, parameter: LPARAM) -> i32 {
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(window, &mut pid) };
            if pid == parameter as u32
                && unsafe { IsWindowVisible(window) } != 0
                && unsafe { GetWindow(window, GW_OWNER) }.is_null()
            {
                if unsafe { IsIconic(window) } != 0 {
                    unsafe { ShowWindow(window, SW_RESTORE) };
                }
                unsafe { SetForegroundWindow(window) };
                return 0;
            }
            1
        }
        // SAFETY: the callback receives only a PID value, retains no pointers,
        // and completes synchronously on this UI thread.
        unsafe { EnumWindows(Some(activate), std::process::id() as LPARAM) };
        true
    }
}

#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: successful creation gave this guard ownership of both handles.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.activation);
            windows_sys::Win32::Foundation::CloseHandle(self.mutex);
        }
    }
}

#[cfg(not(windows))]
pub struct InstanceGuard;
#[cfg(not(windows))]
impl InstanceGuard {
    pub fn acquire(_: &std::path::Path) -> Result<Option<Self>, String> {
        Ok(Some(Self))
    }
    pub fn poll_activation(&self) -> bool {
        false
    }
}

pub fn open_developer_portal() -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
        let verb: Vec<u16> = "open\0".encode_utf16().collect();
        let url: Vec<u16> = "https://discord.com/developers/applications\0"
            .encode_utf16()
            .collect();
        // SAFETY: all strings are NUL-terminated and live for the call. Only the
        // fixed Developer Portal URL is opened, never arbitrary editor content.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if result as usize <= 32 {
            Err("Could not open the Discord Developer Portal.".into())
        } else {
            Ok(())
        }
    }
    #[cfg(not(windows))]
    Err("Open https://discord.com/developers/applications in your browser.".into())
}

#[cfg(all(test, windows))]
mod tests {
    use super::InstanceGuard;

    #[test]
    fn same_workspace_signals_activation_and_other_workspaces_are_independent() {
        let first = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let guard = InstanceGuard::acquire(first.path()).unwrap().unwrap();
        assert!(InstanceGuard::acquire(first.path()).unwrap().is_none());
        assert!(guard.poll_activation());
        assert!(!guard.poll_activation());
        let other_guard = InstanceGuard::acquire(other.path()).unwrap().unwrap();
        assert!(!other_guard.poll_activation());
        drop(guard);
        assert!(InstanceGuard::acquire(first.path()).unwrap().is_some());
    }
}
