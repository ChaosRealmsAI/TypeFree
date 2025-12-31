//! UI Overlay Panel
//!
//! 纯 HTML/CSS 浮层窗口，显示识别状态和结果。
//! 使用 NSPanel 实现置顶显示，不加载任何网页。

use tauri::{AppHandle, Emitter, Manager};

// macOS 窗口层级常量（高于全屏应用）
#[cfg(target_os = "macos")]
const NS_SCREEN_SAVER_WINDOW_LEVEL: i32 = 1000;

const OVERLAY_WINDOW_LABEL: &str = "overlay";

// macOS 屏幕检测模块
#[cfg(target_os = "macos")]
#[allow(deprecated)]
mod screen {
    use cocoa::appkit::NSScreen;
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSPoint, NSRect};
    use objc::{class, msg_send, sel, sel_impl};

    const OVERLAY_WIDTH: f64 = 500.0;
    const BOTTOM_MARGIN: f64 = 80.0;

    /// 获取鼠标所在屏幕（比 CGWindowListCopyWindowInfo 快很多）
    pub unsafe fn get_screen_at_focused_window() -> id {
        // 使用鼠标位置代替焦点窗口位置，避免 CGWindowListCopyWindowInfo 的性能问题
        let mouse_location: cocoa::foundation::NSPoint = msg_send![class!(NSEvent), mouseLocation];

        let screens: id = NSScreen::screens(nil);
        let count: usize = msg_send![screens, count];

        for i in 0..count {
            let screen: id = msg_send![screens, objectAtIndex: i];
            let frame: NSRect = NSScreen::frame(screen);

            if mouse_location.x >= frame.origin.x
                && mouse_location.x < frame.origin.x + frame.size.width
                && mouse_location.y >= frame.origin.y
                && mouse_location.y < frame.origin.y + frame.size.height
            {
                return screen;
            }
        }

        NSScreen::mainScreen(nil)
    }

    pub unsafe fn get_bottom_center(screen: id) -> NSPoint {
        let frame: NSRect = NSScreen::visibleFrame(screen);
        NSPoint {
            x: frame.origin.x + (frame.size.width - OVERLAY_WIDTH) / 2.0,
            y: frame.origin.y + BOTTOM_MARGIN,
        }
    }
}

/// 预加载 UI Overlay（启动时调用，创建但不显示）
pub fn preload(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        #[allow(deprecated)]
        use cocoa::appkit::NSWindowCollectionBehavior;
        use tauri_nspanel::WebviewWindowExt;

        log::info!("[Overlay] Creating UI panel...");

        // 使用本地 HTML 文件，不加载网页
        let window = tauri::WebviewWindowBuilder::new(
            app,
            OVERLAY_WINDOW_LABEL,
            tauri::WebviewUrl::App("overlay.html".into()),
        )
        .title("")
        .inner_size(500.0, 120.0)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build();

        match window {
            Ok(win) => {
                log::info!("[Overlay] Window created, converting to panel");

                match win.to_panel() {
                    Ok(panel) => {
                        panel.set_released_when_closed(false);
                        panel.set_becomes_key_only_if_needed(true);
                        panel.set_floating_panel(true);
                        panel.set_level(NS_SCREEN_SAVER_WINDOW_LEVEL);

                        const NS_WINDOW_STYLE_MASK_NON_ACTIVATING_PANEL: i32 = 1 << 7;
                        panel.set_style_mask(NS_WINDOW_STYLE_MASK_NON_ACTIVATING_PANEL);

                        #[allow(deprecated)]
                        panel.set_collection_behaviour(
                            NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
                                | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary,
                        );

                        log::info!("[Overlay] Panel ready (hidden)");
                    }
                    Err(e) => {
                        log::error!("[Overlay] Failed to convert to panel: {:?}", e);
                    }
                }
            }
            Err(e) => log::error!("[Overlay] Failed to create window: {}", e),
        }
    }

    #[cfg(target_os = "windows")]
    {
        log::info!("[Overlay] Creating UI window for Windows...");

        // Windows: 获取主显示器的工作区域来定位窗口
        let window = tauri::WebviewWindowBuilder::new(
            app,
            OVERLAY_WINDOW_LABEL,
            tauri::WebviewUrl::App("overlay.html".into()),
        )
        .title("")
        .inner_size(500.0, 120.0)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build();

        match window {
            Ok(win) => {
                // 尝试将窗口定位到屏幕底部中央
                if let Ok(monitor) = win.current_monitor() {
                    if let Some(monitor) = monitor {
                        let size = monitor.size();
                        let position = monitor.position();
                        let scale = monitor.scale_factor();

                        // 计算底部中央位置
                        let window_width = 500.0;
                        let window_height = 120.0;
                        let bottom_margin = 80.0;

                        let x = position.x as f64 + (size.width as f64 / scale - window_width) / 2.0;
                        let y = position.y as f64 + size.height as f64 / scale - window_height - bottom_margin;

                        let _ = win.set_position(tauri::Position::Physical(
                            tauri::PhysicalPosition::new(
                                (x * scale) as i32,
                                (y * scale) as i32,
                            ),
                        ));
                        log::info!("[Overlay] Window positioned at ({}, {})", x, y);
                    }
                }
                log::info!("[Overlay] Window ready (hidden)");
            }
            Err(e) => log::error!("[Overlay] Failed to create window: {}", e),
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        log::info!("[Overlay] Creating UI window...");

        let window = tauri::WebviewWindowBuilder::new(
            app,
            OVERLAY_WINDOW_LABEL,
            tauri::WebviewUrl::App("overlay.html".into()),
        )
        .title("")
        .inner_size(500.0, 120.0)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .center()
        .visible(false)
        .build();

        match window {
            Ok(_) => log::info!("[Overlay] Window ready (hidden)"),
            Err(e) => log::error!("[Overlay] Failed to create window: {}", e),
        }
    }
}

/// 显示 Overlay（必须在主线程调用）
pub fn show(app: &AppHandle) {
    log::info!("[Overlay] show called");

    // 发送重置事件
    let _ = app.emit("overlay-reset", ());

    #[cfg(target_os = "macos")]
    {
        use objc::{msg_send, sel, sel_impl};
        use tauri_nspanel::ManagerExt;

        if let Ok(panel) = app.get_webview_panel(OVERLAY_WINDOW_LABEL) {
            log::info!("[Overlay] Positioning to current screen bottom");

            unsafe {
                let target_screen = screen::get_screen_at_focused_window();
                let position = screen::get_bottom_center(target_screen);
                log::info!(
                    "[Overlay] Setting position to ({}, {})",
                    position.x,
                    position.y
                );

                // 使用 Cocoa API 设置位置（坐标系原点在左下角）
                let _: () = msg_send![&*panel, setFrameOrigin: position];
            }

            panel.order_front_regardless();
            log::info!("[Overlay] Panel shown");
            return;
        }
    }

    if let Some(window) = app.get_webview_window(OVERLAY_WINDOW_LABEL) {
        log::info!("[Overlay] Window exists, showing it");

        // Windows: 重新定位窗口到当前屏幕底部中央
        #[cfg(target_os = "windows")]
        {
            if let Ok(Some(monitor)) = window.current_monitor() {
                let size = monitor.size();
                let position = monitor.position();
                let scale = monitor.scale_factor();

                let window_width = 500.0;
                let window_height = 120.0;
                let bottom_margin = 80.0;

                let x = position.x as f64 + (size.width as f64 / scale - window_width) / 2.0;
                let y = position.y as f64 + size.height as f64 / scale - window_height - bottom_margin;

                let _ = window.set_position(tauri::Position::Physical(
                    tauri::PhysicalPosition::new(
                        (x * scale) as i32,
                        (y * scale) as i32,
                    ),
                ));
                log::info!("[Overlay] Window repositioned to ({}, {})", x, y);
            }
        }

        let _ = window.show();
        return;
    }

    log::warn!("[Overlay] No preloaded overlay, creating now...");
    preload(app);
}

/// 隐藏 Overlay
pub fn hide(app: &AppHandle) {
    log::info!("[Overlay] hide called");

    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::ManagerExt;

        if let Ok(panel) = app.get_webview_panel(OVERLAY_WINDOW_LABEL) {
            panel.order_out(None);
            log::info!("[Overlay] Panel hidden");
            return;
        }
    }

    if let Some(window) = app.get_webview_window(OVERLAY_WINDOW_LABEL) {
        let _ = window.hide();
        log::info!("[Overlay] Window hidden");
    }
}

/// 更新状态文字（如 "聆听中..."、"识别中..."）
pub fn update_status(app: &AppHandle, status: &str) {
    let _ = app.emit("overlay-status", status);
}

/// 更新识别结果文字
pub fn update_text(app: &AppHandle, text: &str) {
    let _ = app.emit("overlay-text", text);
}
