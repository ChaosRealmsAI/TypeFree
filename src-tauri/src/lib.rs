//! TypeFree - Fn 键触发录音 + ASR + 实时字幕 + 粘贴到光标
//!
//! 仅使用 CDP 方案：通过豆包桌面端的 Chrome DevTools Protocol 进行语音识别

mod audio;
mod doubao_asr;
mod doubao_cdp;
mod doubao_launcher;
mod fn_key;
mod keyboard;
mod overlay;
mod permissions;
mod resample;
mod tray;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, WebviewUrl, WebviewWindowBuilder};

// 全局 AppHandle
static APP_HANDLE: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

// ============ 状态 ============

static IS_RECORDING: AtomicBool = AtomicBool::new(false);

static STOP_FLAG: std::sync::LazyLock<Arc<AtomicBool>> =
    std::sync::LazyLock::new(|| Arc::new(AtomicBool::new(false)));

static RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> =
    std::sync::LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    });

// ============ Overlay 控制 ============

fn show_overlay(app: &AppHandle) {
    let app_for_thread = app.clone();
    // UI 操作必须在主线程执行
    let _ = app.run_on_main_thread(move || {
        overlay::update_status(&app_for_thread, "聆听中...");
        overlay::show(&app_for_thread);
    });
}

fn hide_overlay(app: &AppHandle) {
    let app_for_thread = app.clone();
    let _ = app.run_on_main_thread(move || {
        overlay::hide(&app_for_thread);
    });
}


// ============ Fn 键处理 ============

fn on_fn_pressed(app: &AppHandle) {
    log::info!("[TypeFree] === Fn PRESSED ===");

    // 检查豆包是否在运行（需要保持运行以获取实时 Cookie）
    let doubao_running = RUNTIME.block_on(async { doubao_cdp::is_doubao_debug_available().await });

    if !doubao_running {
        log::warn!("[TypeFree] Doubao not running in debug mode");
        show_overlay(app);
        let app_for_error = app.clone();
        let _ = app.run_on_main_thread(move || {
            overlay::update_text(&app_for_error, "请先启动豆包桌面端");
        });
        // 2秒后隐藏
        let app_for_hide = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            hide_overlay(&app_for_hide);
        });
        return;
    }

    if IS_RECORDING.swap(true, Ordering::SeqCst) {
        log::warn!("[TypeFree] Already recording");
        return;
    }

    STOP_FLAG.store(false, Ordering::SeqCst);
    show_overlay(app);

    let app_clone = app.clone();
    let stop_flag = STOP_FLAG.clone();

    RUNTIME.spawn(async move {
        run_stt(&app_clone, stop_flag).await;
    });
}

fn on_fn_released(app: &AppHandle) {
    log::info!("[TypeFree] === Fn RELEASED ===");

    if !IS_RECORDING.swap(false, Ordering::SeqCst) {
        return;
    }

    STOP_FLAG.store(true, Ordering::SeqCst);
    let _ = app.emit("recording-stopped", ());
}

// ============ STT 流程 ============

/// 运行 STT 流程（CDP 方案）
async fn run_stt(app: &AppHandle, stop_flag: Arc<AtomicBool>) {
    log::info!("[TypeFree] Starting STT (realtime Cookie mode)...");

    // 启动录音
    let (audio_tx, audio_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let audio_stop = stop_flag.clone();

    let audio_handle = match audio::start_recording(audio_tx, audio_stop) {
        Ok(h) => {
            log::info!("[TypeFree] Recording started");
            h
        }
        Err(e) => {
            log::error!("[TypeFree] Recording failed: {}", e);
            hide_overlay(app);
            return;
        }
    };

    // 回调函数
    let app_for_partial = app.clone();
    let app_for_final = app.clone();

    let on_partial = move |text: &str| {
        overlay::update_text(&app_for_partial, text);
    };

    let on_final = move |text: &str| {
        log::info!("[TypeFree] ========== 最终结果 ==========");
        log::info!("[TypeFree] {}", text);
        log::info!("[TypeFree] ================================");

        // 粘贴到光标
        keyboard::paste_final(text);

        // 显示最终结果，1秒后隐藏
        overlay::update_text(&app_for_final, text);
        let app_clone = app_for_final.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(1));
            hide_overlay(&app_clone);
        });
    };

    // 运行 ASR 会话
    let session_result = doubao_asr::run_asr_session(audio_rx, stop_flag, on_partial, on_final).await;

    if let Err(e) = &session_result {
        log::error!("[TypeFree] ASR session error: {}", e);
        // 显示错误信息
        overlay::update_text(app, &format!("错误: {}", e));
    }

    let _ = audio_handle.join();
    log::info!("[TypeFree] STT session ended");

    // 如果 ASR 出错，2秒后隐藏 overlay
    if session_result.is_err() {
        let app_clone = app.clone();
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        hide_overlay(&app_clone);
    }
}

// ============ Tauri Commands ============

#[tauri::command]
fn get_permission_status() -> permissions::PermissionStatus {
    let status = permissions::PermissionStatus::check();
    log::info!(
        "[TypeFree] Permission status: input_monitoring={}, accessibility={}, microphone={}",
        status.input_monitoring,
        status.accessibility,
        status.microphone
    );
    status
}

#[tauri::command]
fn open_input_monitoring_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        // Windows 没有专门的 Input Monitoring 设置
        // 打开隐私设置主页面
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "ms-settings:privacy"])
            .spawn();
    }
}

#[tauri::command]
fn open_accessibility_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        // Windows 辅助功能设置
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "ms-settings:easeofaccess"])
            .spawn();
    }
}

#[tauri::command]
fn open_microphone_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        // Windows 麦克风隐私设置
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "ms-settings:privacy-microphone"])
            .spawn();
    }
}

// ============ 豆包桌面端管理 ============

#[derive(serde::Serialize)]
struct DoubaoStatus {
    installed: bool,
    running: bool,
    debug_mode: bool,
    logged_in: bool,
    ws_available: bool,
}

#[tauri::command]
async fn get_doubao_status() -> DoubaoStatus {
    let installed = doubao_launcher::is_doubao_installed();
    let running = doubao_launcher::is_doubao_running();
    let debug_mode = doubao_cdp::is_doubao_debug_available().await;

    // 优先使用缓存的登录状态，如果没有缓存且 CDP 可用则实时检测
    let logged_in = match doubao_cdp::get_cached_login_status() {
        Some(status) => status,
        None if debug_mode => {
            doubao_cdp::check_login_status().await.unwrap_or(false)
        }
        None => false,
    };

    // 判断服务是否可用（有缓存的 Cookie 和 URL 参数即可）
    let ws_available = logged_in &&
        doubao_cdp::get_cached_cookies().is_some() &&
        doubao_cdp::get_cached_url_params().is_some();

    log::info!(
        "[TypeFree] Doubao status: installed={}, running={}, debug_mode={}, logged_in={}, ws_available={}",
        installed, running, debug_mode, logged_in, ws_available
    );

    DoubaoStatus {
        installed,
        running,
        debug_mode,
        logged_in,
        ws_available,
    }
}

#[tauri::command]
async fn test_doubao_connection() -> Result<(), String> {
    doubao_asr::test_connection().await
}

#[tauri::command]
async fn launch_doubao_debug() -> Result<(), String> {
    doubao_launcher::ensure_doubao_debug_mode().await.map(|_| ())
}

#[tauri::command]
async fn restart_doubao_debug() -> Result<(), String> {
    doubao_launcher::restart_doubao_debug_mode().await
}

// ============ 入口 ============

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![]),
        ));

    // macOS: 添加 nspanel 插件用于置顶 overlay
    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }

    builder
        .invoke_handler(tauri::generate_handler![
            get_permission_status,
            open_input_monitoring_settings,
            open_accessibility_settings,
            open_microphone_settings,
            get_doubao_status,
            test_doubao_connection,
            launch_doubao_debug,
            restart_doubao_debug,
        ])
        .setup(|app| {
            let app_handle = app.handle().clone();

            // 保存全局 AppHandle
            let _ = APP_HANDLE.set(app_handle.clone());

            // 初始化系统托盘
            log::info!("[TypeFree] Initializing tray...");
            if let Err(e) = tray::init(&app_handle) {
                log::error!("[TypeFree] Failed to init tray: {}", e);
            }

            // 预热麦克风 - 只在没有权限时触发系统权限弹窗
            if !permissions::check_microphone() {
                log::info!("[TypeFree] Microphone not authorized, warming up to trigger permission prompt...");
                audio::warmup_microphone();
            } else {
                log::info!("[TypeFree] Microphone already authorized");
            }

            // 创建主窗口
            log::info!("[TypeFree] Creating main window...");
            let main_window = WebviewWindowBuilder::new(
                &app_handle,
                "main",
                WebviewUrl::App("index.html".into()),
            )
            .title("TypeFree")
            .inner_size(440.0, 850.0)
            .resizable(false)
            .center()
            .build()
            .expect("Failed to create main window");

            // 拦截关闭事件，改为隐藏窗口而不是销毁
            let window_for_event = main_window.clone();
            main_window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window_for_event.hide();
                    log::info!("[TypeFree] Window hidden instead of closed");
                }
            });

            // 创建 Overlay Panel（使用 NSPanel 置顶显示，定位到焦点窗口所在屏幕底部）
            log::info!("[TypeFree] Creating overlay panel...");
            overlay::preload(&app_handle);

            // 自动启动豆包调试模式 + 捕获 ASR URL 参数
            log::info!("[TypeFree] Ensuring Doubao debug mode...");
            let app_for_doubao = app.handle().clone();
            RUNTIME.spawn(async move {
                match doubao_launcher::ensure_doubao_debug_mode().await {
                    Ok(_) => {
                        log::info!("[TypeFree] Doubao debug mode ready");
                        let _ = app_for_doubao.emit("doubao-ready", true);

                        // 等待豆包页面完全加载
                        log::info!("[TypeFree] Waiting for Doubao page to load...");
                        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

                        // 自动捕获 ASR URL 参数
                        log::info!("[TypeFree] Capturing ASR URL params...");
                        match doubao_cdp::capture_asr_url_by_click().await {
                            Ok(url) => {
                                log::info!("[TypeFree] Captured ASR URL: {}", url);
                                let params = doubao_cdp::parse_asr_url_params(&url);
                                log::info!("[TypeFree] Parsed {} params, caching...", params.len());
                                doubao_cdp::set_cached_url_params(params);

                                // 检测登录状态
                                log::info!("[TypeFree] Checking login status...");
                                match doubao_cdp::check_login_status().await {
                                    Ok(logged_in) => {
                                        log::info!("[TypeFree] Login status: {}", logged_in);
                                        doubao_cdp::set_cached_login_status(logged_in);
                                    }
                                    Err(e) => {
                                        log::warn!("[TypeFree] Failed to check login: {}", e);
                                    }
                                }

                                // 保持豆包在后台运行，不关闭
                                log::info!("[TypeFree] Doubao will keep running in background for real-time Cookie fetching");

                                let _ = app_for_doubao.emit("asr-params-ready", true);
                            }
                            Err(e) => {
                                log::warn!("[TypeFree] Failed to capture ASR URL: {}", e);
                                log::warn!("[TypeFree] Will use fallback params when needed");
                                let _ = app_for_doubao.emit("asr-params-ready", false);
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("[TypeFree] Doubao debug mode not available: {}", e);
                        let _ = app_for_doubao.emit("doubao-ready", false);
                    }
                }
            });

            // 启动 Fn 键监听
            log::info!("[TypeFree] Starting Fn key monitor...");
            fn_key::start_fn_key_monitor(move |pressed| {
                if pressed {
                    on_fn_pressed(&app_handle);
                } else {
                    on_fn_released(&app_handle);
                }
            });

            log::info!("[TypeFree] Ready!");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
