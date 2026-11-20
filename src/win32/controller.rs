//! Win32 window mutation engine with state preservation, style guard, and zero-orphan restore.

use std::collections::HashMap;
use std::ffi::c_void;

use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, GetLastError, HWND, RECT, SetLastError};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, HWND_BOTTOM, HWND_TOP,
    IsIconic, IsWindow, IsZoomed, SW_MINIMIZE, SW_RESTORE, SW_SHOWMAXIMIZED, SW_SHOWNOACTIVATE,
    SW_SHOWNORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};
use windows::core::{PCWSTR, w};

use super::HideStrategy;
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
                return Err("Window no longer exists.".to_owned());
            }

            // If OS-minimized, wake its DWM surface without stealing focus first.
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }

            let mut orig_rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut orig_rect);
            let current_ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let was_maximized = IsZoomed(hwnd).as_bool();

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

            let new_ex = (current_ex | (WS_EX_TOOLWINDOW.0 as isize))
                & !(WS_EX_APPWINDOW.0 as isize);

            SetLastError(windows::Win32::Foundation::WIN32_ERROR(0));
            let prev = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex);
            let err = GetLastError();
            if prev == 0 && err == ERROR_ACCESS_DENIED {
                self.saved.remove(&hwnd_raw);
                return Err("Access denied (target runs as Admin — elevate GhostDock).".to_owned());
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

            let mut actual_strategy = requested_strategy;
            match requested_strategy {
                HideStrategy::SecondaryMonitor => {
                    if let Some(mon) = find_secondary_monitor() {
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
                        } else {
                            actual_strategy = HideStrategy::StealthBackground;
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
                    } else {
                        actual_strategy = HideStrategy::StealthBackground;
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
                HideStrategy::CornerPeek => {
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
                    }
                }
                HideStrategy::StealthBackground => {
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
            Ok(actual_strategy)
        }
    }

    /// Restore a window back to Alt+Tab, Taskbar, original position, and foreground.
    pub fn show_window(&mut self, hwnd_raw: isize, focus: bool) -> Result<(), String> {
        let hwnd = to_hwnd(hwnd_raw);
        let saved = self.saved.remove(&hwnd_raw);

        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                return Ok(());
            }

            let mut restored_max = false;
            if let Some(st) = saved {
                restored_max = st.was_maximized;
                SetLastError(windows::Win32::Foundation::WIN32_ERROR(0));
                let prev = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, st.orig_ex_style);
                let err = GetLastError();
                if prev == 0 && err == ERROR_ACCESS_DENIED {
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

            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            } else if restored_max && focus {
                let _ = ShowWindow(hwnd, SW_SHOWMAXIMIZED);
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

    /// Periodic guard: prune closed HWNDs and re-apply `WS_EX_TOOLWINDOW` if an app resets it.
    pub fn enforce_stealth_guards(&mut self) {
        let keys: Vec<isize> = self.saved.keys().copied().collect();
        for hwnd_raw in keys {
            let hwnd = to_hwnd(hwnd_raw);
            unsafe {
                if !IsWindow(Some(hwnd)).as_bool() {
                    self.saved.remove(&hwnd_raw);
                    continue;
                }
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                let tool_bit = WS_EX_TOOLWINDOW.0 as isize;
                let app_bit = WS_EX_APPWINDOW.0 as isize;
                if (ex & tool_bit) == 0 || (ex & app_bit) != 0 {
                    let fixed = (ex | tool_bit) & !app_bit;
                    let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, fixed);
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
            }
        }
    }

    /// Restore all windows currently stealth-hidden by GhostDock.
    pub fn restore_all(&mut self) {
        let hwnds: Vec<isize> = self.saved.keys().copied().collect();
        for hwnd_raw in hwnds {
            let _ = self.show_window(hwnd_raw, false);
        }
    }
}
