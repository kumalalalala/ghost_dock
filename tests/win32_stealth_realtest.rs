//! Real Win32 cross-process integration tests for GhostDock enumerator & stealth controller.

use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use ghost_dock::win32::controller::WindowController;
use ghost_dock::win32::enumerator::{
    ProcessMetaCache, enumerate_window_groups, enumerate_window_groups_cached,
};
use ghost_dock::win32::{HideStrategy, WindowState};
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GW_HWNDNEXT, GWL_EXSTYLE, GetLayeredWindowAttributes, GetWindow, GetWindowLongPtrW,
    GetWindowRect, IsIconic, IsWindowVisible, IsZoomed, LAYERED_WINDOW_ATTRIBUTES_FLAGS,
    LWA_ALPHA, SW_MINIMIZE, SW_SHOWMAXIMIZED, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT,
};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_real_test_window(title: &str) -> (ChildGuard, isize) {
    let py_code = format!(
        "import tkinter as tk; r=tk.Tk(); r.title('{title}'); r.geometry('320x120+140+140'); r.mainloop()"
    );
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let child = Command::new("python")
        .args(["-c", &py_code])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("python must be available to spawn real test window");
    let target_pid = child.id();
    let guard = ChildGuard(child);

    let deadline = Instant::now() + Duration::from_secs(6);
    let empty_map = std::collections::HashMap::new();
    while Instant::now() < deadline {
        let groups = enumerate_window_groups(&empty_map);
        for g in groups {
            if g.pid == target_pid
                && let Some(w) = g.windows.iter().find(|w| w.title == title)
            {
                return (guard, w.hwnd_raw);
            }
        }
        thread::sleep(Duration::from_millis(60));
    }
    panic!("Timed out waiting for real test window '{title}' (pid={target_pid})");
}

#[test]
fn real_window_enumeration_stealth_guard_corner_minimize_and_restore() {
    let (_child, hwnd_raw) = spawn_real_test_window("GhostDockRealTestTarget");
    let hwnd = HWND(hwnd_raw as *mut c_void);

    let mut orig_rect = RECT::default();
    unsafe {
        GetWindowRect(hwnd, &mut orig_rect).expect("GetWindowRect");
    }

    let mut ctrl = WindowController::default();

    // 1. Hide with StealthBackground -> WS_EX_TOOLWINDOW set, WS_EX_APPWINDOW cleared, still visible & not iconic
    let actual = ctrl
        .hide_window(hwnd_raw, HideStrategy::StealthBackground)
        .expect("hide_window StealthBackground");
    assert_eq!(actual, HideStrategy::StealthBackground);

    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        assert_ne!(ex & WS_EX_TOOLWINDOW.0, 0, "must have WS_EX_TOOLWINDOW");
        assert_eq!(ex & WS_EX_APPWINDOW.0, 0, "must clear WS_EX_APPWINDOW");
        assert!(IsWindowVisible(hwnd).as_bool(), "must remain WS_VISIBLE");
        assert!(!IsIconic(hwnd).as_bool(), "must NOT become WS_ICONIC");
    }

    // 2. Enumerator still tracks managed stealth window in its process group
    let groups = enumerate_window_groups(&ctrl.managed_map());
    let tracked = groups
        .iter()
        .flat_map(|g| g.windows.iter())
        .find(|w| w.hwnd_raw == hwnd_raw)
        .expect("stealth-hidden window must remain tracked in Dashboard");
    assert_eq!(
        tracked.state,
        WindowState::StealthHidden(HideStrategy::StealthBackground)
    );

    // 3. Simulate an Electron/Qt app resetting its own GWL_EXSTYLE -> enforce_stealth_guards fixes it
    unsafe {
        let corrupted = WS_EX_APPWINDOW.0 as isize;
        let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, corrupted);
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
    ctrl.enforce_stealth_guards();
    unsafe {
        let repaired = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        assert_ne!(repaired & WS_EX_TOOLWINDOW.0, 0, "guard must restore WS_EX_TOOLWINDOW");
        assert_eq!(repaired & WS_EX_APPWINDOW.0, 0, "guard must strip WS_EX_APPWINDOW");
    }

    // 4. Switch to CornerPeek -> relocates while remaining non-iconic
    ctrl.hide_window(hwnd_raw, HideStrategy::CornerPeek)
        .expect("hide_window CornerPeek");
    unsafe {
        assert!(!IsIconic(hwnd).as_bool());
        assert!(IsWindowVisible(hwnd).as_bool());
    }

    // 5. Show -> restores original style & position
    ctrl.show_window(hwnd_raw, false).expect("show_window");
    let mut restored_rect = RECT::default();
    unsafe {
        let ex_restored = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        assert_eq!(ex_restored & WS_EX_TOOLWINDOW.0, 0);
        GetWindowRect(hwnd, &mut restored_rect).expect("GetWindowRect after restore");
    }
    assert_eq!(restored_rect.left, orig_rect.left);
    assert_eq!(restored_rect.top, orig_rect.top);

    // 6. Standard OS Minimize -> sets WS_ICONIC
    ctrl.minimize_window(hwnd_raw).expect("minimize_window");
    unsafe {
        assert!(IsIconic(hwnd).as_bool(), "minimize_window must set WS_ICONIC");
    }

    // 7. Re-hide from minimized -> wakes DWM surface out of WS_ICONIC and stealth-hides, then restore_all restores
    ctrl.hide_window(hwnd_raw, HideStrategy::StealthBackground)
        .expect("hide from minimized");
    unsafe {
        assert!(!IsIconic(hwnd).as_bool());
        assert!(IsWindowVisible(hwnd).as_bool());
    }
    ctrl.restore_all();
    assert_eq!(ctrl.hidden_count(), 0);
}

#[test]
fn cached_enumeration_tracks_and_prunes_exited_process() {
    let mut cache = ProcessMetaCache::default();
    let empty = std::collections::HashMap::new();
    let (guard, hwnd_raw) = spawn_real_test_window("GhostDockCacheTarget");

    let g1 = enumerate_window_groups_cached(&empty, &mut cache);
    assert!(
        g1.iter()
            .flat_map(|g| g.windows.iter())
            .any(|w| w.hwnd_raw == hwnd_raw)
    );

    // Second sweep hits cache and finds identical window
    let g2 = enumerate_window_groups_cached(&empty, &mut cache);
    assert!(
        g2.iter()
            .flat_map(|g| g.windows.iter())
            .any(|w| w.hwnd_raw == hwnd_raw)
    );

    // Drop child process and verify dead window is pruned cleanly
    drop(guard);
    thread::sleep(Duration::from_millis(120));
    let g3 = enumerate_window_groups_cached(&empty, &mut cache);
    assert!(
        !g3.iter()
            .flat_map(|g| g.windows.iter())
            .any(|w| w.hwnd_raw == hwnd_raw)
    );
}

#[test]
fn single_instance_secondary_launch_wakes_primary_instead_of_spawning() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use ghost_dock::single_instance::{InstanceAcquisition, acquire_or_wake_named};

    let pid = std::process::id();
    let mutex_name = format!("Local\\GhostDock_TestMutex_{pid}");
    let event_name = format!("Local\\GhostDock_TestEvent_{pid}");

    let mut primary = match acquire_or_wake_named(&mutex_name, &event_name) {
        InstanceAcquisition::Primary(g) => g,
        InstanceAcquisition::SecondarySignaled => panic!("First acquisition must be Primary"),
    };

    let woken = Arc::new(AtomicBool::new(false));
    let woken_clone = Arc::clone(&woken);
    primary.start_listener(move || {
        woken_clone.store(true, Ordering::SeqCst);
    });

    // Second launch must signal Primary and NOT acquire Primary
    match acquire_or_wake_named(&mutex_name, &event_name) {
        InstanceAcquisition::SecondarySignaled => {}
        InstanceAcquisition::Primary(_) => panic!("Second acquisition must signal Primary"),
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    while !woken.load(Ordering::SeqCst) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(woken.load(Ordering::SeqCst), "Primary listener was not woken");

    // Dropping primary releases mutex so a subsequent launch acquires Primary cleanly
    drop(primary);
    match acquire_or_wake_named(&mutex_name, &event_name) {
        InstanceAcquisition::Primary(_) => {}
        InstanceAcquisition::SecondarySignaled => panic!("Lock should be released after drop"),
    }
}

#[test]
fn taskbar_capturable_strategy_preserves_window_capture_flags_and_toggles_cleanly() {
    let (_child, hwnd_raw) = spawn_real_test_window("GhostDockCapturableTarget");
    let hwnd = HWND(hwnd_raw as *mut c_void);
    let mut ctrl = WindowController::default();

    let actual = ctrl
        .hide_window(hwnd_raw, HideStrategy::TaskbarCapturable)
        .expect("hide_window TaskbarCapturable");
    assert_eq!(actual, HideStrategy::TaskbarCapturable);

    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        assert_eq!(
            ex & WS_EX_TOOLWINDOW.0,
            0,
            "TaskbarCapturable must NOT set WS_EX_TOOLWINDOW so OBS/TikTok pickers see it"
        );
        assert!(IsWindowVisible(hwnd).as_bool(), "must stay WS_VISIBLE");
        assert!(!IsIconic(hwnd).as_bool(), "must NOT become WS_ICONIC");
    }

    // Periodic guard must not inject WS_EX_TOOLWINDOW under TaskbarCapturable
    ctrl.enforce_stealth_guards();
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        assert_eq!(ex & WS_EX_TOOLWINDOW.0, 0);
    }

    // Switching to StealthBackground enables WS_EX_TOOLWINDOW, and switching back clears it
    ctrl.hide_window(hwnd_raw, HideStrategy::StealthBackground)
        .expect("switch to StealthBackground");
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        assert_ne!(ex & WS_EX_TOOLWINDOW.0, 0);
    }

    ctrl.hide_window(hwnd_raw, HideStrategy::TaskbarCapturable)
        .expect("switch back to TaskbarCapturable");
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        assert_eq!(ex & WS_EX_TOOLWINDOW.0, 0);
    }

    ctrl.restore_all();
    assert_eq!(ctrl.hidden_count(), 0);
}

fn is_hwnd_above(upper: HWND, lower: HWND) -> bool {
    unsafe {
        let mut cur = GetWindow(upper, GW_HWNDNEXT).unwrap_or_default();
        let mut hops = 0;
        while !cur.is_invalid() && hops < 512 {
            hops += 1;
            if cur == lower {
                return true;
            }
            cur = GetWindow(cur, GW_HWNDNEXT).unwrap_or_default();
        }
    }
    false
}

#[test]
fn desktop_win_d_restore_zorder_and_layered_ghost_containment() {
    use ghost_dock::win32::monitors::corner_peek_position;
    use windows::Win32::UI::WindowsAndMessaging::HWND_TOP;

    let (_c1, hidden_raw) = spawn_real_test_window("GhostDockHiddenDesktopTarget");
    let (_c2, front_raw) = spawn_real_test_window("GhostDockFrontNormalApp");
    let hidden = HWND(hidden_raw as *mut c_void);
    let front = HWND(front_raw as *mut c_void);

    let mut ctrl = WindowController::default();

    // 1. Default CornerPeek hides off Desktop (corner_peek_position), sets WS_EX_NOACTIVATE, keeps !WS_EX_TOOLWINDOW
    let actual = ctrl
        .hide_window(hidden_raw, HideStrategy::CornerPeek)
        .expect("hide_window CornerPeek");
    assert_eq!(actual, HideStrategy::CornerPeek);
    let (cx, cy) = corner_peek_position();

    unsafe {
        let ex = GetWindowLongPtrW(hidden, GWL_EXSTYLE) as u32;
        assert_ne!(ex & WS_EX_NOACTIVATE.0, 0, "must set WS_EX_NOACTIVATE");
        assert_eq!(ex & WS_EX_TOOLWINDOW.0, 0, "CornerPeek must stay OBS/TikTok pickable");
        let mut r = RECT::default();
        GetWindowRect(hidden, &mut r).unwrap();
        assert!((r.left - cx).abs() <= 2 && (r.top - cy).abs() <= 2);
        assert!(
            !is_hwnd_above(hidden, front),
            "hidden window must sit below normal front window"
        );
    }

    // 2. Simulate Explorer Win+D minimize + Desktop restore trying to yank window back on-screen and HWND_TOP
    unsafe {
        let _ = ShowWindow(hidden, SW_MINIMIZE);
    }
    thread::sleep(Duration::from_millis(250));
    unsafe {
        let _ = SetWindowPos(hidden, Some(HWND_TOP), 160, 140, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        let _ = SetWindowLongPtrW(hidden, GWL_EXSTYLE, WS_EX_APPWINDOW.0 as isize);
    }

    // Give WinEventGuard background thread a brief moment + fallback sweep to enforce containment
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut contained = false;
    while Instant::now() < deadline {
        ctrl.enforce_stealth_guards();
        unsafe {
            let ex = GetWindowLongPtrW(hidden, GWL_EXSTYLE) as u32;
            let mut r = RECT::default();
            GetWindowRect(hidden, &mut r).unwrap();
            if !IsIconic(hidden).as_bool()
                && (ex & WS_EX_NOACTIVATE.0) != 0
                && (ex & WS_EX_APPWINDOW.0) == 0
                && (r.left - cx).abs() <= 2
                && (r.top - cy).abs() <= 2
                && !is_hwnd_above(hidden, front)
            {
                contained = true;
                break;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        contained,
        "WinEventGuard + controller must keep hidden window off Desktop, un-minimized, non-activating, and below normal apps"
    );

    // 3. Test LayeredGhost (Alpha 0 + click-through WS_EX_TRANSPARENT) + alpha tamper guard & clean restore
    let ghost_actual = ctrl
        .hide_window(hidden_raw, HideStrategy::LayeredGhost)
        .expect("hide_window LayeredGhost");
    assert_eq!(ghost_actual, HideStrategy::LayeredGhost);
    unsafe {
        let ex = GetWindowLongPtrW(hidden, GWL_EXSTYLE) as u32;
        assert_ne!(ex & WS_EX_LAYERED.0, 0);
        assert_ne!(ex & WS_EX_TRANSPARENT.0, 0);
        assert_ne!(ex & WS_EX_NOACTIVATE.0, 0);
        assert_eq!(ex & WS_EX_TOOLWINDOW.0, 0);

        // Simulate target app resetting its own alpha to 255 without changing GWL_EXSTYLE
        let _ = SetLayeredWindowAttributes(hidden, COLORREF(0), 255, LWA_ALPHA);
    }
    ctrl.enforce_stealth_guards();
    unsafe {
        let mut alpha: u8 = 255;
        let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
        GetLayeredWindowAttributes(hidden, None, Some(&mut alpha), Some(&mut flags))
            .expect("GetLayeredWindowAttributes");
        assert_ne!(flags.0 & LWA_ALPHA.0, 0);
        assert_eq!(alpha, 0, "guard must reset alpha back to 0 even when GWL_EXSTYLE was unchanged");
    }

    ctrl.restore_all();
    assert_eq!(ctrl.hidden_count(), 0);
    unsafe {
        let ex = GetWindowLongPtrW(hidden, GWL_EXSTYLE) as u32;
        assert_eq!(ex & WS_EX_LAYERED.0, 0, "restore_all must clear added WS_EX_LAYERED");
        assert_eq!(ex & WS_EX_TRANSPARENT.0, 0);
        assert_eq!(ex & WS_EX_NOACTIVATE.0, 0);
    }

    // 4. Maximized window hidden with CornerPeek un-maximizes silently and restores Maximized even with focus=false (Show All)
    unsafe {
        let _ = ShowWindow(hidden, SW_SHOWMAXIMIZED);
    }
    thread::sleep(Duration::from_millis(80));
    ctrl.hide_window(hidden_raw, HideStrategy::CornerPeek)
        .expect("hide zoomed window");
    unsafe {
        assert!(!IsZoomed(hidden).as_bool(), "CornerPeek must un-maximize before tucking");
        let ex = GetWindowLongPtrW(hidden, GWL_EXSTYLE) as u32;
        assert_ne!(ex & WS_EX_TRANSPARENT.0, 0, "CornerPeek 1px edge must be click-transparent");
    }
    ctrl.show_window(hidden_raw, false)
        .expect("show_window non-focused");
    thread::sleep(Duration::from_millis(80));
    unsafe {
        assert!(
            IsZoomed(hidden).as_bool(),
            "show_window(focus=false) must preserve originally maximized state"
        );
        let ex = GetWindowLongPtrW(hidden, GWL_EXSTYLE) as u32;
        assert_eq!(ex & WS_EX_TRANSPARENT.0, 0);
    }
}
