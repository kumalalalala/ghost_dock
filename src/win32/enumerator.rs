//! Strict Alt+Tab-accurate Win32 window enumeration grouped by owning process with PID cache.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::c_void;

use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, OpenProcessToken, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetWindow, GetWindowLongPtrW,
    GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PWSTR};

use super::{HideStrategy, OwnedHandle, ProcessGroup, WindowEntry, WindowState};

#[derive(Clone)]
pub struct ProcMeta {
    pub exe_name: String,
    pub exe_path: String,
    pub is_elevated: bool,
    creation_time: u64,
}

/// Persistent per-PID metadata cache avoiding repeated OpenProcess/OpenProcessToken syscalls.
#[derive(Default)]
pub struct ProcessMetaCache {
    entries: HashMap<u32, ProcMeta>,
}

impl ProcessMetaCache {
    fn get_or_query(&mut self, pid: u32) -> ProcMeta {
        if let Some(cached) = self.entries.get(&pid) {
            return cached.clone();
        }
        let queried = query_process_meta(pid);
        self.entries.insert(pid, queried.clone());
        queried
    }

    /// Prune exited PIDs and verify active PIDs were not recycled by Windows.
    fn retain_active(&mut self, active_pids: &HashSet<u32>) {
        self.entries.retain(|pid, meta| {
            if !active_pids.contains(pid) {
                return false;
            }
            if meta.creation_time == 0 {
                return true;
            }
            read_process_creation_time(*pid).is_none_or(|t| t == meta.creation_time)
        });
    }
}

const IGNORED_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "DV2ControlHost",
    "MsgrIMEWindowClass",
    "SysShadow",
    "Button",
];

fn filetime_to_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

fn read_process_creation_time(pid: u32) -> Option<u64> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let guard = OwnedHandle(h);
        let (mut c, mut e, mut k, mut u) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        GetProcessTimes(guard.0, &mut c, &mut e, &mut k, &mut u).ok()?;
        Some(filetime_to_u64(c))
    }
}

fn query_process_meta(pid: u32) -> ProcMeta {
    unsafe {
        let Ok(proc_handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return ProcMeta {
                exe_name: format!("PID {pid}"),
                exe_path: String::new(),
                is_elevated: false,
                creation_time: 0,
            };
        };
        let proc_guard = OwnedHandle(proc_handle);

        let (mut c_ft, mut e_ft, mut k_ft, mut u_ft) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let creation_time =
            if GetProcessTimes(proc_guard.0, &mut c_ft, &mut e_ft, &mut k_ft, &mut u_ft).is_ok() {
                filetime_to_u64(c_ft)
            } else {
                0
            };

        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let exe_path = if QueryFullProcessImageNameW(
            proc_guard.0,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok()
        {
            String::from_utf16_lossy(&buf[..len as usize])
        } else {
            String::new()
        };

        let exe_name = exe_path
            .rsplit(['\\', '/'])
            .next()
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("PID {pid}"));

        let mut is_elevated = false;
        let mut token_handle = windows::Win32::Foundation::HANDLE::default();
        if OpenProcessToken(proc_guard.0, TOKEN_QUERY, &mut token_handle).is_ok() {
            let token_guard = OwnedHandle(token_handle);
            let mut elevation = TOKEN_ELEVATION::default();
            let mut ret_len = 0u32;
            if GetTokenInformation(
                token_guard.0,
                TokenElevation,
                Some(&mut elevation as *mut TOKEN_ELEVATION as *mut c_void),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut ret_len,
            )
            .is_ok()
            {
                is_elevated = elevation.TokenIsElevated != 0;
            }
        }

        ProcMeta {
            exe_name,
            exe_path,
            is_elevated,
            creation_time,
        }
    }
}

/// Check if the current GhostDock process itself is running elevated (Administrator).
pub fn is_current_process_elevated() -> bool {
    let pid = std::process::id();
    query_process_meta(pid).is_elevated
}

fn read_window_text_buf(hwnd: HWND, buf: &mut [u16]) -> String {
    unsafe {
        let copied = GetWindowTextW(hwnd, buf);
        if copied <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..copied as usize])
    }
}

fn classify_window_buf(hwnd: HWND, buf: &mut [u16]) -> (bool, bool) {
    unsafe {
        let len = GetClassNameW(hwnd, buf);
        if len <= 0 {
            return (false, false);
        }
        let slice = &buf[..len as usize];
        let is_stream_timer = {
            let mut utf16 = "StreamTimerWindowClass".encode_utf16();
            slice.iter().all(|&c| utf16.next() == Some(c)) && utf16.next().is_none()
        };
        let ignored = IGNORED_CLASSES.iter().any(|cls| {
            let mut utf16 = cls.encode_utf16();
            slice.iter().all(|&c| utf16.next() == Some(c)) && utf16.next().is_none()
        });
        (ignored, is_stream_timer)
    }
}

fn is_dwm_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        )
    };
    hr.is_ok() && cloaked != 0
}

struct EnumContext<'a> {
    self_pid: u32,
    managed_states: &'a HashMap<isize, HideStrategy>,
    cache: &'a mut ProcessMetaCache,
    active_pids: HashSet<u32>,
    groups: BTreeMap<(String, u32), Vec<WindowEntry>>,
    text_buf: [u16; 512],
    class_buf: [u16; 128],
}

unsafe extern "system" fn enum_window_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = unsafe { &mut *(lparam.0 as *mut EnumContext<'_>) };
    let hwnd_raw = hwnd.0 as isize;
    let managed_strategy = ctx.managed_states.get(&hwnd_raw).copied();

    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == ctx.self_pid {
            return BOOL(1);
        }

        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }

        if managed_strategy.is_none() && is_dwm_cloaked(hwnd) {
            return BOOL(1);
        }

        let (is_ignored_class, is_stream_timer) = classify_window_buf(hwnd, &mut ctx.class_buf);
        if is_ignored_class {
            return BOOL(1);
        }

        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if managed_strategy.is_none() && !is_stream_timer {
            if (ex_style & WS_EX_TOOLWINDOW.0) != 0 {
                return BOOL(1);
            }
            let owner = GetWindow(hwnd, GW_OWNER).unwrap_or_default();
            if !owner.is_invalid() && (ex_style & WS_EX_APPWINDOW.0) == 0 {
                return BOOL(1);
            }
            if (ex_style & WS_EX_NOACTIVATE.0) != 0 && (ex_style & WS_EX_APPWINDOW.0) == 0 {
                return BOOL(1);
            }
        }

        let title = read_window_text_buf(hwnd, &mut ctx.text_buf);
        if title.trim().is_empty() {
            return BOOL(1);
        }

        let is_minimized = IsIconic(hwnd).as_bool();
        if !is_minimized {
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_ok() {
                let w = rect.right - rect.left;
                let h = rect.bottom - rect.top;
                if w < 32 || h < 24 {
                    return BOOL(1);
                }
            }
        }

        ctx.active_pids.insert(pid);
        let meta = ctx.cache.get_or_query(pid);

        let state = if let Some(strategy) = managed_strategy {
            WindowState::StealthHidden(strategy)
        } else if is_minimized {
            WindowState::Minimized
        } else {
            WindowState::Visible
        };

        let key = (meta.exe_name.to_ascii_lowercase(), pid);
        ctx.groups.entry(key).or_default().push(WindowEntry {
            hwnd_raw,
            title,
            state,
        });
    }

    BOOL(1)
}

/// Cached Alt+Tab window enumeration reusing process metadata across ticks.
pub fn enumerate_window_groups_cached(
    managed_states: &HashMap<isize, HideStrategy>,
    cache: &mut ProcessMetaCache,
) -> Vec<ProcessGroup> {
    let mut ctx = EnumContext {
        self_pid: std::process::id(),
        managed_states,
        cache,
        active_pids: HashSet::new(),
        groups: BTreeMap::new(),
        text_buf: [0u16; 512],
        class_buf: [0u16; 128],
    };

    unsafe {
        let _ = EnumWindows(
            Some(enum_window_proc),
            LPARAM(&mut ctx as *mut EnumContext<'_> as isize),
        );
    }

    let active_pids = ctx.active_pids;
    let groups = ctx.groups;
    cache.retain_active(&active_pids);

    groups
        .into_iter()
        .map(|((_sort_name, pid), mut windows)| {
            windows.sort_by_key(|w| w.hwnd_raw);
            let meta = cache.get_or_query(pid);
            ProcessGroup {
                pid,
                exe_name: meta.exe_name,
                exe_path: meta.exe_path,
                is_elevated: meta.is_elevated,
                windows,
            }
        })
        .collect()
}

/// Convenience one-shot snapshot without persistent caller cache.
pub fn enumerate_window_groups(managed_states: &HashMap<isize, HideStrategy>) -> Vec<ProcessGroup> {
    let mut ephemeral = ProcessMetaCache::default();
    enumerate_window_groups_cached(managed_states, &mut ephemeral)
}
