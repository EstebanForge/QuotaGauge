use tauri::{AppHandle, Manager};

use crate::{
    desktop_integration::DesktopIntegration,
    window::{
        finish_native_panel_resize, fit_panel_to_content as fit_native_panel_to_content,
        hide_popup, lock_native_panel_resize_axis, panel_resize_edge, prepare_native_panel_resize,
        set_manual_panel_height, PanelHeightMode, PanelResizeEdge, PanelResizeSession, MAIN_WINDOW,
    },
};

#[tauri::command]
pub fn dismiss_main_window(app: AppHandle) {
    if app.state::<DesktopIntegration>().standalone_window {
        if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
            finish_native_panel_resize(&window);
        }
        app.exit(0);
    } else if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        hide_popup(&window);
    }
}

#[tauri::command]
pub fn get_panel_resize_edge(app: AppHandle) -> Result<PanelResizeEdge, String> {
    let window = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or("OpenQuota window is unavailable.")?;
    panel_resize_edge(&window)
}

#[tauri::command]
pub fn get_panel_height_mode(app: AppHandle) -> PanelHeightMode {
    app.state::<std::sync::Arc<PanelResizeSession>>().mode()
}

#[tauri::command]
pub fn fit_panel_to_content(app: AppHandle, height: u32) -> Result<bool, String> {
    let window = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or("OpenQuota window is unavailable.")?;
    fit_native_panel_to_content(&window, height.max(1))
}

#[tauri::command]
pub fn set_panel_height_automatic(app: AppHandle) -> Result<(), String> {
    app.state::<std::sync::Arc<PanelResizeSession>>()
        .set_automatic()
}

#[tauri::command]
pub fn set_panel_height_manual(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or("OpenQuota window is unavailable.")?;
    set_manual_panel_height(&window)
}

#[tauri::command]
pub fn begin_panel_resize(app: AppHandle) -> Result<PanelResizeEdge, String> {
    let window = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or("OpenQuota window is unavailable.")?;
    prepare_native_panel_resize(&window)
}

#[tauri::command]
pub fn lock_panel_resize_axis(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or("OpenQuota window is unavailable.")?;
    lock_native_panel_resize_axis(&window)
}

#[tauri::command]
pub fn finish_panel_resize(app: AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        finish_native_panel_resize(&window);
    }
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        finish_native_panel_resize(&window);
    }
    app.exit(0);
}
