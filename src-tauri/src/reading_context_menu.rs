use std::sync::atomic::{AtomicBool, Ordering};

static ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::Release);
}

#[cfg(target_os = "linux")]
pub fn install<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    app_handle: tauri::AppHandle<R>,
) -> tauri::Result<()> {
    use tauri::Emitter;
    use webkit2gtk::{HitTestResultExt, WebViewExt};

    window.with_webview(move |platform_webview| {
        let webview = platform_webview.inner();
        webview.connect_context_menu(move |_webview, _menu, event, hit_test| {
            if !ACTIVE.load(Ordering::Acquire) || !hit_test.context_is_selection() {
                return false;
            }
            let (x, y) = event.coords().unwrap_or_default();
            let _ = app_handle.emit(
                "reading://native-context-menu",
                serde_json::json!({ "x": x, "y": y }),
            );
            true
        });
    })?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn install<R: tauri::Runtime>(
    _window: &tauri::WebviewWindow<R>,
    _app_handle: tauri::AppHandle<R>,
) -> tauri::Result<()> {
    Ok(())
}
