//! 键盘操作 - 极简版，只保留粘贴功能

use arboard::Clipboard;
use std::sync::Mutex;

static SAVED_CLIPBOARD: Mutex<Option<String>> = Mutex::new(None);

/// 保存当前剪贴板内容
pub fn save_clipboard() {
    log::info!("[Keyboard] Saving clipboard...");

    match Clipboard::new() {
        Ok(mut clip) => match clip.get_text() {
            Ok(text) => {
                // 按字符截取，避免在中文字符中间切开
                let preview: String = text.chars().take(50).collect();
                let preview = if text.chars().count() > 50 {
                    format!("{}...", preview)
                } else {
                    preview
                };
                log::info!(
                    "[Keyboard] Clipboard saved ({} chars): {}",
                    text.chars().count(),
                    preview
                );
                *SAVED_CLIPBOARD.lock().unwrap() = Some(text);
            }
            Err(e) => {
                log::warn!("[Keyboard] No text in clipboard: {}", e);
            }
        },
        Err(e) => {
            log::error!("[Keyboard] Failed to open clipboard: {}", e);
        }
    }
}

/// 恢复剪贴板内容
pub fn restore_clipboard() {
    log::info!("[Keyboard] Restoring clipboard...");

    if let Some(text) = SAVED_CLIPBOARD.lock().unwrap().take() {
        match Clipboard::new() {
            Ok(mut clip) => {
                if let Err(e) = clip.set_text(&text) {
                    log::error!("[Keyboard] Failed to restore clipboard: {}", e);
                } else {
                    log::info!("[Keyboard] Clipboard restored ({} chars)", text.len());
                }
            }
            Err(e) => {
                log::error!("[Keyboard] Failed to open clipboard: {}", e);
            }
        }
    } else {
        log::info!("[Keyboard] No saved clipboard to restore");
    }
}

/// 粘贴最终文本到光标位置
pub fn paste_final(text: &str) {
    if text.is_empty() {
        log::warn!("[Keyboard] Empty text, skip paste");
        return;
    }

    log::info!("[Keyboard] Pasting text ({} chars): {}", text.len(), text);

    // 设置剪贴板
    let mut clip = match Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            log::error!("[Keyboard] Failed to open clipboard: {}", e);
            return;
        }
    };

    if let Err(e) = clip.set_text(text) {
        log::error!("[Keyboard] Failed to set clipboard: {}", e);
        return;
    }

    log::info!("[Keyboard] Text set to clipboard");

    // 模拟 Cmd+V
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        log::info!("[Keyboard] Executing Cmd+V via AppleScript");

        // AppleScript 模拟 Cmd+V
        let script = r#"
            tell application "System Events"
                keystroke "v" using command down
            end tell
        "#;

        match Command::new("osascript").arg("-e").arg(script).output() {
            Ok(output) => {
                if output.status.success() {
                    log::info!("[Keyboard] Paste command executed successfully");
                } else {
                    log::error!(
                        "[Keyboard] Paste failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
            }
            Err(e) => {
                log::error!("[Keyboard] Failed to run osascript: {}", e);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        use winapi::um::winuser::{
            SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
            VK_CONTROL,
        };

        log::info!("[Keyboard] Executing Ctrl+V via Windows SendInput API");

        // 小延迟确保剪贴板已就绪
        std::thread::sleep(std::time::Duration::from_millis(50));

        const VK_V: u16 = 0x56;

        unsafe {
            // 构建输入序列：Ctrl按下 -> V按下 -> V释放 -> Ctrl释放
            let mut inputs: [INPUT; 4] = std::mem::zeroed();

            // Ctrl 按下
            inputs[0].type_ = INPUT_KEYBOARD;
            inputs[0].u.ki_mut().wVk = VK_CONTROL as u16;
            inputs[0].u.ki_mut().dwFlags = 0;

            // V 按下
            inputs[1].type_ = INPUT_KEYBOARD;
            inputs[1].u.ki_mut().wVk = VK_V;
            inputs[1].u.ki_mut().dwFlags = 0;

            // V 释放
            inputs[2].type_ = INPUT_KEYBOARD;
            inputs[2].u.ki_mut().wVk = VK_V;
            inputs[2].u.ki_mut().dwFlags = KEYEVENTF_KEYUP;

            // Ctrl 释放
            inputs[3].type_ = INPUT_KEYBOARD;
            inputs[3].u.ki_mut().wVk = VK_CONTROL as u16;
            inputs[3].u.ki_mut().dwFlags = KEYEVENTF_KEYUP;

            let sent = SendInput(
                inputs.len() as u32,
                inputs.as_mut_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );

            if sent == inputs.len() as u32 {
                log::info!("[Keyboard] Paste command executed successfully ({} inputs sent)", sent);
            } else {
                let error = std::io::Error::last_os_error();
                log::error!(
                    "[Keyboard] SendInput failed: only {} of {} inputs sent, error: {}",
                    sent,
                    inputs.len(),
                    error
                );
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        log::warn!("[Keyboard] Paste not supported on this platform");
    }
}
