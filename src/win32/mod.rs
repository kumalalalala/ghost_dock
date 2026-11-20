//! Win32 window discovery, stealth hiding, monitor routing, and elevation helpers.

pub mod controller;
pub mod enumerator;
pub mod monitors;

use windows::Win32::Foundation::{CloseHandle, HANDLE};

/// Strategy used when hiding a window from Alt+Tab and Taskbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HideStrategy {
    /// Remove from Alt+Tab & Taskbar (`WS_EX_TOOLWINDOW`) and push to `HWND_BOTTOM`.
    /// Window stays `WS_VISIBLE` and un-minimized (`IsIconic == 0`).
    StealthBackground,
    /// Same as `StealthBackground`, plus tuck to 1px corner on primary/virtual edge.
    CornerPeek,
    /// Relocate window onto a secondary / virtual (`IddCx`) monitor un-occluded,
    /// while also removing it from Alt+Tab & Taskbar.
    SecondaryMonitor,
}

impl HideStrategy {
    pub fn label(self) -> &'static str {
        match self {
            Self::StealthBackground => "Stealth",
            Self::CornerPeek => "Corner 1px",
            Self::SecondaryMonitor => "Monitor 2",
        }
    }
}

/// Current visibility/capture state of a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Visible,
    Minimized,
    StealthHidden(HideStrategy),
}

/// Metadata for a single user-facing top-level window.
#[derive(Debug, Clone)]
pub struct WindowEntry {
    pub hwnd_raw: isize,
    pub title: String,
    pub state: WindowState,
}

/// Windows grouped by owning process.
#[derive(Debug, Clone)]
pub struct ProcessGroup {
    pub pid: u32,
    pub exe_name: String,
    pub exe_path: String,
    pub is_elevated: bool,
    pub windows: Vec<WindowEntry>,
}

impl ProcessGroup {
    pub fn hidden_count(&self) -> usize {
        self.windows
            .iter()
            .filter(|w| matches!(w.state, WindowState::StealthHidden(_)))
            .count()
    }
}

/// RAII wrapper ensuring Win32 `HANDLE` is always closed.
pub(crate) struct OwnedHandle(pub HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}
