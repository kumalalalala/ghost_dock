//! Single-instance lock and cross-process tray-wakeup signaling (UIPI-safe).

use std::ffi::c_void;
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HLOCAL,
    LocalFree, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
    WaitForMultipleObjects,
};
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
use windows::core::PCWSTR;

const DEFAULT_MUTEX_NAME: &str = "Local\\GhostDock_SingleInstance_Mutex_v1";
const DEFAULT_EVENT_NAME: &str = "Local\\GhostDock_ShowDashboard_Event_v1";

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct SecurityDescGuard(PSECURITY_DESCRIPTOR);

impl Drop for SecurityDescGuard {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0.0)));
            }
        }
    }
}

/// Build a same-session Authenticated-Users + Low-Integrity SACL so Medium and Admin
/// instances can synchronize seamlessly across UIPI boundaries.
fn build_shared_security_attributes() -> Option<(SecurityDescGuard, SECURITY_ATTRIBUTES)> {
    let sddl = to_wide("D:(A;;GA;;;AU)S:(ML;;NW;;;LW)");
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            1, // SDDL_REVISION_1
            &mut sd,
            None,
        )
        .is_ok()
        {
            let sa = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            };
            return Some((SecurityDescGuard(sd), sa));
        }
    }
    None
}

pub enum InstanceAcquisition {
    /// We hold the primary lock; call `start_listener` to wake up on secondary launches.
    Primary(SingleInstanceGuard),
    /// Another instance is already running and has been signaled to show from tray.
    SecondarySignaled,
}

pub struct SingleInstanceGuard {
    mutex_handle: HANDLE,
    show_event: HANDLE,
    stop_event: HANDLE,
    listener: Option<JoinHandle<()>>,
}

impl SingleInstanceGuard {
    /// Start a lightweight background thread invoking `on_wake` whenever another instance launches.
    pub fn start_listener<F>(&mut self, on_wake: F)
    where
        F: Fn() + Send + 'static,
    {
        if self.listener.is_some() || self.show_event.is_invalid() || self.stop_event.is_invalid() {
            return;
        }
        let raw_show = self.show_event.0 as isize;
        let raw_stop = self.stop_event.0 as isize;
        self.listener = Some(thread::spawn(move || {
            let handles = [
                HANDLE(raw_show as *mut c_void),
                HANDLE(raw_stop as *mut c_void),
            ];
            loop {
                let res = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                if res == WAIT_OBJECT_0 {
                    on_wake();
                } else {
                    break;
                }
            }
        }));
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if !self.stop_event.is_invalid() {
            unsafe {
                let _ = SetEvent(self.stop_event);
            }
        }
        if let Some(handle) = self.listener.take() {
            let _ = handle.join();
        }
        unsafe {
            if !self.show_event.is_invalid() {
                let _ = CloseHandle(self.show_event);
            }
            if !self.stop_event.is_invalid() {
                let _ = CloseHandle(self.stop_event);
            }
            if !self.mutex_handle.is_invalid() {
                let _ = CloseHandle(self.mutex_handle);
            }
        }
    }
}

/// Acquire the global GhostDock single-instance lock, or signal the existing instance to show.
pub fn acquire_or_wake_existing() -> InstanceAcquisition {
    acquire_or_wake_named(DEFAULT_MUTEX_NAME, DEFAULT_EVENT_NAME)
}

/// Parameterized helper for isolated tests and production single-instance lock.
pub fn acquire_or_wake_named(mutex_name: &str, event_name: &str) -> InstanceAcquisition {
    let mutex_w = to_wide(mutex_name);
    let event_w = to_wide(event_name);
    let sec = build_shared_security_attributes();
    let sa_ptr = sec
        .as_ref()
        .map(|(_, sa)| sa as *const SECURITY_ATTRIBUTES);

    unsafe {
        let mutex_res = CreateMutexW(sa_ptr, false, PCWSTR(mutex_w.as_ptr()));
        let last_err = GetLastError();

        let already_running = match &mutex_res {
            Ok(_) => last_err == ERROR_ALREADY_EXISTS,
            Err(_) => last_err == ERROR_ACCESS_DENIED || last_err == ERROR_ALREADY_EXISTS,
        };

        if already_running {
            if let Ok(h) = mutex_res
                && !h.is_invalid()
            {
                let _ = CloseHandle(h);
            }
            let _ = AllowSetForegroundWindow(ASFW_ANY);
            if let Ok(ev) = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(event_w.as_ptr())) {
                let _ = SetEvent(ev);
                let _ = CloseHandle(ev);
            }
            return InstanceAcquisition::SecondarySignaled;
        }

        let mutex_handle = mutex_res.unwrap_or_default();
        let show_event =
            CreateEventW(sa_ptr, false, false, PCWSTR(event_w.as_ptr())).unwrap_or_default();
        let stop_event = CreateEventW(None, true, false, PCWSTR::null()).unwrap_or_default();

        InstanceAcquisition::Primary(SingleInstanceGuard {
            mutex_handle,
            show_event,
            stop_event,
            listener: None,
        })
    }
}
