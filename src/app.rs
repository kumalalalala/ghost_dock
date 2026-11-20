//! GhostDock minimalist dashboard UI with Big-Tech hover polish and cached Win32 sync.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, CornerRadius, CursorIcon, Key, Layout, Margin, RichText, Sense, Stroke,
    Vec2,
};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
};
use windows::core::BOOL;

use crate::single_instance::{InstanceAcquisition, SingleInstanceGuard, acquire_or_wake_existing};
use crate::tray::{TrayCommand, TrayController};
use crate::ui::theme::{
    self, BADGE_WARN, BG_CANVAS, BG_SURFACE, BG_SURFACE_HOVER, BG_TRACK, BORDER_HAIRLINE,
    BORDER_HOVER, DOT_CAPTURABLE, DOT_GHOST, DOT_LIVE, DOT_MIN, PillTone, TEXT_MUTED,
    TEXT_PRIMARY, TEXT_SECONDARY, animated_pill, draw_status_dot, lerp_color, segmented_strategy,
};
use crate::win32::controller::{
    WindowController, launch_without_occlusion, relaunch_elevated, style_own_window_dark,
};
use crate::win32::enumerator::{
    ProcessMetaCache, enumerate_window_groups_cached, is_current_process_elevated,
};
use crate::win32::monitors::list_monitors;
use crate::win32::{HideStrategy, ProcessGroup, WindowEntry, WindowState};

pub struct GhostDockApp {
    controller: WindowController,
    proc_cache: ProcessMetaCache,
    tray: Option<TrayController>,
    single_instance: Option<SingleInstanceGuard>,
    groups: Vec<ProcessGroup>,
    strategy: HideStrategy,
    monitor_count: usize,
    self_elevated: bool,
    search: String,
    dashboard_visible: bool,
    quitting: bool,
    own_hwnd: isize,
    status_msg: Option<(String, bool, Instant)>,
    last_refresh: Instant,
}

enum RowAction {
    Show(isize),
    Hide(isize, HideStrategy),
    Minimize(isize),
    HideAll(Vec<isize>, HideStrategy),
    ShowAll(Vec<isize>),
    RelaunchNoOcclusion(String),
}

fn find_own_visible_hwnd() -> isize {
    struct Ctx {
        pid: u32,
        found: isize,
    }
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = unsafe { &mut *(lparam.0 as *mut Ctx) };
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == ctx.pid && IsWindowVisible(hwnd).as_bool() {
                ctx.found = hwnd.0 as isize;
                return BOOL(0);
            }
        }
        BOOL(1)
    }
    let mut ctx = Ctx {
        pid: std::process::id(),
        found: 0,
    };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut Ctx as isize));
    }
    ctx.found
}

impl GhostDockApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        mut single_instance: Option<SingleInstanceGuard>,
    ) -> Self {
        theme::apply_minimal_theme(&cc.egui_ctx);
        let controller = WindowController::default();
        let mut proc_cache = ProcessMetaCache::default();
        let tray = TrayController::new(cc.egui_ctx.clone());

        if let Some(guard) = single_instance.as_mut() {
            let tx_wake = tray.tx.clone();
            let ctx_wake = cc.egui_ctx.clone();
            guard.start_listener(move || {
                let _ = tx_wake.send(TrayCommand::ShowDashboard);
                ctx_wake.request_repaint();
            });
        }

        let groups = enumerate_window_groups_cached(&controller.managed_map(), &mut proc_cache);
        let monitor_count = list_monitors().len();
        let self_elevated = is_current_process_elevated();

        Self {
            controller,
            proc_cache,
            tray: Some(tray),
            single_instance,
            groups,
            strategy: HideStrategy::CornerPeek,
            monitor_count,
            self_elevated,
            search: String::new(),
            dashboard_visible: true,
            quitting: false,
            own_hwnd: 0,
            status_msg: None,
            last_refresh: Instant::now(),
        }
    }

    fn show_and_focus_dashboard(&mut self, ctx: &egui::Context) {
        self.dashboard_visible = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        if self.own_hwnd != 0 {
            let hwnd = HWND(self.own_hwnd as *mut std::ffi::c_void);
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                let _ = BringWindowToTop(hwnd);
                let _ = SetForegroundWindow(hwnd);
            }
        }
        self.refresh_windows();
    }

    fn set_status(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status_msg = Some((msg.into(), is_error, Instant::now()));
    }

    fn refresh_windows(&mut self) {
        self.controller.enforce_stealth_guards();
        self.groups =
            enumerate_window_groups_cached(&self.controller.managed_map(), &mut self.proc_cache);
        self.monitor_count = list_monitors().len();
        self.last_refresh = Instant::now();
    }

    fn handle_tray_events(&mut self, ctx: &egui::Context) {
        let mut drained = Vec::new();
        if let Some(tray) = &self.tray {
            while let Ok(cmd) = tray.rx.try_recv() {
                drained.push(cmd);
            }
        }
        for cmd in drained {
            match cmd {
                TrayCommand::ToggleDashboard => {
                    if self.dashboard_visible {
                        self.dashboard_visible = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    } else {
                        self.show_and_focus_dashboard(ctx);
                    }
                }
                TrayCommand::ShowDashboard => {
                    self.show_and_focus_dashboard(ctx);
                }
                TrayCommand::RestoreAllWindows => {
                    self.controller.restore_all();
                    self.refresh_windows();
                    self.set_status("Restored all hidden windows", false);
                }
                TrayCommand::Quit => {
                    self.controller.restore_all();
                    self.single_instance = None;
                    self.tray = None;
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn execute_action(&mut self, action: RowAction) {
        match action {
            RowAction::Show(hwnd) => match self.controller.show_window(hwnd, true) {
                Ok(()) => self.set_status("Window restored to Alt+Tab & Taskbar", false),
                Err(e) => self.set_status(e, true),
            },
            RowAction::Hide(hwnd, strategy) => {
                match self.controller.hide_window(hwnd, strategy) {
                    Ok(actual) => {
                        self.set_status(format!("Hidden ({})", actual.label()), false);
                    }
                    Err(e) => self.set_status(e, true),
                }
            }
            RowAction::Minimize(hwnd) => match self.controller.minimize_window(hwnd) {
                Ok(()) => self.set_status("Window minimized", false),
                Err(e) => self.set_status(e, true),
            },
            RowAction::HideAll(hwnds, strategy) => {
                let mut ok_count = 0usize;
                let mut last_err = None;
                for h in hwnds {
                    match self.controller.hide_window(h, strategy) {
                        Ok(_) => ok_count += 1,
                        Err(e) => last_err = Some(e),
                    }
                }
                if let Some(err) = last_err {
                    self.set_status(err, true);
                } else {
                    self.set_status(format!("Hidden {ok_count} windows"), false);
                }
            }
            RowAction::ShowAll(hwnds) => {
                for h in hwnds {
                    let _ = self.controller.show_window(h, false);
                }
                self.set_status("Restored process windows", false);
            }
            RowAction::RelaunchNoOcclusion(path) => match launch_without_occlusion(&path) {
                Ok(()) => self.set_status("Launched instance with anti-occlusion flags", false),
                Err(e) => self.set_status(e, true),
            },
        }
        self.refresh_windows();
    }

    fn draw_window_controls(
        ui: &mut egui::Ui,
        win: &WindowEntry,
        default_strategy: HideStrategy,
        exe_path: &str,
        actions: &mut Vec<RowAction>,
    ) {
        let is_visible = matches!(win.state, WindowState::Visible);
        let is_hidden = matches!(win.state, WindowState::StealthHidden(_));
        let is_minimized = matches!(win.state, WindowState::Minimized);
        let hwnd = win.hwnd_raw;

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;

            let menu_resp = ui
                .menu_button(
                    RichText::new("···").size(11.0).color(TEXT_SECONDARY),
                    |ui| {
                        if ui.button("Hide Corner 1px (Desktop-free, OBS OK)").clicked() {
                            actions.push(RowAction::Hide(hwnd, HideStrategy::CornerPeek));
                            ui.close();
                        }
                        if ui.button("Hide Invisible (Ghost Alpha 0)").clicked() {
                            actions.push(RowAction::Hide(hwnd, HideStrategy::LayeredGhost));
                            ui.close();
                        }
                        if ui.button("Hide Background (Live / WGC OK)").clicked() {
                            actions.push(RowAction::Hide(hwnd, HideStrategy::TaskbarCapturable));
                            ui.close();
                        }
                        if ui.button("Hide Strict Stealth (ToolWindow)").clicked() {
                            actions.push(RowAction::Hide(hwnd, HideStrategy::StealthBackground));
                            ui.close();
                        }
                        if ui.button("Move to Monitor 2 (Un-occluded)").clicked() {
                            actions.push(RowAction::Hide(hwnd, HideStrategy::SecondaryMonitor));
                            ui.close();
                        }
                        if !exe_path.is_empty() {
                            ui.separator();
                            if ui
                                .button("Launch New w/ Anti-Occlusion Flags")
                                .on_hover_text(
                                    "Starts a new Chromium/Electron instance with CalculateNativeWinOcclusion disabled",
                                )
                                .clicked()
                            {
                                actions.push(RowAction::RelaunchNoOcclusion(exe_path.to_owned()));
                                ui.close();
                            }
                        }
                    },
                )
                .response;
            menu_resp.on_hover_cursor(CursorIcon::PointingHand);

            if animated_pill(
                ui,
                ("min", hwnd),
                "Min",
                is_minimized,
                PillTone::SlateMin,
                Vec2::new(36.0, 21.0),
            )
            .on_hover_text("Standard OS minimize")
            .clicked()
            {
                actions.push(RowAction::Minimize(hwnd));
            }

            if animated_pill(
                ui,
                ("hide", hwnd),
                "Hide",
                is_hidden,
                PillTone::AmberGhost,
                Vec2::new(40.0, 21.0),
            )
            .on_hover_text("Hide via active mode (real-time WinEvent guard against Desktop / Win+D / focus steal)")
            .clicked()
            {
                actions.push(RowAction::Hide(hwnd, default_strategy));
            }

            if animated_pill(
                ui,
                ("show", hwnd),
                "Show",
                is_visible,
                PillTone::Primary,
                Vec2::new(42.0, 21.0),
            )
            .on_hover_text("Show & focus window")
            .clicked()
            {
                actions.push(RowAction::Show(hwnd));
            }
        });
    }
}

impl eframe::App for GhostDockApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.own_hwnd == 0 && self.dashboard_visible {
            let hwnd = find_own_visible_hwnd();
            if hwnd != 0 {
                style_own_window_dark(hwnd);
                self.own_hwnd = hwnd;
            }
        }

        self.handle_tray_events(ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            self.controller.restore_all();
            self.single_instance = None;
            self.tray = None;
            self.quitting = true;
        }

        let interval = if self.dashboard_visible {
            Duration::from_millis(800)
        } else {
            Duration::from_millis(1500)
        };
        if self.last_refresh.elapsed() >= interval {
            if self.dashboard_visible {
                self.refresh_windows();
            } else {
                self.controller.enforce_stealth_guards();
                self.last_refresh = Instant::now();
            }
        }

        ctx.request_repaint_after(interval);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut actions: Vec<RowAction> = Vec::new();
        let ctx = ui.ctx().clone();
        let search_id = ui.make_persistent_id("ghost_dock_search");

        // Global in-app keyboard shortcuts
        let wants_kb = ctx.egui_wants_keyboard_input();
        let (focus_search, clear_or_tray, manual_refresh) = ctx.input(|i| {
            let focus_f = (i.modifiers.ctrl && i.key_pressed(Key::F))
                || (!wants_kb && i.key_pressed(Key::Slash));
            let esc = i.key_pressed(Key::Escape);
            let refresh = i.key_pressed(Key::F5) || (i.modifiers.ctrl && i.key_pressed(Key::R));
            (focus_f, esc, refresh)
        });

        if manual_refresh {
            self.refresh_windows();
            self.set_status("Refreshed window list", false);
        }

        if clear_or_tray {
            if !self.search.is_empty() {
                self.search.clear();
            } else {
                self.dashboard_visible = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(BG_CANVAS)
                    .inner_margin(Margin::symmetric(14, 12)),
            )
            .show(ui, |ui| {
                // ── Top Header ────────────────────────────────────────────────
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(
                            RichText::new("GHOST DOCK")
                                .size(13.0)
                                .strong()
                                .color(TEXT_PRIMARY),
                        );
                        let total_wins: usize = self.groups.iter().map(|g| g.windows.len()).sum();
                        let hidden_cnt = self.controller.hidden_count();
                        ui.label(
                            RichText::new(format!(
                                "{} apps  ·  {} windows  ·  {} hidden",
                                self.groups.len(),
                                total_wins,
                                hidden_cnt
                            ))
                            .size(10.5)
                            .color(TEXT_SECONDARY),
                        );
                    });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;

                        if animated_pill(
                            ui,
                            "hdr_tray",
                            "Tray",
                            false,
                            PillTone::Neutral,
                            Vec2::new(44.0, 24.0),
                        )
                        .on_hover_text("Minimize GhostDock to System Tray (Ctrl+Shift+Space / Esc)")
                        .clicked()
                        {
                            self.dashboard_visible = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                        }

                        if self.controller.hidden_count() > 0
                            && animated_pill(
                                ui,
                                "hdr_restore",
                                "Restore All",
                                false,
                                PillTone::Neutral,
                                Vec2::new(72.0, 24.0),
                            )
                            .on_hover_text("Restore every stealth-hidden window")
                            .clicked()
                        {
                            self.controller.restore_all();
                            self.refresh_windows();
                            self.set_status("Restored all hidden windows", false);
                        }

                        if !self.self_elevated
                            && animated_pill(
                                ui,
                                "hdr_admin",
                                "Admin",
                                true,
                                PillTone::Danger,
                                Vec2::new(48.0, 24.0),
                            )
                            .on_hover_text(
                                "Restart as Administrator to control elevated apps (e.g. TikTok Live Studio)",
                            )
                            .clicked()
                        {
                            self.single_instance = None;
                            match relaunch_elevated() {
                                Ok(()) => {
                                    self.controller.restore_all();
                                    self.tray = None;
                                    self.quitting = true;
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                                Err(e) => {
                                    if let InstanceAcquisition::Primary(mut guard) =
                                        acquire_or_wake_existing()
                                        && let Some(tray) = &self.tray
                                    {
                                        let tx_wake = tray.tx.clone();
                                        let ctx_wake = ctx.clone();
                                        guard.start_listener(move || {
                                            let _ = tx_wake.send(TrayCommand::ShowDashboard);
                                            ctx_wake.request_repaint();
                                        });
                                        self.single_instance = Some(guard);
                                    }
                                    self.set_status(e, true);
                                }
                            }
                        }
                    });
                });

                ui.add_space(9.0);

                // ── Search & Segmented Hide Strategy Bar ──────────────────────
                ui.horizontal(|ui| {
                    let strat_reserve = 202.0;
                    let search_w = (ui.available_width() - strat_reserve).max(100.0);
                    let search_focused = ctx.memory(|m| m.has_focus(search_id));
                    let focus_t = ctx.animate_bool_with_time(
                        ui.make_persistent_id("search_focus_anim"),
                        search_focused,
                        0.12,
                    );
                    let search_border = lerp_color(BORDER_HAIRLINE, BORDER_HOVER, focus_t);

                    egui::Frame::NONE
                        .fill(BG_TRACK)
                        .stroke(Stroke::new(1.0, search_border))
                        .corner_radius(CornerRadius::same(5))
                        .inner_margin(Margin::symmetric(8, 3))
                        .show(ui, |ui| {
                            ui.set_width(search_w - 18.0);
                            ui.horizontal(|ui| {
                                let inner_w =
                                    (search_w - if self.search.is_empty() { 40.0 } else { 42.0 })
                                        .max(50.0);
                                let edit = egui::TextEdit::singleline(&mut self.search)
                                    .id(search_id)
                                    .frame(egui::Frame::NONE)
                                    .hint_text("Filter process or window... (/)")
                                    .desired_width(inner_w);
                                let resp = ui.add(edit);
                                if focus_search {
                                    resp.request_focus();
                                }
                                if !self.search.is_empty() {
                                    let clr = ui.add(
                                        egui::Button::new(
                                            RichText::new("×").size(12.0).color(TEXT_SECONDARY),
                                        )
                                        .frame(false),
                                    );
                                    if clr
                                        .on_hover_cursor(CursorIcon::PointingHand)
                                        .on_hover_text("Clear filter (Esc)")
                                        .clicked()
                                    {
                                        self.search.clear();
                                    }
                                }
                            });
                        });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        segmented_strategy(ui, &mut self.strategy, self.monitor_count > 1);
                    });
                });

                ui.add_space(9.0);

                // ── Process / Window List ─────────────────────────────────────
                let query = self.search.trim().to_ascii_lowercase();
                let footer_reserve = 26.0;
                let scroll_height = (ui.available_height() - footer_reserve).max(120.0);

                egui::ScrollArea::vertical()
                    .max_height(scroll_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 5.0;
                        let mut shown_any = false;

                        for group in &self.groups {
                            let proc_match = query.is_empty()
                                || group.exe_name.to_ascii_lowercase().contains(&query);
                            let matching_windows: Vec<&WindowEntry> = group
                                .windows
                                .iter()
                                .filter(|w| {
                                    proc_match || w.title.to_ascii_lowercase().contains(&query)
                                })
                                .collect();
                            if matching_windows.is_empty() {
                                continue;
                            }
                            shown_any = true;
                            let needs_admin = group.is_elevated && !self.self_elevated;

                            let card_id = ui.make_persistent_id(("card", group.pid));
                            let last_rect = ui.ctx().data(|d| d.get_temp::<egui::Rect>(card_id));
                            let card_hovered = last_rect
                                .is_some_and(|r| ui.rect_contains_pointer(r));
                            let card_hov_t = ui.ctx().animate_bool_with_time(
                                ui.make_persistent_id(("card_hov", group.pid)),
                                card_hovered,
                                0.12,
                            );

                            let card_fill = lerp_color(BG_SURFACE, BG_SURFACE_HOVER, card_hov_t);
                            let card_border =
                                lerp_color(BORDER_HAIRLINE, BORDER_HOVER, card_hov_t);

                            let frame_resp = egui::Frame::NONE
                                .fill(card_fill)
                                .stroke(Stroke::new(1.0, card_border))
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(Margin::symmetric(10, 7))
                                .show(ui, |ui| {
                                    if matching_windows.len() == 1 && group.windows.len() == 1 {
                                        let win = matching_windows[0];
                                        ui.horizontal(|ui| {
                                            let dot_col = match win.state {
                                                WindowState::Visible => DOT_LIVE,
                                                WindowState::StealthHidden(s)
                                                    if !s.hides_from_alt_tab() =>
                                                {
                                                    DOT_CAPTURABLE
                                                }
                                                WindowState::StealthHidden(_) => DOT_GHOST,
                                                WindowState::Minimized => DOT_MIN,
                                            };
                                            draw_status_dot(ui, dot_col, 2.8);

                                            let label_w = (ui.available_width() - 168.0).max(70.0);
                                            ui.allocate_ui_with_layout(
                                                Vec2::new(label_w, 22.0),
                                                Layout::left_to_right(Align::Center),
                                                |ui| {
                                                    let clean_name = group
                                                        .exe_name
                                                        .strip_suffix(".exe")
                                                        .or_else(|| {
                                                            group.exe_name.strip_suffix(".EXE")
                                                        })
                                                        .unwrap_or(&group.exe_name);
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(clean_name)
                                                                .size(12.0)
                                                                .strong()
                                                                .color(TEXT_PRIMARY),
                                                        )
                                                        .truncate(),
                                                    )
                                                    .on_hover_text(format!(
                                                        "{}\nPID {} · {}",
                                                        win.title, group.pid, group.exe_path
                                                    ));
                                                    if needs_admin {
                                                        ui.label(
                                                            RichText::new("ADMIN")
                                                                .size(9.0)
                                                                .color(BADGE_WARN),
                                                        );
                                                    }
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(format!(
                                                                "· {}",
                                                                win.title
                                                            ))
                                                            .size(11.0)
                                                            .color(TEXT_SECONDARY),
                                                        )
                                                        .truncate(),
                                                    )
                                                    .on_hover_text(&win.title);
                                                },
                                            );

                                            Self::draw_window_controls(
                                                ui,
                                                win,
                                                self.strategy,
                                                &group.exe_path,
                                                &mut actions,
                                            );
                                        });
                                    } else {
                                        ui.horizontal(|ui| {
                                            let clean_name = group
                                                .exe_name
                                                .strip_suffix(".exe")
                                                .or_else(|| group.exe_name.strip_suffix(".EXE"))
                                                .unwrap_or(&group.exe_name);
                                            ui.label(
                                                RichText::new(clean_name)
                                                    .size(12.0)
                                                    .strong()
                                                    .color(TEXT_PRIMARY),
                                            )
                                            .on_hover_text(format!(
                                                "PID {} · {}",
                                                group.pid, group.exe_path
                                            ));
                                            ui.label(
                                                RichText::new(format!(
                                                    "· {} win",
                                                    group.windows.len()
                                                ))
                                                .size(10.5)
                                                .color(TEXT_MUTED),
                                            );
                                            if needs_admin {
                                                ui.label(
                                                    RichText::new("ADMIN")
                                                        .size(9.0)
                                                        .color(BADGE_WARN),
                                                );
                                            }
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    ui.spacing_mut().item_spacing.x = 4.0;
                                                    let hwnds: Vec<isize> = matching_windows
                                                        .iter()
                                                        .map(|w| w.hwnd_raw)
                                                        .collect();
                                                    if animated_pill(
                                                        ui,
                                                        ("hide_all", group.pid),
                                                        "Hide All",
                                                        group.hidden_count() == group.windows.len(),
                                                        PillTone::AmberGhost,
                                                        Vec2::new(52.0, 20.0),
                                                    )
                                                    .clicked()
                                                    {
                                                        actions.push(RowAction::HideAll(
                                                            hwnds.clone(),
                                                            self.strategy,
                                                        ));
                                                    }
                                                    if group.hidden_count() > 0
                                                        && animated_pill(
                                                            ui,
                                                            ("show_all", group.pid),
                                                            "Show All",
                                                            false,
                                                            PillTone::Neutral,
                                                            Vec2::new(54.0, 20.0),
                                                        )
                                                        .clicked()
                                                    {
                                                        actions.push(RowAction::ShowAll(hwnds));
                                                    }
                                                },
                                            );
                                        });

                                        ui.add_space(4.0);
                                        for win in matching_windows {
                                            ui.horizontal(|ui| {
                                                ui.add_space(4.0);
                                                let dot_col = match win.state {
                                                    WindowState::Visible => DOT_LIVE,
                                                    WindowState::StealthHidden(s)
                                                        if !s.hides_from_alt_tab() =>
                                                    {
                                                        DOT_CAPTURABLE
                                                    }
                                                    WindowState::StealthHidden(_) => DOT_GHOST,
                                                    WindowState::Minimized => DOT_MIN,
                                                };
                                                draw_status_dot(ui, dot_col, 2.4);

                                                let title_w =
                                                    (ui.available_width() - 164.0).max(60.0);
                                                ui.allocate_ui_with_layout(
                                                    Vec2::new(title_w, 20.0),
                                                    Layout::left_to_right(Align::Center),
                                                    |ui| {
                                                        ui.add(
                                                            egui::Label::new(
                                                                RichText::new(&win.title)
                                                                    .size(11.0)
                                                                    .color(TEXT_SECONDARY),
                                                            )
                                                            .truncate(),
                                                        )
                                                        .on_hover_text(&win.title);
                                                    },
                                                );

                                                Self::draw_window_controls(
                                                    ui,
                                                    win,
                                                    self.strategy,
                                                    &group.exe_path,
                                                    &mut actions,
                                                );
                                            });
                                        }
                                    }
                                });

                            ui.ctx()
                                .data_mut(|d| d.insert_temp(card_id, frame_resp.response.rect));
                        }

                        if !shown_any {
                            ui.add_space(28.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new("No matching active windows")
                                        .size(12.0)
                                        .color(TEXT_SECONDARY),
                                );
                            });
                        }
                    });

                // ── Footer Status Strip ───────────────────────────────────────
                ui.add_space(5.0);
                ui.horizontal(|ui| {
                    if let Some((text, is_err, ts)) = &self.status_msg {
                        let age = ts.elapsed().as_secs_f32();
                        if age < 3.8 {
                            let alpha = if age > 3.0 {
                                ((3.8 - age) / 0.8).clamp(0.0, 1.0)
                            } else {
                                1.0
                            };
                            let dot_col = if *is_err { BADGE_WARN } else { DOT_LIVE };
                            let txt_col = if *is_err { BADGE_WARN } else { TEXT_PRIMARY };
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                            ui.painter().circle_filled(
                                r.center(),
                                2.5,
                                Color32::from_rgba_unmultiplied(
                                    dot_col.r(),
                                    dot_col.g(),
                                    dot_col.b(),
                                    (alpha * 255.0) as u8,
                                ),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(text).size(10.5).color(
                                        Color32::from_rgba_unmultiplied(
                                            txt_col.r(),
                                            txt_col.g(),
                                            txt_col.b(),
                                            (alpha * 255.0) as u8,
                                        ),
                                    ),
                                )
                                .truncate(),
                            );
                            ctx.request_repaint();
                            return;
                        }
                    }

                    ui.add(
                        egui::Label::new(
                            RichText::new("Ctrl+Shift+Space: Tray  ·  /: Filter  ·  Esc: Clear/Tray  ·  F5: Refresh")
                                .size(10.5)
                                .color(TEXT_MUTED),
                        )
                        .truncate(),
                    );
                });
            });

        for act in actions {
            self.execute_action(act);
        }
    }
}
