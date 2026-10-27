use std::{
    sync::{
        mpsc::{self, Sender},
        Arc, Mutex, MutexGuard, PoisonError,
    },
    thread,
    time::Duration,
};

use serde::Serialize;
#[cfg(target_os = "macos")]
use tauri::tray::TrayIconEvent;
#[cfg(target_os = "macos")]
use tauri::LogicalPosition;
use tauri::{
    webview::Color, AppHandle, Emitter, LogicalSize, Manager, Theme, WebviewWindow, Window,
    WindowEvent,
};
#[cfg(target_os = "windows")]
use tauri_plugin_positioner::{Position, WindowExt};

use crate::{
    desktop_integration::DesktopIntegration,
    models::{ThemePreference, WindowMode},
    popup::PopupDismissGuard,
    settings::SettingsService,
    storage::Storage,
};

pub const MAIN_WINDOW: &str = "main";
pub const PANEL_WIDTH: f64 = 320.0;
pub const PANEL_MIN_HEIGHT: u32 = 240;
const PANEL_SCREEN_FRACTION: f64 = 0.85;
const PANEL_RESIZE_SAVE_DELAY: Duration = Duration::from_millis(120);
const LIGHT_PANEL_SURFACE: Color = Color(0xff, 0xff, 0xff, 0xff);
const DARK_PANEL_SURFACE: Color = Color(0x1d, 0x1d, 0x1f, 0xff);

#[derive(Clone, Copy)]
struct PendingPanelHeight {
    generation: u64,
    height: u32,
}

struct PanelState {
    active: bool,
    generation: u64,
    latest: Option<u32>,
    mode: PanelHeightMode,
}

pub struct PanelResizeSession {
    state: Arc<Mutex<PanelState>>,
    height_sender: Sender<PendingPanelHeight>,
    storage: Arc<Storage>,
}

pub(crate) struct PanelResetToken {
    previous_height: Option<u32>,
    generation: u64,
}

impl PanelResizeSession {
    fn lock_state(&self) -> MutexGuard<'_, PanelState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn new(storage: Arc<Storage>) -> Self {
        let mode = match storage.load_panel_height() {
            Ok(Some(_)) => PanelHeightMode::Manual,
            _ => PanelHeightMode::Automatic,
        };
        let (height_sender, height_receiver) = mpsc::channel::<PendingPanelHeight>();
        let state = Arc::new(Mutex::new(PanelState {
            active: false,
            generation: 0,
            latest: None,
            mode,
        }));
        let worker_state = state.clone();
        let worker_storage = storage.clone();
        thread::spawn(move || {
            while let Ok(mut pending) = height_receiver.recv() {
                while let Ok(next) = height_receiver.recv_timeout(PANEL_RESIZE_SAVE_DELAY) {
                    pending = next;
                }
                let panel = worker_state.lock().unwrap_or_else(PoisonError::into_inner);
                if pending.generation == panel.generation && panel.mode == PanelHeightMode::Manual {
                    let _ = worker_storage.save_panel_height(pending.height);
                }
            }
        });
        Self {
            state,
            height_sender,
            storage,
        }
    }

    fn begin(&self, height: u32) -> Result<(), String> {
        let mut panel = self.lock_state();
        panel.active = true;
        panel.latest = None;
        match self.storage.save_panel_height(height) {
            Ok(()) => {
                panel.generation += 1;
                panel.mode = PanelHeightMode::Manual;
                Ok(())
            }
            Err(_) => {
                panel.active = false;
                Err("QuotaGauge panel state could not be saved.".to_owned())
            }
        }
    }

    pub fn finish(&self, current_height: Option<u32>) {
        let mut panel = self.lock_state();
        let was_active = std::mem::replace(&mut panel.active, false);
        let recorded_height = panel.latest.take();
        let Some(height) = current_height.or(recorded_height) else {
            return;
        };
        if !was_active {
            return;
        }
        if panel.mode == PanelHeightMode::Manual && self.storage.save_panel_height(height).is_ok() {
            panel.generation += 1;
        }
    }

    pub fn set_manual(&self, height: u32) -> Result<(), String> {
        let mut panel = self.lock_state();
        self.storage
            .save_panel_height(height)
            .map_err(|_| "QuotaGauge panel state could not be saved.".to_owned())?;
        panel.generation += 1;
        panel.mode = PanelHeightMode::Manual;
        Ok(())
    }

    fn record(&self, height: u32) {
        let generation = {
            let mut panel = self.lock_state();
            if !panel.active {
                return;
            }
            panel.latest = Some(height);
            panel.generation
        };
        let _ = self
            .height_sender
            .send(PendingPanelHeight { generation, height });
    }

    pub fn mode(&self) -> PanelHeightMode {
        self.lock_state().mode
    }

    fn allows_automatic_fit(&self) -> bool {
        let panel = self.lock_state();
        panel.mode == PanelHeightMode::Automatic && !panel.active
    }

    pub fn set_automatic(&self) -> Result<(), String> {
        self.begin_automatic_reset().map(|_| ())
    }

    pub(crate) fn begin_automatic_reset(&self) -> Result<PanelResetToken, String> {
        let mut panel = self.lock_state();
        let previous_height = self
            .storage
            .load_panel_height()
            .map_err(|_| "QuotaGauge panel state could not be loaded.".to_owned())?;
        self.storage
            .clear_panel_height()
            .map_err(|_| "QuotaGauge panel state could not be saved.".to_owned())?;
        panel.active = false;
        panel.latest = None;
        panel.generation += 1;
        panel.mode = PanelHeightMode::Automatic;
        Ok(PanelResetToken {
            previous_height,
            generation: panel.generation,
        })
    }

    pub(crate) fn rollback_automatic_reset(&self, token: PanelResetToken) -> Result<bool, String> {
        let mut panel = self.lock_state();
        if panel.generation != token.generation || panel.mode != PanelHeightMode::Automatic {
            return Ok(false);
        }
        match token.previous_height {
            Some(height) => {
                self.storage
                    .save_panel_height(height)
                    .map_err(|_| "QuotaGauge panel state could not be restored.".to_owned())?;
                panel.mode = PanelHeightMode::Manual;
            }
            None => {
                self.storage
                    .clear_panel_height()
                    .map_err(|_| "QuotaGauge panel state could not be restored.".to_owned())?;
                panel.mode = PanelHeightMode::Automatic;
            }
        }
        panel.generation += 1;
        Ok(true)
    }

    fn saved_height(&self) -> Option<u32> {
        self.storage.load_panel_height().ok().flatten()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelHeightMode {
    Automatic,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelResizeEdge {
    Top,
    Bottom,
}

fn panel_surface_color(preference: ThemePreference, system_theme: Theme) -> Color {
    match preference {
        ThemePreference::Dark => DARK_PANEL_SURFACE,
        ThemePreference::Light => LIGHT_PANEL_SURFACE,
        ThemePreference::System if system_theme == Theme::Dark => DARK_PANEL_SURFACE,
        ThemePreference::System => LIGHT_PANEL_SURFACE,
    }
}

fn apply_panel_surface_for_theme(
    window: &WebviewWindow,
    preference: ThemePreference,
    system_theme: Theme,
) -> tauri::Result<()> {
    window.set_background_color(Some(panel_surface_color(preference, system_theme)))
}

pub fn apply_panel_surface(
    window: &WebviewWindow,
    preference: ThemePreference,
) -> tauri::Result<()> {
    let native_theme = match preference {
        ThemePreference::Dark => Some(Theme::Dark),
        ThemePreference::Light => Some(Theme::Light),
        ThemePreference::System => None,
    };
    window.set_theme(native_theme)?;
    apply_panel_surface_for_theme(window, preference, window.theme().unwrap_or(Theme::Light))
}

/// Brings the already-running application forward when a later launch is redirected to it by the
/// single-instance plugin. During an extremely tight simultaneous-launch race the callback can arrive
/// before setup has installed the popup state; the fallback still reveals and focuses the window, while
/// the normal path preserves tray positioning and cancels any pending focus-loss dismissal.
pub fn activate_existing_instance(app: &AppHandle) {
    if let Some(guard) = app.try_state::<PopupDismissGuard>() {
        guard.cancel_pending();
    }

    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    if app.try_state::<DesktopIntegration>().is_some() {
        show_main_window(&window);
        return;
    }

    crate::webview_memory::set_inactive(&window, false);
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq)]
struct PointRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl PointRect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct ScreenPoints {
    bounds: PointRect,
    work: PointRect,
    scale: f64,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct PopupFrame {
    x: f64,
    y: f64,
    height: f64,
    max_height: f64,
}

#[derive(Default)]
pub struct TrayAnchor(Mutex<Option<PointRect>>);

impl TrayAnchor {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn store(&self, rect: Option<PointRect>) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = rect;
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn load(&self) -> Option<PointRect> {
        self.0.lock().ok().and_then(|slot| *slot)
    }
}

// tray-icon reports the icon rect in its own screen's backing pixels while every placement API
// downstream consumes logical points; only the cursor reliably identifies which screen to divide by.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn tray_icon_points(
    icon_px: PointRect,
    cursor: (f64, f64),
    screens: &[ScreenPoints],
) -> Option<PointRect> {
    let screen = screens
        .iter()
        .find(|screen| screen.bounds.contains(cursor.0, cursor.1))?;
    let scale = screen.scale;
    let icon = PointRect {
        x: icon_px.x / scale,
        y: icon_px.y / scale,
        width: icon_px.width / scale,
        height: icon_px.height / scale,
    };
    let (center_x, center_y) = icon.center();
    if screen.bounds.contains(center_x, center_y) {
        return Some(icon);
    }
    Some(PointRect {
        x: cursor.0,
        y: screen.bounds.y,
        width: 0.0,
        height: screen.work.y - screen.bounds.y,
    })
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn popup_frame(
    anchor: PointRect,
    screens: &[ScreenPoints],
    width: f64,
    wanted_height: f64,
) -> Option<PopupFrame> {
    let (center_x, center_y) = anchor.center();
    let work = screens
        .iter()
        .find(|screen| screen.bounds.contains(center_x, center_y))?
        .work;
    let right_limit = (work.x + work.width - width).max(work.x);
    let x = (center_x - width / 2.0).clamp(work.x, right_limit);
    let y = (anchor.y + anchor.height).max(work.y);
    let room = work.y + work.height - y;
    let max_height = room
        .min(work.height * PANEL_SCREEN_FRACTION)
        .floor()
        .max(1.0);
    let height = wanted_height.clamp(f64::from(PANEL_MIN_HEIGHT).min(max_height), max_height);
    Some(PopupFrame {
        x,
        y,
        height,
        max_height,
    })
}

#[cfg(target_os = "macos")]
fn screen_points(monitor: &tauri::Monitor) -> ScreenPoints {
    let scale = monitor.scale_factor();
    let position = monitor.position();
    let size = monitor.size();
    let work = monitor.work_area();
    let divide = |x: i32, y: i32, width: u32, height: u32| PointRect {
        x: f64::from(x) / scale,
        y: f64::from(y) / scale,
        width: f64::from(width) / scale,
        height: f64::from(height) / scale,
    };
    ScreenPoints {
        bounds: divide(position.x, position.y, size.width, size.height),
        work: divide(
            work.position.x,
            work.position.y,
            work.size.width,
            work.size.height,
        ),
        scale,
    }
}

#[cfg(target_os = "macos")]
fn screens_points(app: &AppHandle) -> Vec<ScreenPoints> {
    app.available_monitors()
        .map(|monitors| monitors.iter().map(screen_points).collect())
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn cursor_points(app: &AppHandle) -> Option<(f64, f64)> {
    // tao reports the cursor in primary-screen-scaled pixels; the primary scale converts it back.
    let scale = app.primary_monitor().ok()??.scale_factor();
    let cursor = app.cursor_position().ok()?;
    Some((cursor.x / scale, cursor.y / scale))
}

#[cfg(target_os = "macos")]
pub fn record_tray_anchor(app: &AppHandle, event: &TrayIconEvent) {
    let rect = match event {
        TrayIconEvent::Click { rect, .. } | TrayIconEvent::Enter { rect, .. } => rect,
        _ => return,
    };
    let position = rect.position.to_physical(1.0);
    let size = rect.size.to_physical(1.0);
    let icon = PointRect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };
    let anchor =
        cursor_points(app).and_then(|cursor| tray_icon_points(icon, cursor, &screens_points(app)));
    if let Some(slot) = app.try_state::<TrayAnchor>() {
        slot.store(anchor);
    }
}

#[cfg(target_os = "linux")]
fn position_popup(window: &WebviewWindow) {
    let _ = window.center();
}

#[cfg(target_os = "windows")]
fn position_popup(window: &WebviewWindow) {
    let _ = window
        .as_ref()
        .window()
        .move_window_constrained(Position::TrayCenter);
}

#[cfg(target_os = "macos")]
fn position_popup(window: &WebviewWindow) {
    let app = window.app_handle();
    let screens = screens_points(app);
    let anchor = app
        .try_state::<TrayAnchor>()
        .and_then(|slot| slot.load())
        .or_else(|| {
            let cursor = cursor_points(app)?;
            let work = screens
                .iter()
                .find(|screen| screen.bounds.contains(cursor.0, cursor.1))
                .map(|screen| screen.work)?;
            Some(PointRect {
                x: work.x + work.width - PANEL_WIDTH,
                y: work.y,
                width: 0.0,
                height: 0.0,
            })
        });
    let wanted = app
        .try_state::<Arc<PanelResizeSession>>()
        .and_then(|session| session.saved_height())
        .or_else(|| current_logical_height(&window.as_ref().window()))
        .map_or(f64::from(PANEL_MIN_HEIGHT), f64::from);
    let Some(frame) = anchor.and_then(|anchor| popup_frame(anchor, &screens, PANEL_WIDTH, wanted))
    else {
        return;
    };
    let minimum = f64::from(PANEL_MIN_HEIGHT).min(frame.max_height);
    let _ = window.set_max_size(Some(LogicalSize::new(PANEL_WIDTH, frame.max_height)));
    let _ = window.set_min_size(Some(LogicalSize::new(PANEL_WIDTH, minimum)));
    let _ = window.set_size(LogicalSize::new(PANEL_WIDTH, frame.height));
    let _ = window.set_position(LogicalPosition::new(frame.x, frame.y));
}

pub fn show_main_window(window: &WebviewWindow) {
    finish_native_panel_resize(window);
    crate::webview_memory::set_inactive(window, false);
    if window
        .app_handle()
        .state::<DesktopIntegration>()
        .is_floating()
    {
        let _ = window.unminimize();
        let _ = restore_manual_panel_height(window);
    } else {
        position_popup(window);
        // The macOS placement above lands asynchronously on the main run loop; a restore here
        // would re-anchor the panel from stale pre-move geometry and pull it to the old screen.
        #[cfg(not(target_os = "macos"))]
        {
            let _ = restore_manual_panel_height(window);
        }
    }
    let _ = window.show();
    let _ = window.set_focus();
}

fn set_window_chrome(window: &WebviewWindow, floating: bool) -> tauri::Result<()> {
    window
        .set_resizable(false)
        .and_then(|_| window.set_skip_taskbar(!floating))
        .and_then(|_| window.set_always_on_top(!floating))
        .and_then(|_| window.set_decorations(false))
}

pub fn apply_window_mode(
    window: &WebviewWindow,
    mode: WindowMode,
    center_floating: bool,
) -> Result<(), String> {
    let integration = window.app_handle().state::<DesktopIntegration>();
    let previous_floating = integration.is_floating();
    let floating = integration.floats_for(mode);

    window
        .app_handle()
        .state::<PopupDismissGuard>()
        .cancel_pending();
    finish_native_panel_resize(window);
    if set_window_chrome(window, floating).is_err() {
        let _ = set_window_chrome(window, previous_floating);
        return Err("QuotaGauge window mode could not be changed.".to_owned());
    }

    integration.apply_window_mode(mode);
    window
        .app_handle()
        .state::<PopupDismissGuard>()
        .cancel_pending();
    let result = {
        crate::webview_memory::set_inactive(window, false);
        let _ = window.unminimize();
        if floating {
            if center_floating {
                let _ = window.center();
            }
        } else {
            position_popup(window);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = restore_manual_panel_height(window);
        }
        window
            .show()
            .and_then(|_| window.set_focus())
            .map_err(|_| "QuotaGauge window could not be shown.".to_owned())
    };
    if result.is_err() {
        integration.set_floating(previous_floating);
        let _ = set_window_chrome(window, previous_floating);
    }
    result
}

fn hide_main_native_window(window: &Window) {
    if window.hide().is_err() {
        return;
    }
    if let Some(webview) = window.app_handle().get_webview_window(MAIN_WINDOW) {
        crate::webview_memory::set_inactive(&webview, true);
    }
    let _ = window.app_handle().emit("main-window-hidden", ());
}

pub fn hide_main_window(window: &WebviewWindow) {
    finish_native_panel_resize(window);
    hide_main_native_window(&window.as_ref().window());
}

pub fn toggle_main_window(app: &AppHandle) {
    app.state::<PopupDismissGuard>().cancel_pending();

    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };

    let visible = window.is_visible().unwrap_or(false);
    let minimized = window.is_minimized().unwrap_or(false);
    if visible && !minimized {
        hide_main_window(&window);
    } else {
        show_main_window(&window);
    }
}

pub fn open_screen(app: &AppHandle, screen: &str) {
    app.state::<PopupDismissGuard>().cancel_pending();
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        show_main_window(&window);
        let _ = app.emit("open-screen", screen);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VerticalFrame {
    top: i32,
    height: u32,
}

fn panel_resize_edge_for_frames(
    current: VerticalFrame,
    work_area: VerticalFrame,
) -> PanelResizeEdge {
    let current_bottom = i64::from(current.top) + i64::from(current.height);
    let work_bottom = i64::from(work_area.top) + i64::from(work_area.height);
    let top_gap = (i64::from(current.top) - i64::from(work_area.top)).abs();
    let bottom_gap = (work_bottom - current_bottom).abs();
    if bottom_gap <= top_gap {
        PanelResizeEdge::Top
    } else {
        PanelResizeEdge::Bottom
    }
}

fn panel_resize_edge_for_context(
    current: VerticalFrame,
    work_area: VerticalFrame,
    floating: bool,
) -> PanelResizeEdge {
    if floating {
        PanelResizeEdge::Bottom
    } else {
        panel_resize_edge_for_frames(current, work_area)
    }
}

fn anchored_vertical_frame(
    current: VerticalFrame,
    work_area: VerticalFrame,
    new_height: u32,
) -> VerticalFrame {
    let current_bottom = i64::from(current.top) + i64::from(current.height);
    let top = match panel_resize_edge_for_frames(current, work_area) {
        PanelResizeEdge::Top => current_bottom.saturating_sub(i64::from(new_height)),
        PanelResizeEdge::Bottom => i64::from(current.top),
    };
    VerticalFrame {
        top: top.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        height: new_height,
    }
}

pub fn panel_resize_edge(window: &WebviewWindow) -> Result<PanelResizeEdge, String> {
    let position = window
        .outer_position()
        .map_err(|_| "QuotaGauge window position is unavailable.")?;
    let size = window
        .outer_size()
        .map_err(|_| "QuotaGauge window size is unavailable.")?;
    let monitor = window
        .current_monitor()
        .map_err(|_| "QuotaGauge display is unavailable.")?
        .ok_or("QuotaGauge display is unavailable.")?;
    let work_area = monitor.work_area();
    Ok(panel_resize_edge_for_context(
        VerticalFrame {
            top: position.y,
            height: size.height,
        },
        VerticalFrame {
            top: work_area.position.y,
            height: work_area.size.height,
        },
        window
            .app_handle()
            .state::<DesktopIntegration>()
            .is_floating(),
    ))
}

fn panel_maximum_height(window: &WebviewWindow) -> Result<u32, String> {
    let position = window
        .outer_position()
        .map_err(|_| "QuotaGauge window position is unavailable.")?;
    let outer_size = window
        .outer_size()
        .map_err(|_| "QuotaGauge window size is unavailable.")?;
    let inner_size = window
        .inner_size()
        .map_err(|_| "QuotaGauge content size is unavailable.")?;
    let scale = window
        .scale_factor()
        .map_err(|_| "QuotaGauge display scale is unavailable.")?;
    let monitor = window
        .current_monitor()
        .map_err(|_| "QuotaGauge display is unavailable.")?
        .ok_or("QuotaGauge display is unavailable.")?;
    let work_area = monitor.work_area();
    let current = VerticalFrame {
        top: position.y,
        height: outer_size.height,
    };
    let work = VerticalFrame {
        top: work_area.position.y,
        height: work_area.size.height,
    };
    let current_bottom = i64::from(current.top) + i64::from(current.height);
    let work_bottom = i64::from(work.top) + i64::from(work.height);
    let room = match panel_resize_edge_for_context(
        current,
        work,
        window
            .app_handle()
            .state::<DesktopIntegration>()
            .is_floating(),
    ) {
        PanelResizeEdge::Top => current_bottom.saturating_sub(i64::from(work.top)),
        PanelResizeEdge::Bottom => work_bottom.saturating_sub(i64::from(current.top)),
    }
    .max(1) as f64;
    let aesthetic_cap = f64::from(work.height) * PANEL_SCREEN_FRACTION;
    let frame_overhead = outer_size.height.saturating_sub(inner_size.height);
    let inner_cap =
        room.min(aesthetic_cap).max(f64::from(frame_overhead) + 1.0) - f64::from(frame_overhead);
    Ok((inner_cap / scale).floor().clamp(1.0, f64::from(u32::MAX)) as u32)
}

fn configure_panel_size_constraints(window: &WebviewWindow) -> Result<u32, String> {
    let maximum = panel_maximum_height(window)?;
    let minimum = PANEL_MIN_HEIGHT.min(maximum);
    window
        .set_max_size(Some(LogicalSize::new(PANEL_WIDTH, f64::from(maximum))))
        .and_then(|_| window.set_min_size(Some(LogicalSize::new(PANEL_WIDTH, f64::from(minimum)))))
        .map_err(|_| "QuotaGauge panel size limits could not be applied.".to_owned())?;
    Ok(maximum)
}

fn restore_manual_panel_height(window: &WebviewWindow) -> Result<(), String> {
    let maximum = panel_maximum_height(window)?;
    let minimum = PANEL_MIN_HEIGHT.min(maximum);
    let saved = window
        .app_handle()
        .try_state::<Arc<PanelResizeSession>>()
        .and_then(|session| session.saved_height());
    if let Some(saved) = saved {
        resize_panel_for_context(window, saved.clamp(minimum, maximum))?;
    }
    configure_panel_size_constraints(window)?;
    Ok(())
}

fn resize_panel_for_context(window: &WebviewWindow, height: u32) -> Result<(), String> {
    if window
        .app_handle()
        .state::<DesktopIntegration>()
        .is_floating()
    {
        return window
            .set_size(LogicalSize::new(PANEL_WIDTH, f64::from(height)))
            .map_err(|_| "QuotaGauge window could not be resized.".to_owned());
    }
    resize_popup_anchored(window, height)
}

pub fn fit_panel_to_content(window: &WebviewWindow, height: u32) -> Result<bool, String> {
    let Some(session) = window.app_handle().try_state::<Arc<PanelResizeSession>>() else {
        return Ok(false);
    };
    if !session.allows_automatic_fit() {
        return Ok(false);
    }
    // Show/open and native drag setup already install the constraints. Auto-fit only needs the
    // current monitor clamp here; reapplying native min/max constraints on every animation frame
    // causes unnecessary platform window-manager work.
    let maximum = panel_maximum_height(window)?;
    let minimum = PANEL_MIN_HEIGHT.min(maximum);
    resize_panel_for_context(window, height.clamp(minimum, maximum))?;
    Ok(true)
}

pub fn prepare_native_panel_resize(window: &WebviewWindow) -> Result<PanelResizeEdge, String> {
    let edge = panel_resize_edge(window)?;
    configure_panel_size_constraints(window)?;
    window
        .set_resizable(true)
        .map_err(|_| "QuotaGauge panel resize could not be enabled.".to_owned())?;
    if let Some(session) = window.app_handle().try_state::<Arc<PanelResizeSession>>() {
        let height = current_logical_height(&window.as_ref().window())
            .ok_or("QuotaGauge content size is unavailable.")?;
        session.begin(height)?;
    }
    Ok(edge)
}

pub fn set_manual_panel_height(window: &WebviewWindow) -> Result<(), String> {
    let height = current_logical_height(&window.as_ref().window())
        .ok_or("QuotaGauge content size is unavailable.")?;
    window
        .app_handle()
        .state::<Arc<PanelResizeSession>>()
        .set_manual(height)
}

pub fn finish_native_panel_resize(window: &WebviewWindow) {
    if let Some(session) = window.app_handle().try_state::<Arc<PanelResizeSession>>() {
        session.finish(current_logical_height(&window.as_ref().window()));
    }
    let _ = lock_native_panel_resize_axis(window);
}

pub fn lock_native_panel_resize_axis(window: &WebviewWindow) -> Result<(), String> {
    // Keep the system's invisible left/right resize borders disabled outside the explicit vertical
    // gesture. Re-applying the logical width also repairs any transient WebView viewport change if a
    // platform briefly reported a horizontal resize before the native constraint took effect.
    window
        .set_resizable(false)
        .map_err(|_| "QuotaGauge panel resize could not be settled.".to_owned())?;
    let size = window
        .inner_size()
        .map_err(|_| "QuotaGauge content size is unavailable.")?;
    let scale = window
        .scale_factor()
        .map_err(|_| "QuotaGauge display scale is unavailable.")?;
    let height = f64::from(size.height) / scale;
    window
        .set_size(LogicalSize::new(PANEL_WIDTH, height))
        .map_err(|_| "QuotaGauge panel resize could not be settled.".to_owned())
}

fn current_logical_height(window: &Window) -> Option<u32> {
    let size = window.inner_size().ok()?;
    let scale = window.scale_factor().ok()?;
    Some(
        (f64::from(size.height) / scale)
            .round()
            .clamp(1.0, f64::from(u32::MAX)) as u32,
    )
}

struct PopupAnchoring {
    outer_x: i32,
    anchored: VerticalFrame,
}

fn anchored_popup_frame(
    window: &WebviewWindow,
    target_outer_height: u32,
) -> Result<PopupAnchoring, String> {
    let outer_position = window
        .outer_position()
        .map_err(|_| "QuotaGauge window position is unavailable.")?;
    let outer_size = window
        .outer_size()
        .map_err(|_| "QuotaGauge window size is unavailable.")?;
    let monitor = window
        .current_monitor()
        .map_err(|_| "QuotaGauge display is unavailable.")?
        .ok_or("QuotaGauge display is unavailable.")?;
    let work_area = monitor.work_area();
    let anchored = anchored_vertical_frame(
        VerticalFrame {
            top: outer_position.y,
            height: outer_size.height,
        },
        VerticalFrame {
            top: work_area.position.y,
            height: work_area.size.height,
        },
        target_outer_height,
    );
    Ok(PopupAnchoring {
        outer_x: outer_position.x,
        anchored,
    })
}

#[cfg(target_os = "windows")]
pub fn resize_popup_anchored(window: &WebviewWindow, height: u32) -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER,
    };

    let inner_size = window
        .inner_size()
        .map_err(|_| "QuotaGauge content size is unavailable.")?;
    let outer_size = window
        .outer_size()
        .map_err(|_| "QuotaGauge window size is unavailable.")?;
    let scale = window
        .scale_factor()
        .map_err(|_| "QuotaGauge display scale is unavailable.")?;
    let target_inner_height = (f64::from(height) * scale)
        .round()
        .clamp(1.0, f64::from(u32::MAX));
    let frame_overhead = outer_size.height.saturating_sub(inner_size.height);
    let anchoring = anchored_popup_frame(
        window,
        (target_inner_height as u32).saturating_add(frame_overhead),
    )?;
    let result = unsafe {
        SetWindowPos(
            window
                .hwnd()
                .map_err(|_| "QuotaGauge native window is unavailable.")?
                .0 as _,
            std::ptr::null_mut(),
            anchoring.outer_x,
            anchoring.anchored.top,
            i32::try_from(outer_size.width).unwrap_or(i32::MAX),
            i32::try_from(anchoring.anchored.height).unwrap_or(i32::MAX),
            SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOZORDER,
        )
    };
    if result == 0 {
        return Err("QuotaGauge window could not be resized.".into());
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn resize_popup_anchored(window: &WebviewWindow, height: u32) -> Result<(), String> {
    let scale = window
        .scale_factor()
        .map_err(|_| "QuotaGauge display scale is unavailable.")?;
    let target_outer_height = (f64::from(height) * scale)
        .round()
        .clamp(1.0, f64::from(u32::MAX)) as u32;
    let anchoring = anchored_popup_frame(window, target_outer_height)?;
    window
        .set_size(tauri::LogicalSize::new(PANEL_WIDTH, f64::from(height)))
        .and_then(|_| {
            window.set_position(tauri::PhysicalPosition::new(
                anchoring.outer_x,
                anchoring.anchored.top,
            ))
        })
        .map_err(|_| "QuotaGauge window could not be resized.".into())
}

fn schedule_outside_click_dismiss(window: Window) {
    let app = window.app_handle().clone();
    let token = app.state::<PopupDismissGuard>().token();

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        let app_for_dismiss = app.clone();
        let _ = app.run_on_main_thread(move || {
            let guard = app_for_dismiss.state::<PopupDismissGuard>();
            let still_unfocused = window.is_focused().is_ok_and(|focused| !focused);

            if guard.is_current(token) && still_unfocused {
                if let Some(session) = window.app_handle().try_state::<Arc<PanelResizeSession>>() {
                    session.finish(current_logical_height(&window));
                }
                let _ = window.set_resizable(false);
                hide_main_native_window(&window);
            }
        });
    });
}

pub fn handle_window_event(window: &Window, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW {
        return;
    }

    match event {
        WindowEvent::Resized(size) => {
            if let Some(session) = window.app_handle().try_state::<Arc<PanelResizeSession>>() {
                let scale = window.scale_factor().unwrap_or(1.0);
                let height = (f64::from(size.height) / scale)
                    .round()
                    .clamp(1.0, f64::from(u32::MAX)) as u32;
                session.record(height);
            }
        }
        WindowEvent::ThemeChanged(theme) => {
            let app = window.app_handle();
            let preference = app
                .try_state::<Arc<SettingsService>>()
                .map(|settings| settings.get().theme);
            if preference == Some(ThemePreference::System) {
                if let Some(webview) = app.get_webview_window(MAIN_WINDOW) {
                    let _ =
                        apply_panel_surface_for_theme(&webview, ThemePreference::System, *theme);
                }
            }
        }
        WindowEvent::Focused(false)
            if !window
                .app_handle()
                .state::<DesktopIntegration>()
                .is_floating() =>
        {
            schedule_outside_click_dismiss(window.clone())
        }
        WindowEvent::CloseRequested { api, .. } => {
            if let Some(session) = window.app_handle().try_state::<Arc<PanelResizeSession>>() {
                session.finish(current_logical_height(window));
            }
            let _ = window.set_resizable(false);
            api.prevent_close();
            let integration = window.app_handle().state::<DesktopIntegration>();
            if integration.exits_on_close() {
                window.app_handle().exit(0);
                return;
            }
            window
                .app_handle()
                .state::<PopupDismissGuard>()
                .cancel_pending();
            hide_main_native_window(window);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, thread, time::Duration};

    use tempfile::tempdir;

    use super::{
        anchored_vertical_frame, panel_resize_edge_for_context, panel_resize_edge_for_frames,
        panel_surface_color, popup_frame, tray_icon_points, PanelHeightMode, PanelResizeEdge,
        PanelResizeSession, PointRect, ScreenPoints, VerticalFrame, DARK_PANEL_SURFACE,
        LIGHT_PANEL_SURFACE, PANEL_WIDTH,
    };
    use crate::models::ThemePreference;
    use crate::storage::Storage;
    use tauri::Theme;

    #[test]
    fn recorded_heights_persist_once_the_debounce_settles() {
        let directory = tempdir().unwrap();
        let storage = Arc::new(Storage::open(&directory.path().join("quotagauge.db")).unwrap());
        let session = PanelResizeSession::new(storage.clone());

        session.begin(540).unwrap();
        session.record(612);
        assert_eq!(storage.load_panel_height().unwrap(), Some(540));
        thread::sleep(super::PANEL_RESIZE_SAVE_DELAY + Duration::from_millis(200));
        assert_eq!(storage.load_panel_height().unwrap(), Some(612));

        session.record(700);
        session.set_automatic().unwrap();
        thread::sleep(super::PANEL_RESIZE_SAVE_DELAY + Duration::from_millis(200));
        assert_eq!(storage.load_panel_height().unwrap(), None);
    }

    #[test]
    fn a_real_resize_owns_the_height_until_automatic_mode_is_restored() {
        let directory = tempdir().unwrap();
        let storage = Arc::new(Storage::open(&directory.path().join("quotagauge.db")).unwrap());
        let session = PanelResizeSession::new(storage.clone());

        assert_eq!(session.mode(), PanelHeightMode::Automatic);
        session.begin(540).unwrap();
        assert_eq!(session.mode(), PanelHeightMode::Manual);
        assert_eq!(storage.load_panel_height().unwrap(), Some(540));
        session.record(612);
        session.finish(Some(612));
        assert_eq!(session.mode(), PanelHeightMode::Manual);
        assert_eq!(storage.load_panel_height().unwrap(), Some(612));

        let restarted = PanelResizeSession::new(storage.clone());
        assert_eq!(restarted.mode(), PanelHeightMode::Manual);
        assert_eq!(restarted.saved_height(), Some(612));

        session.set_automatic().unwrap();
        assert_eq!(session.mode(), PanelHeightMode::Automatic);
        assert_eq!(storage.load_panel_height().unwrap(), None);

        session.begin(680).unwrap();
        session.record(720);
        session.set_automatic().unwrap();
        session.finish(Some(720));
        assert_eq!(session.mode(), PanelHeightMode::Automatic);
        assert_eq!(storage.load_panel_height().unwrap(), None);
    }

    #[test]
    fn failed_settings_reset_only_restores_unchanged_panel_state() {
        let directory = tempdir().unwrap();
        let storage = Arc::new(Storage::open(&directory.path().join("quotagauge.db")).unwrap());
        let session = PanelResizeSession::new(storage.clone());
        session.set_manual(560).unwrap();

        let reset = session.begin_automatic_reset().unwrap();
        assert_eq!(session.mode(), PanelHeightMode::Automatic);
        assert!(session.rollback_automatic_reset(reset).unwrap());
        assert_eq!(session.mode(), PanelHeightMode::Manual);
        assert_eq!(storage.load_panel_height().unwrap(), Some(560));

        let stale_reset = session.begin_automatic_reset().unwrap();
        session.set_manual(640).unwrap();
        assert!(!session.rollback_automatic_reset(stale_reset).unwrap());
        assert_eq!(session.mode(), PanelHeightMode::Manual);
        assert_eq!(storage.load_panel_height().unwrap(), Some(640));
    }

    #[test]
    fn panel_surface_follows_explicit_and_system_theme_preferences() {
        assert_eq!(
            panel_surface_color(ThemePreference::Dark, Theme::Light),
            DARK_PANEL_SURFACE
        );
        assert_eq!(
            panel_surface_color(ThemePreference::Light, Theme::Dark),
            LIGHT_PANEL_SURFACE
        );
        assert_eq!(
            panel_surface_color(ThemePreference::System, Theme::Dark),
            DARK_PANEL_SURFACE
        );
        assert_eq!(
            panel_surface_color(ThemePreference::System, Theme::Light),
            LIGHT_PANEL_SURFACE
        );
    }

    #[test]
    fn bottom_anchored_popup_exposes_a_top_resize_grip() {
        assert_eq!(
            panel_resize_edge_for_frames(
                VerticalFrame {
                    top: 496,
                    height: 300,
                },
                VerticalFrame {
                    top: 100,
                    height: 700,
                },
            ),
            PanelResizeEdge::Top
        );
    }

    #[test]
    fn top_anchored_popup_exposes_a_bottom_resize_grip() {
        assert_eq!(
            panel_resize_edge_for_frames(
                VerticalFrame {
                    top: 104,
                    height: 300,
                },
                VerticalFrame {
                    top: 100,
                    height: 700,
                },
            ),
            PanelResizeEdge::Bottom
        );
    }

    #[test]
    fn floating_window_always_uses_the_bottom_resize_grip() {
        let work = VerticalFrame {
            top: 0,
            height: 1_080,
        };
        for top in [0, 140, 700] {
            assert_eq!(
                panel_resize_edge_for_context(VerticalFrame { top, height: 320 }, work, true),
                PanelResizeEdge::Bottom
            );
        }
    }

    #[test]
    fn shrinking_bottom_anchored_popup_preserves_its_bottom_edge() {
        let resized = anchored_vertical_frame(
            VerticalFrame {
                top: 496,
                height: 300,
            },
            VerticalFrame {
                top: 100,
                height: 700,
            },
            200,
        );
        assert_eq!(
            resized,
            VerticalFrame {
                top: 596,
                height: 200
            }
        );
    }

    #[test]
    fn shrinking_top_anchored_popup_preserves_its_top_edge() {
        let resized = anchored_vertical_frame(
            VerticalFrame {
                top: 104,
                height: 300,
            },
            VerticalFrame {
                top: 100,
                height: 700,
            },
            200,
        );
        assert_eq!(
            resized,
            VerticalFrame {
                top: 104,
                height: 200
            }
        );
    }

    fn laptop_screen() -> ScreenPoints {
        ScreenPoints {
            bounds: PointRect {
                x: 0.0,
                y: 0.0,
                width: 1512.0,
                height: 982.0,
            },
            work: PointRect {
                x: 0.0,
                y: 37.0,
                width: 1512.0,
                height: 945.0,
            },
            scale: 2.0,
        }
    }

    fn external_scale_one() -> ScreenPoints {
        ScreenPoints {
            bounds: PointRect {
                x: 1512.0,
                y: 0.0,
                width: 2560.0,
                height: 1440.0,
            },
            work: PointRect {
                x: 1512.0,
                y: 25.0,
                width: 2560.0,
                height: 1415.0,
            },
            scale: 1.0,
        }
    }

    fn external_at(x: f64, scale: f64) -> ScreenPoints {
        ScreenPoints {
            bounds: PointRect {
                x,
                y: 0.0,
                width: 2560.0,
                height: 1440.0,
            },
            work: PointRect {
                x,
                y: 25.0,
                width: 2560.0,
                height: 1415.0,
            },
            scale,
        }
    }

    fn px_rect(x: f64, y: f64, width: f64, height: f64) -> PointRect {
        PointRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn icon_on_scale_one_external_anchors_to_that_screen() {
        let screens = [laptop_screen(), external_scale_one()];
        let anchor = tray_icon_points(px_rect(3900.0, 0.0, 30.0, 25.0), (3915.0, 12.0), &screens)
            .expect("cursor is on the external");
        assert_eq!(anchor, px_rect(3900.0, 0.0, 30.0, 25.0));

        let frame = popup_frame(anchor, &screens, PANEL_WIDTH, 600.0).expect("screen found");
        assert_eq!(frame.x, 3752.0);
        assert_eq!(frame.y, 25.0);
        assert_eq!(frame.height, 600.0);
        assert_eq!(frame.max_height, 1202.0);
    }

    #[test]
    fn icon_backing_pixels_never_leak_into_the_neighbor_screen() {
        // Icon on the laptop at point x 1400; its backing-pixel x 2800 would sit on the
        // external if the scale were guessed as 1. The cursor decides the screen instead.
        let screens = [laptop_screen(), external_scale_one()];
        let anchor = tray_icon_points(px_rect(2800.0, 0.0, 60.0, 74.0), (1415.0, 18.0), &screens)
            .expect("cursor is on the laptop");
        assert_eq!(anchor, px_rect(1400.0, 0.0, 30.0, 37.0));

        let frame = popup_frame(anchor, &screens, PANEL_WIDTH, 600.0).expect("screen found");
        assert_eq!(frame.x, 1192.0);
        assert_eq!(frame.y, 37.0);
    }

    #[test]
    fn same_scale_screens_anchor_by_cursor_not_by_window_history() {
        let screens = [external_at(0.0, 2.0), external_at(2560.0, 2.0)];
        let anchor = tray_icon_points(px_rect(9000.0, 0.0, 60.0, 50.0), (4515.0, 12.0), &screens)
            .expect("cursor is on the second screen");
        assert_eq!(anchor, px_rect(4500.0, 0.0, 30.0, 25.0));

        let frame = popup_frame(anchor, &screens, PANEL_WIDTH, 600.0).expect("screen found");
        assert_eq!(frame.x, 4355.0);
        assert_eq!(frame.y, 25.0);
    }

    #[test]
    fn display_stacked_below_the_primary_drops_inside_it() {
        let below = ScreenPoints {
            bounds: PointRect {
                x: 0.0,
                y: 1440.0,
                width: 2560.0,
                height: 1440.0,
            },
            work: PointRect {
                x: 0.0,
                y: 1465.0,
                width: 2560.0,
                height: 1415.0,
            },
            scale: 1.0,
        };
        let screens = [below];
        let anchor = tray_icon_points(
            px_rect(2400.0, 1440.0, 30.0, 25.0),
            (2415.0, 1452.0),
            &screens,
        )
        .expect("cursor is on the stacked display");
        let frame = popup_frame(anchor, &screens, PANEL_WIDTH, 600.0).expect("screen found");
        assert_eq!(frame.x, 2240.0);
        assert_eq!(frame.y, 1465.0);
    }

    #[test]
    fn negative_coordinate_arrangement_clamps_inside_its_work_area() {
        let left = ScreenPoints {
            bounds: PointRect {
                x: -1920.0,
                y: -1080.0,
                width: 1920.0,
                height: 1080.0,
            },
            work: PointRect {
                x: -1920.0,
                y: -1055.0,
                width: 1920.0,
                height: 1055.0,
            },
            scale: 1.0,
        };
        let screens = [left];
        let anchor = tray_icon_points(
            px_rect(-100.0, -1080.0, 30.0, 25.0),
            (-85.0, -1068.0),
            &screens,
        )
        .expect("cursor is on the left display");
        let frame = popup_frame(anchor, &screens, PANEL_WIDTH, 600.0).expect("screen found");
        assert_eq!(frame.x, -320.0);
        assert_eq!(frame.y, -1055.0);
    }

    #[test]
    fn wanted_height_caps_to_the_screen_fraction_and_work_room() {
        let screens = [laptop_screen()];
        let anchor = px_rect(0.0, 0.0, 30.0, 37.0);
        let frame = popup_frame(anchor, &screens, PANEL_WIDTH, 900.0).expect("screen found");
        assert_eq!(frame.height, 803.0);
        assert_eq!(frame.max_height, 803.0);
    }

    #[test]
    fn anchor_off_every_screen_yields_no_frame() {
        let screens = [laptop_screen()];
        let anchor = px_rect(5000.0, 0.0, 30.0, 25.0);
        assert!(popup_frame(anchor, &screens, PANEL_WIDTH, 600.0).is_none());
    }

    #[test]
    fn rect_off_the_cursor_screen_falls_back_to_the_cursor_position() {
        let screens = [laptop_screen(), external_scale_one()];
        let anchor = tray_icon_points(px_rect(1400.0, 0.0, 60.0, 74.0), (3915.0, 12.0), &screens)
            .expect("cursor is on the external");
        assert_eq!(anchor.x, 3915.0);
        assert_eq!(anchor.width, 0.0);

        let frame = popup_frame(anchor, &screens, PANEL_WIDTH, 600.0).expect("screen found");
        assert_eq!(frame.x, 3752.0);
        assert_eq!(frame.y, 25.0);
    }
}
