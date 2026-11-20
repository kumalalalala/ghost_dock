//! Win32 window mutation engine with state preservation, style guard, and zero-orphan restore.

use std::collections::HashMap;
use std::ffi::c_void;

use windows::Win32::Foundation::{
    COLORREF, ERROR_ACCESS_DENIED, GetLastError, HWND, RECT, S_OK, SetLastError,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Shell::{ITaskbarList, ShellExecuteW, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, GetWindowPlacement,
    GetWindowRect, HWND_BOTTOM, HWND_TOP, IsIconic, IsWindow, IsZoomed, LWA_ALPHA, SW_MINIMIZE,
    SW_RESTORE, SW_SHOWMAXIMIZED, SW_SHOWNOACTIVATE, SW_SHOWNORMAL, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetForegroundWindow,
    SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOWPLACEMENT,
    WS_EX_LAYERED,
};
use windows::core::{PCWSTR, w};

use super::HideStrategy;
use super::guard::{GuardRule, WinEventGuard, compute_hidden_ex_style, unmaximize_no_activate};
use super::monitors::{corner_peek_position, find_secondary_monitor};

#[derive(Debug, Clone)]
struct SavedWindowState {
    orig_ex_style: isize,
    orig_rect: RECT,
    was_maximized: bool,
    relocated: bool,
    strategy: HideStrategy,
}

#[derive(Default)]
pub struct WindowController {
    saved: HashMap<isize, SavedWindowState>,
    guard: WinEventGuard,
}

impl Drop for WindowController {
    fn drop(&mut self) {
        self.restore_all();
    }
}

fn to_hwnd(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Show or hide an HWND's Taskbar button via Windows Shell `ITaskbarList`
/// without altering `WS_EX_TOOLWINDOW` (preserving OBS / TikTok Window Capture discovery).
pub fn try_taskbar_tab(hwnd: HWND, show: bool) -> bool {
    unsafe {
        let created: windows::core::Result<ITaskbarList> =
            CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER);
        let (tb, should_uninit) = match created {
            Ok(tb) => (tb, false),
            Err(_) => {
                let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let did_init = hr == S_OK;
                match CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) {
                    Ok(tb) => (tb, did_init),
                    Err(_) => {
                        if did_init {
                            CoUninitialize();
                        }
                        return false;
                    }
                }
            }
        };

        let _ = tb.HrInit();
        let res = if show {
            tb.AddTab(hwnd)
        } else {
            tb.DeleteTab(hwnd)
        };
        drop(tb);
        if should_uninit {
            CoUninitialize();
        }
        res.is_ok()
    }
}

/// Apply `#0B0B0C` obsidian DWM titlebar and `#222227` border to GhostDock's own window.
pub fn style_own_window_dark(hwnd_raw: isize) {
    if hwnd_raw == 0 {
        return;
    }
    let hwnd = to_hwnd(hwnd_raw);
    let dark: u32 = 1;
    let caption_bgr: u32 = 0x000C_0B0B; // #0B0B0C
    let border_bgr: u32 = 0x0027_2222; // #222227
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const u32 as *const c_void,
            size_of::<u32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &caption_bgr as *const u32 as *const c_void,
            size_of::<u32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &border_bgr as *const u32 as *const c_void,
            size_of::<u32>() as u32,
        );
    }
}

/// Relaunch GhostDock with UAC elevation (`runas`).
pub fn relaunch_elevated() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Cannot resolve exe path: {e}"))?;
    let exe_w = to_wide(&exe.to_string_lossy());
    unsafe {
        let res = ShellExecuteW(
            None,
            w!("runas"),
            PCWSTR(exe_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if (res.0 as isize) <= 32 {
            return Err("UAC elevation was cancelled or failed.".to_owned());
        }
    }
    Ok(())
}

/// Relaunch an external Chromium/Electron executable with occlusion-throttling disabled.
pub fn launch_without_occlusion(exe_path: &str) -> Result<(), String> {
    if exe_path.is_empty() {
        return Err("Executable path unavailable.".to_owned());
    }
    let exe_w = to_wide(exe_path);
    let args_w = to_wide(
        "--disable-features=CalculateNativeWinOcclusion --disable-backgrounding-occluded-windows --disable-renderer-backgrounding",
    );
    unsafe {
        let res = ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(exe_w.as_ptr()),
            PCWSTR(args_w.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if (res.0 as isize) <= 32 {
            return Err("Failed to launch process with anti-occlusion flags.".to_owned());
        }
    }
    Ok(())
}

impl WindowController {
    pub fn managed_map(&self) -> HashMap<isize, HideStrategy> {
        self.saved
            .iter()
            .map(|(&hwnd, st)| (hwnd, st.strategy))
            .collect()
    }

    pub fn hidden_count(&self) -> usize {
        self.saved.len()
    }

    /// Hide a window from Alt+Tab and Taskbar while keeping its render surface live.
    pub fn hide_window(
        &mut self,
        hwnd_raw: isize,
        requested_strategy: HideStrategy,
    ) -> Result<HideStrategy, String> {
        let hwnd = to_hwnd(hwnd_raw);
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                self.saved.remove(&hwnd_raw);
                self.guard.remove(hwnd_raw);
                return Err("Window no longer exists.".to_owned());
            }

            // If OS-minimized, wake its DWM surface without stealing focus first.
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }

            let was_maximized = IsZoomed(hwnd).as_bool();
            let mut orig_rect = RECT::default();
            if was_maximized {
                let mut wp = WINDOWPLACEMENT {
                    length: size_of::<WINDOWPLACEMENT>() as u32,
                    ..Default::default()
                };
                if GetWindowPlacement(hwnd, &mut wp).is_ok() {
                    orig_rect = wp.rcNormalPosition;
                } else {
                    let _ = GetWindowRect(hwnd, &mut orig_rect);
                }
            } else {
                let _ = GetWindowRect(hwnd, &mut orig_rect);
            }
            let current_ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);

            let entry = self
                .saved
                .entry(hwnd_raw)
                .or_insert_with(|| SavedWindowState {
                    orig_ex_style: current_ex,
                    orig_rect,
                    was_maximized,
                    relocated: false,
                    strategy: requested_strategy,
                });
            let orig_ex_style = entry.orig_ex_style;

            // Remove temporarily while mutating styles/coords to prevent echo inside WinEventGuard.
            self.guard.remove(hwnd_raw);

            let mut actual_strategy = requested_strategy;
            let mut new_ex = compute_hidden_ex_style(orig_ex_style, current_ex, actual_strategy);
            if new_ex != current_ex {
                SetLastError(windows::Win32::Foundation::WIN32_ERROR(0));
                let prev = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex);
                let err = GetLastError();
                if prev == 0 && err == ERROR_ACCESS_DENIED {
                    self.saved.remove(&hwnd_raw);
                    return Err(
                        "Access denied (target runs as Admin — elevate GhostDock).".to_owned(),
                    );
                }

                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }

            if actual_strategy == HideStrategy::LayeredGhost
                && SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA).is_err()
            {
                actual_strategy = HideStrategy::CornerPeek;
                new_ex = compute_hidden_ex_style(orig_ex_style, new_ex, actual_strategy);
                let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }

            let _ = try_taskbar_tab(hwnd, false);

            let mut expected_pos: Option<(i32, i32)> = None;
            match actual_strategy {
                HideStrategy::SecondaryMonitor => {
                    if let Some(mon) = find_secondary_monitor() {
                        unmaximize_no_activate(hwnd);
                        let wa = mon.work_area;
                        let mw = (wa.right - wa.left).max(320);
                        let mh = (wa.bottom - wa.top).max(240);
                        let cur_w = (entry.orig_rect.right - entry.orig_rect.left)
                            .clamp(320, mw);
                        let cur_h = (entry.orig_rect.bottom - entry.orig_rect.top)
                            .clamp(240, mh);
                        let tx = wa.left + (mw - cur_w) / 2;
                        let ty = wa.top + (mh - cur_h) / 2;
                        if SetWindowPos(
                            hwnd,
                            Some(HWND_TOP),
                            tx,
                            ty,
                            cur_w,
                            cur_h,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        )
                        .is_ok()
                        {
                            entry.relocated = true;
                            expected_pos = Some((tx, ty));
                        } else {
                            actual_strategy = HideStrategy::CornerPeek;
                            unmaximize_no_activate(hwnd);
                            let (px, py) = corner_peek_position();
                            if SetWindowPos(
                                hwnd,
                                Some(HWND_BOTTOM),
                                px,
                                py,
                                0,
                                0,
                                SWP_NOSIZE | SWP_NOACTIVATE,
                            )
                            .is_ok()
                            {
                                entry.relocated = true;
                                expected_pos = Some((px, py));
                            }
                        }
                    } else {
                        actual_strategy = HideStrategy::CornerPeek;
                        unmaximize_no_activate(hwnd);
                        let (px, py) = corner_peek_position();
                        if SetWindowPos(
                            hwnd,
                            Some(HWND_BOTTOM),
                            px,
                            py,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOACTIVATE,
                        )
                        .is_ok()
                        {
                            entry.relocated = true;
                            expected_pos = Some((px, py));
                        }
                    }
                }
                HideStrategy::CornerPeek => {
                    unmaximize_no_activate(hwnd);
                    let (px, py) = corner_peek_position();
                    if SetWindowPos(
                        hwnd,
                        Some(HWND_BOTTOM),
                        px,
                        py,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE,
                    )
                    .is_ok()
                    {
                        entry.relocated = true;
                        expected_pos = Some((px, py));
                    }
                }
                HideStrategy::LayeredGhost
                | HideStrategy::TaskbarCapturable
                | HideStrategy::StealthBackground => {
                    if entry.relocated {
                        let r = entry.orig_rect;
                        let _ = SetWindowPos(
                            hwnd,
                            Some(HWND_BOTTOM),
                            r.left,
                            r.top,
                            (r.right - r.left).max(120),
                            (r.bottom - r.top).max(80),
                            SWP_NOACTIVATE,
                        );
                        entry.relocated = false;
                    } else {
                        let _ = SetWindowPos(
                            hwnd,
                            Some(HWND_BOTTOM),
                            0,
                            0,
                            0,
                            0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                    }
                }
            }

            entry.strategy = actual_strategy;
            self.guard.upsert(
                hwnd_raw,
                GuardRule {
                    strategy: actual_strategy,
                    orig_ex_style,
                    expected_pos,
                },
            );
            if GetForegroundWindow() == hwnd {
                self.guard.sweep_now(false);
            }
            Ok(actual_strategy)
        }
    }

    /// Restore a window back to Alt+Tab, Taskbar, original position, and foreground.
    pub fn show_window(&mut self, hwnd_raw: isize, focus: bool) -> Result<(), String> {
        let hwnd = to_hwnd(hwnd_raw);
        self.guard.remove(hwnd_raw);
        let saved = self.saved.remove(&hwnd_raw);

        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                return Ok(());
            }

            let mut restored_max = false;
            if let Some(st) = saved {
                restored_max = st.was_maximized;
                if st.strategy == HideStrategy::LayeredGhost
                    && (st.orig_ex_style & (WS_EX_LAYERED.0 as isize)) != 0
                {
                    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
                }

                SetLastError(windows::Win32::Foundation::WIN32_ERROR(0));
                let prev = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, st.orig_ex_style);
                let err = GetLastError();
                if prev == 0 && err == ERROR_ACCESS_DENIED {
                    self.guard.upsert(
                        hwnd_raw,
                        GuardRule {
                            strategy: st.strategy,
                            orig_ex_style: st.orig_ex_style,
                            expected_pos: None,
                        },
                    );
                    self.saved.insert(hwnd_raw, st);
                    return Err("Access denied while restoring window.".to_owned());
                }

                if st.relocated {
                    let r = st.orig_rect;
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOP),
                        r.left,
                        r.top,
                        (r.right - r.left).max(120),
                        (r.bottom - r.top).max(80),
                        SWP_FRAMECHANGED | SWP_SHOWWINDOW | if focus { Default::default() } else { SWP_NOACTIVATE },
                    );
                } else {
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOP),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE
                            | SWP_NOSIZE
                            | SWP_FRAMECHANGED
                            | SWP_SHOWWINDOW
                            | if focus { Default::default() } else { SWP_NOACTIVATE },
                    );
                }
            }

            let _ = try_taskbar_tab(hwnd, true);

            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            } else if restored_max {
                let prev_fg = if !focus {
                    Some(GetForegroundWindow())
                } else {
                    None
                };
                let _ = ShowWindow(hwnd, SW_SHOWMAXIMIZED);
                if let Some(fg) = prev_fg
                    && !fg.is_invalid()
                    && fg != hwnd
                    && IsWindow(Some(fg)).as_bool()
                {
                    let _ = SetForegroundWindow(fg);
                }
            }

            if focus {
                let _ = BringWindowToTop(hwnd);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
                let _ = SetForegroundWindow(hwnd);
            }
        }
        Ok(())
    }

    /// Standard OS minimize (`SW_MINIMIZE`). Restores normal Taskbar style first if stealth-hidden.
    pub fn minimize_window(&mut self, hwnd_raw: isize) -> Result<(), String> {
        let hwnd = to_hwnd(hwnd_raw);
        if self.saved.contains_key(&hwnd_raw) {
            self.show_window(hwnd_raw, false)?;
        }
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                return Ok(());
            }
            if !ShowWindow(hwnd, SW_MINIMIZE).as_bool() {
                // ShowWindow returns false if window was already minimized; still check access error.
                if GetLastError() == ERROR_ACCESS_DENIED {
                    return Err("Access denied (target runs as Admin).".to_owned());
                }
            }
        }
        Ok(())
    }

    /// Periodic fallback guard: prune closed HWNDs and sweep all managed windows via WinEventGuard.
    pub fn enforce_stealth_guards(&mut self) {
        let keys: Vec<isize> = self.saved.keys().copied().collect();
        for hwnd_raw in keys {
            let hwnd = to_hwnd(hwnd_raw);
            unsafe {
                if !IsWindow(Some(hwnd)).as_bool() {
                    self.saved.remove(&hwnd_raw);
                    self.guard.remove(hwnd_raw);
                }
            }
        }
        self.guard.sweep_now(false);
    }

    /// Restore all windows currently stealth-hidden by GhostDock.
    pub fn restore_all(&mut self) {
        let hwnds: Vec<isize> = self.saved.keys().copied().collect();
        for hwnd_raw in hwnds {
            let _ = self.show_window(hwnd_raw, false);
        }
    }
}
