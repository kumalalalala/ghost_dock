//! Display monitor enumeration for Secondary/Virtual Monitor and Corner-Peek positioning.

use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::core::BOOL;

#[derive(Debug, Clone, Copy)]
pub struct MonitorRect {
    pub bounds: RECT,
    pub work_area: RECT,
    pub is_primary: bool,
}

const MONITORINFOF_PRIMARY: u32 = 1;

unsafe extern "system" fn enum_monitor_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _lprc: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let out = unsafe { &mut *(lparam.0 as *mut Vec<MonitorRect>) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(hmon, &mut info).as_bool() } {
        out.push(MonitorRect {
            bounds: info.rcMonitor,
            work_area: info.rcWork,
            is_primary: (info.dwFlags & MONITORINFOF_PRIMARY) != 0,
        });
    }
    BOOL(1)
}

/// Enumerate all connected physical and virtual (`IddCx`) monitors.
pub fn list_monitors() -> Vec<MonitorRect> {
    let mut monitors = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(enum_monitor_proc),
            LPARAM(&mut monitors as *mut Vec<MonitorRect> as isize),
        );
    }
    monitors
}

/// Return the work-area rectangle of the first non-primary monitor, if available.
pub fn find_secondary_monitor() -> Option<MonitorRect> {
    list_monitors().into_iter().find(|m| !m.is_primary)
}

/// Find a 1px corner-peek coordinate on the outer bottom-right edge of all monitors.
pub fn corner_peek_position() -> (i32, i32) {
    let monitors = list_monitors();
    if let Some(best) = monitors
        .into_iter()
        .max_by_key(|m| m.bounds.right + m.bounds.bottom)
    {
        (best.bounds.right - 1, best.bounds.bottom - 1)
    } else {
        (1919, 1079)
    }
}
