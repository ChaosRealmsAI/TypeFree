//! 系统托盘 (Menu Bar) 功能

use tauri::{
    image::Image,
    include_image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager,
};
use tauri_plugin_autostart::ManagerExt;

const TRAY_ICON: Image<'static> = include_image!("icons/tray-icon@2x.png");

pub fn init(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    // 检查当前自动启动状态
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart_text = if autostart_enabled {
        "✓ 开机自动启动"
    } else {
        "开机自动启动"
    };

    // 创建菜单项（只保留操作按钮）
    let open = MenuItem::with_id(app, "open", "打开 TypeFree", true, None::<&str>)?;
    let autostart_item =
        MenuItem::with_id(app, "autostart", autostart_text, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    // 分隔符
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;

    // 菜单结构
    let menu = Menu::with_items(
        app,
        &[&open, &sep1, &autostart_item, &sep2, &quit],
    )?;

    // 克隆用于闭包
    let autostart_for_closure = autostart_item.clone();

    // 构建托盘图标
    let _tray = TrayIconBuilder::with_id("main")
        .icon(TRAY_ICON)
        .icon_as_template(true)
        .menu(&menu)
        .tooltip("TypeFree")
        .on_menu_event(move |app, event| {
            let id = event.id.as_ref();
            log::info!("[Tray] Menu event: {}", id);

            match id {
                "open" => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "autostart" => {
                    let autolaunch = app.autolaunch();
                    let is_enabled = autolaunch.is_enabled().unwrap_or(false);

                    let result = if is_enabled {
                        autolaunch.disable()
                    } else {
                        autolaunch.enable()
                    };

                    match result {
                        Ok(_) => {
                            let new_enabled = !is_enabled;
                            let text = if new_enabled {
                                "✓ 开机自动启动"
                            } else {
                                "开机自动启动"
                            };
                            let _ = autostart_for_closure.set_text(text);
                            log::info!(
                                "[Tray] Autostart {}",
                                if new_enabled { "enabled" } else { "disabled" }
                            );
                        }
                        Err(e) => {
                            log::error!("[Tray] Failed to toggle autostart: {}", e);
                        }
                    }
                }
                "quit" => {
                    log::info!("[Tray] Quit");
                    app.exit(0);
                }
                _ => {}
            }
        })
        .build(app)?;

    log::info!("[Tray] Initialized");
    Ok(())
}
