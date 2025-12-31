//! 豆包桌面端启动器
//!
//! 管理豆包桌面端的启动（调试模式）
//! 目前仅支持 macOS，Windows 支持待实现

// ============ macOS 实现 ============
#[cfg(target_os = "macos")]
mod macos {
    use std::process::Command;

    const DOUBAO_APP_PATH: &str = "/Applications/Doubao.app/Contents/MacOS/Doubao";
    const CDP_PORT: u16 = 9222;

    /// 检查豆包是否正在运行
    /// 使用 -f 匹配命令行中包含 Doubao 的进程
    pub fn is_doubao_running() -> bool {
        // 方法1: 通过 pgrep -f 匹配命令行
        let output = Command::new("pgrep")
            .args(["-f", "Doubao.app/Contents/MacOS"])
            .output();

        if let Ok(o) = output {
            if o.status.success() {
                return true;
            }
        }

        // 方法2: 通过 ps 查找
        let output = Command::new("ps")
            .args(["aux"])
            .output();

        if let Ok(o) = output {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if stdout.contains("Doubao.app") || stdout.contains("/MacOS/Doubao") {
                return true;
            }
        }

        false
    }

    /// 关闭豆包（多种方法确保杀死）
    pub fn kill_doubao() -> Result<(), String> {
        log::info!("[DoubaoLauncher] Killing Doubao...");

        // 方法1: 使用 pkill -f 匹配命令行
        let _ = Command::new("pkill")
            .args(["-f", "Doubao.app"])
            .output();

        // 方法2: 使用 osascript 优雅关闭（macOS）
        let _ = Command::new("osascript")
            .args(["-e", "tell application \"Doubao\" to quit"])
            .output();

        // 方法3: 强制杀死 (SIGKILL)
        let _ = Command::new("pkill")
            .args(["-9", "-f", "Doubao.app"])
            .output();

        // 等待进程完全退出
        std::thread::sleep(std::time::Duration::from_millis(800));

        // 验证是否已关闭
        if is_doubao_running() {
            log::warn!("[DoubaoLauncher] Doubao still running after kill attempts");
            // 再试一次强杀
            let _ = Command::new("killall")
                .args(["-9", "Doubao"])
                .output();
            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        if is_doubao_running() {
            log::error!("[DoubaoLauncher] Failed to kill Doubao");
            return Err("无法关闭豆包，请手动关闭后重试".to_string());
        }

        log::info!("[DoubaoLauncher] Doubao killed successfully");
        Ok(())
    }

    /// 以调试模式启动豆包（后台隐藏启动）
    pub fn launch_doubao_debug() -> Result<(), String> {
        log::info!("[DoubaoLauncher] Launching Doubao in debug mode (background)...");

        // 检查豆包是否存在
        if !std::path::Path::new(DOUBAO_APP_PATH).exists() {
            return Err("Doubao.app not found in /Applications".to_string());
        }

        // 使用 open -g -j 后台隐藏启动
        // -g: 不激活应用（不获得焦点）
        // -j: 隐藏启动（窗口不显示）
        // --args: 传递参数给应用
        Command::new("open")
            .args([
                "-g", "-j",
                "-a", "/Applications/Doubao.app",
                "--args",
                &format!("--remote-debugging-port={}", CDP_PORT),
            ])
            .spawn()
            .map_err(|e| format!("Failed to launch Doubao: {}", e))?;

        log::info!("[DoubaoLauncher] Doubao launched in background with --remote-debugging-port={}", CDP_PORT);
        Ok(())
    }

    /// 确保豆包以调试模式运行
    ///
    /// 返回 Ok(true) 表示是我们启动/重启的（可以关闭）
    /// 返回 Ok(false) 表示用户已经在以调试模式运行（不应关闭）
    pub async fn ensure_doubao_debug_mode() -> Result<bool, String> {
        // 先检查 CDP 是否已经可用
        if crate::doubao_cdp::is_doubao_debug_available().await {
            log::info!("[DoubaoLauncher] Doubao debug mode already available");
            return Ok(false); // 已经是调试模式，不需要重启
        }

        // CDP 不可用，检查豆包是否在运行
        if is_doubao_running() {
            // 豆包在运行但不是调试模式，自动重启
            log::info!("[DoubaoLauncher] Doubao running in normal mode, restarting with debug mode...");
            kill_doubao()?;
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        // 启动调试模式
        launch_doubao_debug()?;

        // 等待 CDP 可用
        for i in 0..30 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            if crate::doubao_cdp::is_doubao_debug_available().await {
                log::info!("[DoubaoLauncher] CDP available after {}ms", (i + 1) * 500);
                return Ok(true); // 我们启动的，可以关闭
            }
        }

        Err("豆包启动超时，请手动检查".to_string())
    }

    /// 强制以调试模式重启豆包
    pub async fn restart_doubao_debug_mode() -> Result<(), String> {
        // 先关闭
        kill_doubao()?;

        // 等待一下
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // 启动
        launch_doubao_debug()?;

        // 等待 CDP 可用
        for i in 0..30 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            if crate::doubao_cdp::is_doubao_debug_available().await {
                log::info!("[DoubaoLauncher] CDP available after restart, took {}ms", (i + 1) * 500);
                return Ok(());
            }
        }

        Err("豆包重启后 CDP 不可用".to_string())
    }

    /// 检查豆包桌面端是否已安装
    pub fn is_doubao_installed() -> bool {
        std::path::Path::new(DOUBAO_APP_PATH).exists()
    }
}

// ============ Windows 实现（待完善） ============
#[cfg(target_os = "windows")]
mod windows {
    use std::process::Command;

    // Windows 上豆包的可能安装路径
    fn get_doubao_paths() -> Vec<String> {
        let mut paths = Vec::new();

        // 常见安装路径
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            paths.push(format!("{}\\Doubao\\Doubao.exe", local_app_data));
            paths.push(format!("{}\\Programs\\Doubao\\Doubao.exe", local_app_data));
        }
        if let Ok(program_files) = std::env::var("PROGRAMFILES") {
            paths.push(format!("{}\\Doubao\\Doubao.exe", program_files));
        }
        if let Ok(program_files_x86) = std::env::var("PROGRAMFILES(X86)") {
            paths.push(format!("{}\\Doubao\\Doubao.exe", program_files_x86));
        }

        paths
    }

    fn find_doubao_path() -> Option<String> {
        for path in get_doubao_paths() {
            if std::path::Path::new(&path).exists() {
                return Some(path);
            }
        }
        None
    }

    /// 检查豆包是否正在运行
    pub fn is_doubao_running() -> bool {
        let output = Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq Doubao.exe", "/FO", "CSV", "/NH"])
            .output();

        if let Ok(o) = output {
            let stdout = String::from_utf8_lossy(&o.stdout);
            return stdout.contains("Doubao.exe");
        }

        false
    }

    /// 关闭豆包
    pub fn kill_doubao() -> Result<(), String> {
        log::info!("[DoubaoLauncher] Killing Doubao...");

        let _ = Command::new("taskkill")
            .args(["/IM", "Doubao.exe", "/F"])
            .output();

        std::thread::sleep(std::time::Duration::from_millis(800));

        if is_doubao_running() {
            log::error!("[DoubaoLauncher] Failed to kill Doubao");
            return Err("无法关闭豆包，请手动关闭后重试".to_string());
        }

        log::info!("[DoubaoLauncher] Doubao killed successfully");
        Ok(())
    }

    /// 以调试模式启动豆包
    pub fn launch_doubao_debug() -> Result<(), String> {
        log::info!("[DoubaoLauncher] Launching Doubao in debug mode...");

        let doubao_path = find_doubao_path()
            .ok_or_else(|| "Doubao not found. Please install Doubao first.".to_string())?;

        Command::new(&doubao_path)
            .arg("--remote-debugging-port=9222")
            .spawn()
            .map_err(|e| format!("Failed to launch Doubao: {}", e))?;

        log::info!("[DoubaoLauncher] Doubao launched with --remote-debugging-port=9222");
        Ok(())
    }

    /// 确保豆包以调试模式运行
    pub async fn ensure_doubao_debug_mode() -> Result<bool, String> {
        if crate::doubao_cdp::is_doubao_debug_available().await {
            log::info!("[DoubaoLauncher] Doubao debug mode already available");
            return Ok(false);
        }

        if is_doubao_running() {
            log::info!("[DoubaoLauncher] Doubao running in normal mode, restarting with debug mode...");
            kill_doubao()?;
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        launch_doubao_debug()?;

        for i in 0..30 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            if crate::doubao_cdp::is_doubao_debug_available().await {
                log::info!("[DoubaoLauncher] CDP available after {}ms", (i + 1) * 500);
                return Ok(true);
            }
        }

        Err("豆包启动超时，请手动检查".to_string())
    }

    /// 强制以调试模式重启豆包
    pub async fn restart_doubao_debug_mode() -> Result<(), String> {
        kill_doubao()?;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        launch_doubao_debug()?;

        for i in 0..30 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            if crate::doubao_cdp::is_doubao_debug_available().await {
                log::info!("[DoubaoLauncher] CDP available after restart, took {}ms", (i + 1) * 500);
                return Ok(());
            }
        }

        Err("豆包重启后 CDP 不可用".to_string())
    }

    /// 检查豆包桌面端是否已安装
    pub fn is_doubao_installed() -> bool {
        find_doubao_path().is_some()
    }
}

// ============ 其他平台（不支持） ============
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported {
    pub fn is_doubao_running() -> bool {
        log::warn!("[DoubaoLauncher] Platform not supported");
        false
    }

    pub fn kill_doubao() -> Result<(), String> {
        Err("Platform not supported".to_string())
    }

    pub fn launch_doubao_debug() -> Result<(), String> {
        Err("Platform not supported".to_string())
    }

    pub async fn ensure_doubao_debug_mode() -> Result<bool, String> {
        Err("Platform not supported".to_string())
    }

    pub async fn restart_doubao_debug_mode() -> Result<(), String> {
        Err("Platform not supported".to_string())
    }

    pub fn is_doubao_installed() -> bool {
        false
    }
}

// ============ 导出 ============
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use unsupported::*;
