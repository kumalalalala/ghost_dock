//! Win32 window discovery, stealth hiding, monitor routing, and elevation helpers.

pub mod controller;
pub mod enumerator;
pub mod guard;
pub mod monitors;

use windows::Win32::Foundation::{CloseHandle, HANDLE};

/// Strategy used when hiding a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HideStrategy {
    /// Default: `WS_EX_NOACTIVATE` + `ITaskbarList::DeleteTab` + locked 1px at bottom-right
    /// monitor corner + real-time `SetWinEventHook` Z-order & Win+D guard.
    /// Hidden from Desktop, Taskbar, and Alt+Tab while staying pickable in OBS / TikTok (`!WS_EX_TOOLWINDOW`).
    CornerPeek,
    /// `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE` with `LWA_ALPHA = 0` + `DeleteTab` + `HWND_BOTTOM`.
    /// 100% invisible & click-through even on empty Desktop; compatible with `BitBlt` / `PrintWindow` capture.
    LayeredGhost,
    /// `WS_EX_NOACTIVATE` + `DeleteTab` + real-time `HWND_BOTTOM` pin keeping full on-screen bounds.
    /// 100% compatible with WGC (`Windows.Graphics.Capture`), `BitBlt`, and OBS/TikTok pickers behind active apps.
    TaskbarCapturable,
    /// Strict legacy stealth: adds `WS_EX_TOOLWINDOW` + `WS_EX_NOACTIVATE` + `DeleteTab` + `HWND_BOTTOM`.
    StealthBackground,
    /// Relocate window onto a secondary / virtual (`IddCx`) monitor un-occluded,
    /// hiding its Taskbar tab and preventing focus activation while staying pickable in OBS / TikTok.
    SecondaryMonitor,
}

impl HideStrategy {
    pub fn hides_from_alt_tab(self) -> bool {
        matches!(self, Self::StealthBackground)
    }

    pub fn is_relocated(self) -> bool {
        matches!(self, Self::CornerPeek | Self::SecondaryMonitor)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CornerPeek => "1px (Desktop-free)",
            Self::LayeredGhost => "Ghost (Alpha 0)",
            Self::TaskbarCapturable => "Live (WGC OK)",
            Self::StealthBackground => "Stealth (Tool)",
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
