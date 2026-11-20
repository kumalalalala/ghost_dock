//! Real-time `SetWinEventHook` containment engine preventing Desktop/`Win+D`, focus cascade,
//! display/DPI shifts, Explorer restarts, and self-restoration leaks on hidden windows.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CHILDID_SELF, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_REORDER,
    EVENT_OBJECT_SHOW, EVENT_OBJECT_STATECHANGE, EVENT_SYSTEM_FOREGROUND,
    EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, GW_HWNDNEXT, GW_OWNER, GWL_EXSTYLE,
    GetClassNameW, GetForegroundWindow, GetLayeredWindowAttributes, GetMessageW, GetShellWindow,
    GetWindow, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, GetWindowThreadProcessId,
    HWND_BOTTOM, IsIconic, IsWindow, IsWindowVisible, IsZoomed, KillTimer,
    LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA, MSG, OBJID_WINDOW, PostThreadMessageW,
    RegisterClassW, RegisterWindowMessageW, SW_SHOWNOACTIVATE, SWP_ASYNCWINDOWPOS,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSENDCHANGING, SWP_NOSIZE, SWP_NOZORDER,
    SetForegroundWindow, SetLayeredWindowAttributes, SetTimer, SetWindowLongPtrW,
    SetWindowPlacement, SetWindowPos, ShowWindowAsync, TranslateMessage, WINDOW_EX_STYLE,
    WINDOWPLACEMENT, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_DISPLAYCHANGE, WM_QUIT,
    WM_SETTINGCHANGE, WM_TIMER, WNDCLASSW, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::w;

use super::HideStrategy;
use super::controller::try_taskbar_tab;
use super::monitors::{corner_peek_position, find_secondary_monitor};

#[derive(Debug, Clone, Copy)]
pub struct GuardRule {
    pub strategy: HideStrategy,
    pub orig_ex_style: isize,
    pub expected_pos: Option<(i32, i32)>,
}

struct GuardShared {
    rules: Mutex<HashMap<isize, GuardRule>>,
    last_unmanaged_fg: AtomicIsize,
    thread_id: AtomicU32,
}

static ACTIVE_GUARDS: OnceLock<Mutex<Vec<Weak<GuardShared>>>> = OnceLock::new();

fn active_guards_slot() -> &'static Mutex<Vec<Weak<GuardShared>>> {
    ACTIVE_GUARDS.get_or_init(|| Mutex::new(Vec::new()))
}

fn live_guards() -> Vec<Arc<GuardShared>> {
    let Ok(mut list) = active_guards_slot().lock() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(list.len());
    list.retain(|w| {
        if let Some(arc) = w.upgrade() {
            out.push(arc);
            true
        } else {
            false
        }
    });
    out
}

#[inline]
fn to_hwnd(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

fn is_shell_or_desktop_class(hwnd: HWND) -> bool {
    let mut buf = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    if len <= 0 {
        return false;
    }
    let name = String::from_utf16_lossy(&buf[..len as usize]);
    matches!(
        name.as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
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

fn is_above_any_normal_window(hwnd: HWND, managed: &HashMap<isize, GuardRule>) -> bool {
    unsafe {
        let mut next = GetWindow(hwnd, GW_HWNDNEXT).unwrap_or_default();
        let mut hops = 0usize;
        while !next.is_invalid() && hops < 512 {
            hops += 1;
            let raw = next.0 as isize;
            if !managed.contains_key(&raw)
                && IsWindowVisible(next).as_bool()
                && !IsIconic(next).as_bool()
                && !is_dwm_cloaked(next)
                && !is_shell_or_desktop_class(next)
            {
                let owner = GetWindow(next, GW_OWNER).unwrap_or_default();
                let ex = GetWindowLongPtrW(next, GWL_EXSTYLE) as u32;
                let is_helper = (ex & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0)) != 0
                    && (ex & WS_EX_APPWINDOW.0) == 0;
                if owner != hwnd && !is_helper {
                    let mut r = RECT::default();
                    if GetWindowRect(next, &mut r).is_ok()
                        && (r.right - r.left) >= 32
                        && (r.bottom - r.top) >= 24
                    {
                        return true;
                    }
                }
            }
            next = GetWindow(next, GW_HWNDNEXT).unwrap_or_default();
        }
    }
    false
}

/// Un-maximize a zoomed window without activating it so off-corner / monitor placement sticks.
pub(crate) fn unmaximize_no_activate(hwnd: HWND) {
    unsafe {
        if !IsZoomed(hwnd).as_bool() {
            return;
        }
        let mut wp = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        if GetWindowPlacement(hwnd, &mut wp).is_ok() {
            wp.showCmd = SW_SHOWNOACTIVATE.0 as u32;
            let _ = SetWindowPlacement(hwnd, &wp);
        }
    }
}

/// Compute desired `GWL_EXSTYLE` bits cleanly without losing unrelated flags.
pub(crate) fn compute_hidden_ex_style(
    orig_ex_style: isize,
    current_ex: isize,
    strategy: HideStrategy,
) -> isize {
    let tool_bit = WS_EX_TOOLWINDOW.0 as isize;
    let app_bit = WS_EX_APPWINDOW.0 as isize;
    let noact_bit = WS_EX_NOACTIVATE.0 as isize;
    let layered_bit = WS_EX_LAYERED.0 as isize;
    let transp_bit = WS_EX_TRANSPARENT.0 as isize;

    let mut ex = (current_ex | noact_bit) & !app_bit;
    if strategy.hides_from_alt_tab() {
        ex |= tool_bit;
    } else if (orig_ex_style & tool_bit) == 0 {
        ex &= !tool_bit;
    }

    if strategy == HideStrategy::LayeredGhost {
        ex |= layered_bit | transp_bit;
    } else {
        if (orig_ex_style & layered_bit) == 0 {
            ex &= !layered_bit;
        }
        if strategy == HideStrategy::CornerPeek {
            // Keep 1px edge cursor/click-transparent even if Taskbar auto-hides.
            ex |= transp_bit;
        } else if (orig_ex_style & transp_bit) == 0 {
            ex &= !transp_bit;
        }
    }
    ex
}

fn bounce_foreground_safely(hidden_hwnd: HWND, fallback_raw: isize) {
    unsafe {
        let fallback = to_hwnd(fallback_raw);
        let shell = GetShellWindow();
        let target = if fallback_raw != 0
            && fallback != hidden_hwnd
            && IsWindow(Some(fallback)).as_bool()
            && IsWindowVisible(fallback).as_bool()
            && !IsIconic(fallback).as_bool()
            && !is_dwm_cloaked(fallback)
        {
            fallback
        } else if !shell.is_invalid() {
            shell
        } else {
            return;
        };

        if SetForegroundWindow(target).as_bool() && GetForegroundWindow() != hidden_hwnd {
            return;
        }

        // Bypass background foreground-lock safely via brief thread-input attachment.
        let cur_tid = GetCurrentThreadId();
        let fg_tid = GetWindowThreadProcessId(hidden_hwnd, None);
        let tgt_tid = GetWindowThreadProcessId(target, None);
        if fg_tid != 0 && fg_tid != cur_tid {
            let _ = AttachThreadInput(cur_tid, fg_tid, true);
        }
        if tgt_tid != 0 && tgt_tid != cur_tid && tgt_tid != fg_tid {
            let _ = AttachThreadInput(cur_tid, tgt_tid, true);
        }
        let _ = SetForegroundWindow(target);
        let _ = BringWindowToTop(target);
        if tgt_tid != 0 && tgt_tid != cur_tid && tgt_tid != fg_tid {
            let _ = AttachThreadInput(cur_tid, tgt_tid, false);
        }
        if fg_tid != 0 && fg_tid != cur_tid {
            let _ = AttachThreadInput(cur_tid, fg_tid, false);
        }
    }
}

/// Enforce all invariants (non-minimized, styles, alpha, position, Z-order, Taskbar, focus bounce) for one HWND.
pub(crate) fn enforce_hwnd_state(
    hwnd_raw: isize,
    rule: GuardRule,
    snapshot_rules: &HashMap<isize, GuardRule>,
    include_taskbar_tab: bool,
    last_unmanaged_fg: isize,
) -> bool {
    let hwnd = to_hwnd(hwnd_raw);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }

        let current_ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let desired_ex = compute_hidden_ex_style(rule.orig_ex_style, current_ex, rule.strategy);
        if desired_ex != current_ex {
            let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired_ex);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE
                    | SWP_NOSIZE
                    | SWP_NOZORDER
                    | SWP_NOACTIVATE
                    | SWP_FRAMECHANGED
                    | SWP_ASYNCWINDOWPOS,
            );
        }

        if rule.strategy == HideStrategy::LayeredGhost {
            let mut alpha: u8 = 255;
            let mut lwa_flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
            let ok = GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), Some(&mut lwa_flags))
                .is_ok();
            if !ok || (lwa_flags.0 & LWA_ALPHA.0) == 0 || alpha != 0 {
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
            }
        }

        // Defend against `Win+D` / Show Desktop minimizing hidden windows and freezing DWM capture.
        // Skip rect checks while still iconic (`(-32000, -32000)`); burst timer + LOCATIONCHANGE fix once awake.
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE);
            if include_taskbar_tab {
                let _ = try_taskbar_tab(hwnd, false);
            }
            if GetForegroundWindow() == hwnd {
                bounce_foreground_safely(hwnd, last_unmanaged_fg);
            }
            return true;
        }

        if rule.strategy.is_relocated() && IsZoomed(hwnd).as_bool() {
            unmaximize_no_activate(hwnd);
        }

        if let Some((tx, ty)) = rule.expected_pos {
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r).is_ok()
                && ((r.left - tx).abs() > 2 || (r.top - ty).abs() > 2)
            {
                let insert_after = if rule.strategy != HideStrategy::SecondaryMonitor {
                    Some(HWND_BOTTOM)
                } else {
                    None
                };
                let z_flag = if insert_after.is_some() {
                    Default::default()
                } else {
                    SWP_NOZORDER
                };
                let _ = SetWindowPos(
                    hwnd,
                    insert_after,
                    tx,
                    ty,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOSENDCHANGING | SWP_ASYNCWINDOWPOS | z_flag,
                );
            }
        }

        if rule.strategy != HideStrategy::SecondaryMonitor
            && is_above_any_normal_window(hwnd, snapshot_rules)
        {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_BOTTOM),
                0,
                0,
                0,
                0,
                SWP_NOMOVE
                    | SWP_NOSIZE
                    | SWP_NOACTIVATE
                    | SWP_NOSENDCHANGING
                    | SWP_ASYNCWINDOWPOS,
            );
        }

        if include_taskbar_tab {
            let _ = try_taskbar_tab(hwnd, false);
        }

        if GetForegroundWindow() == hwnd {
            bounce_foreground_safely(hwnd, last_unmanaged_fg);
        }
    }
    true
}

// Per-thread hook & shell broadcast state live on the dedicated WinEventGuard OS thread only.
thread_local! {
    static HOOK_BURST_TICKS: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    static HOOK_TIMER_ID: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LAST_ENFORCED: std::cell::RefCell<HashMap<isize, Instant>> =
        std::cell::RefCell::new(HashMap::new());
    static TASKBAR_CREATED_MSG: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

const BURST_TIMER_ELAPSE_MS: u32 = 75;
const BURST_TIMER_SLOT: usize = 0x4744;

fn arm_burst_sweep() {
    HOOK_BURST_TICKS.with(|t| t.set(6));
    HOOK_TIMER_ID.with(|id| {
        if id.get() == 0 {
            let created = unsafe { SetTimer(None, BURST_TIMER_SLOT, BURST_TIMER_ELAPSE_MS, None) };
            id.set(created);
        }
    });
}

fn recompute_relocated_positions(shared: &GuardShared) {
    let mut guard = match shared.rules.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_empty() {
        return;
    }
    let corner = corner_peek_position();
    let mon2 = find_secondary_monitor();
    for rule in guard.values_mut() {
        match rule.strategy {
            HideStrategy::CornerPeek => {
                rule.expected_pos = Some(corner);
            }
            HideStrategy::SecondaryMonitor => {
                if let Some(m) = mon2 {
                    let wa = m.work_area;
                    if let Some((cur_x, cur_y)) = rule.expected_pos {
                        let cx = cur_x.clamp(wa.left, (wa.right - 320).max(wa.left));
                        let cy = cur_y.clamp(wa.top, (wa.bottom - 240).max(wa.top));
                        rule.expected_pos = Some((cx, cy));
                    }
                } else {
                    rule.strategy = HideStrategy::CornerPeek;
                    rule.expected_pos = Some(corner);
                }
            }
            _ => {}
        }
    }
}

fn sweep_snapshot(shared: &GuardShared, include_taskbar_tab: bool) {
    let snapshot: HashMap<isize, GuardRule> = {
        let Ok(guard) = shared.rules.lock() else {
            return;
        };
        if guard.is_empty() {
            return;
        }
        guard.clone()
    };
    let last_fg = shared.last_unmanaged_fg.load(Ordering::Relaxed);
    let mut dead = Vec::new();
    for (&hwnd_raw, &rule) in &snapshot {
        if !enforce_hwnd_state(hwnd_raw, rule, &snapshot, include_taskbar_tab, last_fg) {
            dead.push(hwnd_raw);
        }
    }
    if !dead.is_empty()
        && let Ok(mut guard) = shared.rules.lock()
    {
        for h in dead {
            guard.remove(&h);
        }
    }
}

unsafe extern "system" fn helper_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let taskbar_msg = TASKBAR_CREATED_MSG.with(|m| m.get());
    if msg == WM_DISPLAYCHANGE
        || msg == WM_SETTINGCHANGE
        || (taskbar_msg != 0 && msg == taskbar_msg)
    {
        for shared in live_guards() {
            if msg == WM_DISPLAYCHANGE || msg == WM_SETTINGCHANGE {
                recompute_relocated_positions(&shared);
            }
            sweep_snapshot(&shared, true);
        }
        arm_burst_sweep();
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    if id_object != OBJID_WINDOW.0 || id_child != (CHILDID_SELF as i32) || hwnd.is_invalid() {
        return;
    }

    for shared in live_guards() {
        let hwnd_raw = hwnd.0 as isize;
        if event == EVENT_SYSTEM_FOREGROUND {
            let is_managed = shared
                .rules
                .lock()
                .is_ok_and(|g| g.contains_key(&hwnd_raw));
            if !is_managed && !is_shell_or_desktop_class(hwnd) {
                shared.last_unmanaged_fg.store(hwnd_raw, Ordering::Relaxed);
            }
            sweep_snapshot(&shared, true);
            arm_burst_sweep();
            continue;
        }

        if matches!(event, EVENT_SYSTEM_MINIMIZESTART | EVENT_SYSTEM_MINIMIZEEND) {
            sweep_snapshot(&shared, true);
            arm_burst_sweep();
            continue;
        }

        // Resolve direct rule OR owned-popup coupling (`GW_OWNER` belongs to a managed window).
        let owner_raw = unsafe { GetWindow(hwnd, GW_OWNER).unwrap_or_default().0 as isize };
        let snapshot_opt: Option<(isize, GuardRule, HashMap<isize, GuardRule>)> = {
            let Ok(guard) = shared.rules.lock() else {
                continue;
            };
            if let Some(&r) = guard.get(&hwnd_raw) {
                Some((hwnd_raw, r, guard.clone()))
            } else if owner_raw != 0 {
                guard
                    .get(&owner_raw)
                    .copied()
                    .map(|r| (owner_raw, r, guard.clone()))
            } else {
                None
            }
        };
        let Some((target_raw, rule, snapshot)) = snapshot_opt else {
            continue;
        };

        let allow = LAST_ENFORCED.with(|map| {
            let mut map = map.borrow_mut();
            let now = Instant::now();
            if let Some(prev) = map.get(&target_raw)
                && event == EVENT_OBJECT_LOCATIONCHANGE
                && now.duration_since(*prev) < Duration::from_millis(8)
            {
                return false;
            }
            map.insert(target_raw, now);
            true
        });
        if !allow {
            arm_burst_sweep();
            continue;
        }

        let include_tab = matches!(
            event,
            EVENT_OBJECT_SHOW | EVENT_OBJECT_STATECHANGE | EVENT_OBJECT_HIDE
        );
        let last_fg = shared.last_unmanaged_fg.load(Ordering::Relaxed);
        let alive = enforce_hwnd_state(target_raw, rule, &snapshot, include_tab, last_fg);
        if !alive
            && let Ok(mut guard) = shared.rules.lock()
        {
            guard.remove(&target_raw);
        }
        if matches!(
            event,
            EVENT_OBJECT_SHOW | EVENT_OBJECT_LOCATIONCHANGE | EVENT_OBJECT_REORDER
        ) {
            arm_burst_sweep();
        }
    }
}

pub struct WinEventGuard {
    shared: Arc<GuardShared>,
    handle: Option<JoinHandle<()>>,
}

impl Default for WinEventGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl WinEventGuard {
    pub fn new() -> Self {
        let shared = Arc::new(GuardShared {
            rules: Mutex::new(HashMap::new()),
            last_unmanaged_fg: AtomicIsize::new(0),
            thread_id: AtomicU32::new(0),
        });
        if let Ok(mut list) = active_guards_slot().lock() {
            list.push(Arc::downgrade(&shared));
        }

        let shared_bg = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name("ghostdock-winevent".to_owned())
            .spawn(move || unsafe {
                let tid = GetCurrentThreadId();
                shared_bg.thread_id.store(tid, Ordering::Release);
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

                let tb_msg = RegisterWindowMessageW(w!("TaskbarCreated"));
                TASKBAR_CREATED_MSG.with(|m| m.set(tb_msg));

                let cls_name = w!("GhostDockGuardHelper");
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(helper_wnd_proc),
                    lpszClassName: cls_name,
                    ..Default::default()
                };
                let _ = RegisterClassW(&wc);
                let helper_hwnd = CreateWindowExW(
                    WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0),
                    cls_name,
                    w!(""),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    None,
                    None,
                );

                let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
                let hooks = [
                    SetWinEventHook(
                        EVENT_SYSTEM_FOREGROUND,
                        EVENT_SYSTEM_FOREGROUND,
                        None,
                        Some(win_event_proc),
                        0,
                        0,
                        flags,
                    ),
                    SetWinEventHook(
                        EVENT_SYSTEM_MINIMIZESTART,
                        EVENT_SYSTEM_MINIMIZEEND,
                        None,
                        Some(win_event_proc),
                        0,
                        0,
                        flags,
                    ),
                    SetWinEventHook(
                        EVENT_OBJECT_SHOW,
                        EVENT_OBJECT_REORDER,
                        None,
                        Some(win_event_proc),
                        0,
                        0,
                        flags,
                    ),
                    SetWinEventHook(
                        EVENT_OBJECT_STATECHANGE,
                        EVENT_OBJECT_LOCATIONCHANGE,
                        None,
                        Some(win_event_proc),
                        0,
                        0,
                        flags,
                    ),
                ];

                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    if msg.message == WM_TIMER {
                        let rem = HOOK_BURST_TICKS.with(|t| {
                            let cur = t.get();
                            let next = cur.saturating_sub(1);
                            t.set(next);
                            next
                        });
                        sweep_snapshot(&shared_bg, true);
                        if rem == 0 {
                            HOOK_TIMER_ID.with(|id| {
                                let tid_timer = id.get();
                                if tid_timer != 0 {
                                    let _ = KillTimer(None, tid_timer);
                                    id.set(0);
                                }
                            });
                        }
                    } else {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }

                for h in hooks {
                    if !h.is_invalid() {
                        let _ = UnhookWinEvent(h);
                    }
                }
                if let Ok(hw) = helper_hwnd {
                    let _ = DestroyWindow(hw);
                }
                CoUninitialize();
            })
            .ok();

        Self { shared, handle }
    }

    pub fn upsert(&self, hwnd_raw: isize, rule: GuardRule) {
        if let Ok(mut guard) = self.shared.rules.lock() {
            guard.insert(hwnd_raw, rule);
        }
    }

    pub fn remove(&self, hwnd_raw: isize) {
        if let Ok(mut guard) = self.shared.rules.lock() {
            guard.remove(&hwnd_raw);
        }
    }

    pub fn sweep_now(&self, include_taskbar_tab: bool) {
        sweep_snapshot(&self.shared, include_taskbar_tab);
    }

    pub fn last_unmanaged_fg(&self) -> isize {
        self.shared.last_unmanaged_fg.load(Ordering::Relaxed)
    }
}

impl Drop for WinEventGuard {
    fn drop(&mut self) {
        let tid = self.shared.thread_id.load(Ordering::Acquire);
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
