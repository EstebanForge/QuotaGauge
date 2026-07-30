use tauri::{AppHandle, Manager};

use crate::{
    desktop_integration::DesktopIntegration,
    window::{
        finish_native_panel_resize, hide_popup, panel_resize_edge, prepare_native_panel_resize,
        PanelResizeEdge, MAIN_WINDOW,
    },
};

#[tauri::command]
pub fn dismiss_main_window(app: AppHandle) {
    if app.state::<DesktopIntegration>().standalone_window {
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
pub fn begin_panel_resize(app: AppHandle) -> Result<PanelResizeEdge, String> {
    let window = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or("OpenQuota window is unavailable.")?;
    prepare_native_panel_resize(&window)
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
