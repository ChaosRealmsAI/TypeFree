//! macOS 权限检测模块

#[cfg(target_os = "macos")]
mod macos {
    use core_foundation::base::*;
    use core_foundation::dictionary::*;
    use core_foundation::number::*;
    use core_foundation::runloop::*;
    use core_foundation::string::*;

    const K_IO_HID_DEVICE_USAGE_PAGE_KEY: &str = "DeviceUsagePage";
    const K_IO_HID_DEVICE_USAGE_KEY: &str = "DeviceUsage";
    const K_HID_PAGE_GENERIC_DESKTOP: i32 = 0x01;
    const K_HID_USAGE_KEYBOARD: i32 = 0x06;

    #[repr(C)]
    struct __IOHIDManager {
        _private: [u8; 0],
    }
    type IOHIDManagerRef = *mut __IOHIDManager;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOHIDManagerCreate(allocator: CFAllocatorRef, options: u32) -> IOHIDManagerRef;
        fn IOHIDManagerSetDeviceMatching(manager: IOHIDManagerRef, matching: CFDictionaryRef);
        fn IOHIDManagerOpen(manager: IOHIDManagerRef, options: u32) -> i32;
        fn IOHIDManagerClose(manager: IOHIDManagerRef, options: u32) -> i32;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    /// 检测 Input Monitoring 权限
    /// 通过尝试打开 IOHIDManager 来检测
    pub fn check_input_monitoring() -> bool {
        unsafe {
            let manager = IOHIDManagerCreate(kCFAllocatorDefault, 0);
            if manager.is_null() {
                return false;
            }

            let page_key = CFString::new(K_IO_HID_DEVICE_USAGE_PAGE_KEY);
            let usage_key = CFString::new(K_IO_HID_DEVICE_USAGE_KEY);
            let page_num = CFNumber::from(K_HID_PAGE_GENERIC_DESKTOP);
            let usage_num = CFNumber::from(K_HID_USAGE_KEYBOARD);

            let matching = CFDictionary::from_CFType_pairs(&[
                (page_key.as_CFType(), page_num.as_CFType()),
                (usage_key.as_CFType(), usage_num.as_CFType()),
            ]);

            IOHIDManagerSetDeviceMatching(manager, matching.as_concrete_TypeRef());

            let result = IOHIDManagerOpen(manager, 0);
            if result == 0 {
                IOHIDManagerClose(manager, 0);
                true
            } else {
                false
            }
        }
    }

    /// 检测 Accessibility 权限
    pub fn check_accessibility() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    /// 检测麦克风权限
    /// 通过尝试获取音频输入设备来检测
    pub fn check_microphone() -> bool {
        use std::process::Command;

        // 使用 osascript 检测麦克风权限状态
        // 返回: "not determined", "denied", "authorized"
        let output = Command::new("osascript")
            .args([
                "-e",
                r#"
                use framework "AVFoundation"
                set authStatus to current application's AVCaptureDevice's authorizationStatusForMediaType:(current application's AVMediaTypeAudio)
                if authStatus = 0 then
                    return "not_determined"
                else if authStatus = 1 then
                    return "restricted"
                else if authStatus = 2 then
                    return "denied"
                else if authStatus = 3 then
                    return "authorized"
                end if
                "#,
            ])
            .output();

        match output {
            Ok(o) => {
                let status = String::from_utf8_lossy(&o.stdout).trim().to_string();
                log::info!("[Permissions] Microphone status: {}", status);
                status == "authorized"
            }
            Err(e) => {
                log::warn!("[Permissions] Failed to check microphone: {}", e);
                false
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::*;

// ============ Windows 权限检查 ============
#[cfg(target_os = "windows")]
mod windows {
    /// 检测 Input Monitoring 权限
    /// Windows 不需要显式的 Input Monitoring 权限，全局键盘钩子通常可以直接使用
    /// 但某些安全软件可能会阻止，这里返回 true 表示默认允许
    pub fn check_input_monitoring() -> bool {
        // Windows 上键盘钩子不需要特殊权限（除非被安全软件阻止）
        // 实际检测在 fn_key.rs 中设置钩子时会失败
        true
    }

    /// 检测 Accessibility 权限
    /// Windows 上使用 UI Automation 不需要特殊权限
    pub fn check_accessibility() -> bool {
        true
    }

    /// 检测麦克风权限
    /// Windows 10/11 有麦克风隐私设置，通过尝试枚举音频设备来检测
    pub fn check_microphone() -> bool {
        // 尝试通过 cpal 检测是否有可用的输入设备
        // 如果麦克风权限被拒绝，通常会返回空列表或报错
        use cpal::traits::{DeviceTrait, HostTrait};

        let host = cpal::default_host();

        // 检查是否有默认输入设备
        match host.default_input_device() {
            Some(device) => {
                // 尝试获取设备名称，如果权限被拒绝可能会失败
                match device.name() {
                    Ok(name) => {
                        log::info!("[Permissions] Windows microphone available: {}", name);
                        true
                    }
                    Err(e) => {
                        log::warn!("[Permissions] Windows microphone access error: {}", e);
                        false
                    }
                }
            }
            None => {
                log::warn!("[Permissions] No default input device found on Windows");
                false
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows::*;

// ============ 其他平台（Linux 等） ============
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn check_input_monitoring() -> bool {
    true
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn check_accessibility() -> bool {
    true
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn check_microphone() -> bool {
    true
}

/// 权限状态
#[derive(serde::Serialize, Clone)]
pub struct PermissionStatus {
    pub input_monitoring: bool,
    pub accessibility: bool,
    pub microphone: bool,
}

impl PermissionStatus {
    pub fn check() -> Self {
        Self {
            input_monitoring: check_input_monitoring(),
            accessibility: check_accessibility(),
            microphone: check_microphone(),
        }
    }
}
