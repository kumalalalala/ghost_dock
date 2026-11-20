#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use eframe::egui;
use ghost_dock::app::GhostDockApp;
use ghost_dock::icon::render_ghost_dock_rgba;
use ghost_dock::single_instance::{InstanceAcquisition, acquire_or_wake_existing};

fn build_viewport_icon() -> egui::IconData {
    let size = 64u32;
    egui::IconData {
        rgba: render_ghost_dock_rgba(size),
        width: size,
        height: size,
    }
}

fn main() -> eframe::Result {
    let instance_guard = match acquire_or_wake_existing() {
        InstanceAcquisition::Primary(guard) => guard,
        InstanceAcquisition::SecondarySignaled => return Ok(()),
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("GhostDock")
            .with_inner_size([480.0, 560.0])
            .with_min_inner_size([380.0, 320.0])
            .with_icon(Arc::new(build_viewport_icon())),
        ..Default::default()
    };

    let mut guard_slot = Some(instance_guard);
    eframe::run_native(
        "GhostDock",
        options,
        Box::new(move |cc| Ok(Box::new(GhostDockApp::new(cc, guard_slot.take())))),
    )
}
