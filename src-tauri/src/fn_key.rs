//! macOS Fn key monitoring using IOKit HID

#[cfg(target_os = "macos")]
mod macos {
    use core_foundation::base::*;
    use core_foundation::dictionary::*;
    use core_foundation::number::*;
    use core_foundation::runloop::*;
    use core_foundation::string::*;
    use std::ffi::c_void;
    use std::sync::mpsc::{self, Sender};
    use std::sync::OnceLock;

    const K_IO_HID_DEVICE_USAGE_PAGE_KEY: &str = "DeviceUsagePage";
    const K_IO_HID_DEVICE_USAGE_KEY: &str = "DeviceUsage";
    const K_HID_PAGE_GENERIC_DESKTOP: i32 = 0x01;
    const K_HID_USAGE_KEYBOARD: i32 = 0x06;

    #[repr(C)]
    struct __IOHIDManager {
        _private: [u8; 0],
    }
    type IOHIDManagerRef = *mut __IOHIDManager;

    #[repr(C)]
    struct __IOHIDValue {
        _private: [u8; 0],
    }
    type IOHIDValueRef = *mut __IOHIDValue;

    #[repr(C)]
    struct __IOHIDElement {
        _private: [u8; 0],
    }
    type IOHIDElementRef = *mut __IOHIDElement;

    type IOHIDValueCallback = extern "C" fn(*mut c_void, i32, *mut c_void, IOHIDValueRef);

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOHIDManagerCreate(allocator: CFAllocatorRef, options: u32) -> IOHIDManagerRef;
        fn IOHIDManagerSetDeviceMatching(manager: IOHIDManagerRef, matching: CFDictionaryRef);
        fn IOHIDManagerRegisterInputValueCallback(
            manager: IOHIDManagerRef,
            callback: IOHIDValueCallback,
            context: *mut c_void,
        );
        fn IOHIDManagerScheduleWithRunLoop(
            manager: IOHIDManagerRef,
            run_loop: CFRunLoopRef,
            run_loop_mode: CFStringRef,
        );
        fn IOHIDManagerOpen(manager: IOHIDManagerRef, options: u32) -> i32;
        fn IOHIDValueGetElement(value: IOHIDValueRef) -> IOHIDElementRef;
        fn IOHIDValueGetIntegerValue(value: IOHIDValueRef) -> i64;
        fn IOHIDElementGetUsagePage(element: IOHIDElementRef) -> u32;
        fn IOHIDElementGetUsage(element: IOHIDElementRef) -> u32;
    }

    // 使用 OnceLock + Sender 替代 static mut，避免数据竞争
    static FN_EVENT_SENDER: OnceLock<Sender<bool>> = OnceLock::new();

    extern "C" fn hid_callback(
        _ctx: *mut c_void,
        _result: i32,
        _sender: *mut c_void,
        value: IOHIDValueRef,
    ) {
        unsafe {
            let element = IOHIDValueGetElement(value);
            let usage_page = IOHIDElementGetUsagePage(element);
            let usage = IOHIDElementGetUsage(element);
            let int_value = IOHIDValueGetIntegerValue(value);

            // Fn key: Apple vendor page 0xFF or 0xFF00, usage 0x03
            if (usage_page == 0xFF || usage_page == 0xFF00) && usage == 0x03 {
                let pressed = int_value != 0;
                log::info!(
                    "[FnKey] Fn key {} (IOKit callback thread)",
                    if pressed { "PRESSED" } else { "RELEASED" }
                );

                // 通过 channel 发送事件，不直接调用回调（避免在 IOKit 线程执行 GUI 操作）
                if let Some(sender) = FN_EVENT_SENDER.get() {
                    if let Err(e) = sender.send(pressed) {
                        log::error!("[FnKey] Failed to send event: {}", e);
                    }
                }
            }
        }
    }

    pub fn start_fn_key_monitor<F>(callback: F) -> std::thread::JoinHandle<()>
    where
        F: Fn(bool) + Send + Sync + 'static,
    {
        // 创建 channel 用于 IOKit 线程和事件处理线程之间通信
        let (tx, rx) = mpsc::channel::<bool>();
        let _ = FN_EVENT_SENDER.set(tx);

        // 启动事件处理线程，接收 IOKit 发来的事件并调用回调
        let callback = std::sync::Arc::new(callback);
        let callback_clone = callback.clone();

        std::thread::spawn(move || {
            log::info!("[FnKey] Event processor thread started");
            while let Ok(pressed) = rx.recv() {
                log::info!("[FnKey] Processing event: pressed={}", pressed);
                callback_clone(pressed);
            }
            log::info!("[FnKey] Event processor thread ended");
        });

        // 启动 IOKit HID 监听线程
        std::thread::spawn(|| unsafe {
            log::info!("[FnKey] Starting HID monitor thread");

            let manager = IOHIDManagerCreate(kCFAllocatorDefault, 0);
            if manager.is_null() {
                log::error!("[FnKey] Failed to create HID manager");
                return;
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
            IOHIDManagerRegisterInputValueCallback(manager, hid_callback, std::ptr::null_mut());

            let run_loop = CFRunLoop::get_current();
            IOHIDManagerScheduleWithRunLoop(
                manager,
                run_loop.as_concrete_TypeRef(),
                kCFRunLoopDefaultMode,
            );

            let result = IOHIDManagerOpen(manager, 0);
            if result != 0 {
                log::error!(
                    "[FnKey] Failed to open HID manager (error: {}). Grant Input Monitoring permission.",
                    result
                );
                return;
            }

            log::info!("[FnKey] HID monitor started, entering run loop");
            CFRunLoop::run_current();
        })
    }
}

#[cfg(target_os = "macos")]
pub use macos::start_fn_key_monitor;

// ============ Windows: 右 Alt 长按 ============
#[cfg(target_os = "windows")]
mod windows {
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::sync::OnceLock;
    use std::time::{SystemTime, UNIX_EPOCH};
    use winapi::shared::minwindef::{LPARAM, LRESULT, WPARAM};
    use winapi::shared::windef::HHOOK;
    use winapi::um::winuser::{
        CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, KBDLLHOOKSTRUCT, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP,
        WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    const VK_RMENU: u32 = 0xA5; // 右 Alt
    const LONG_PRESS_THRESHOLD_MS: u64 = 200;

    // HHOOK 是裸指针，不实现 Sync，需要包装
    struct HookHandle(HHOOK);
    unsafe impl Send for HookHandle {}
    unsafe impl Sync for HookHandle {}

    static CALLBACK: OnceLock<Box<dyn Fn(bool) + Send + Sync>> = OnceLock::new();
    static HOOK: OnceLock<HookHandle> = OnceLock::new();
    static IS_PRESSED: AtomicBool = AtomicBool::new(false);
    static LONG_PRESS_TRIGGERED: AtomicBool = AtomicBool::new(false);
    // 使用 AtomicI64 存储按下时间戳（毫秒），避免 static mut 的不安全性
    // 0 表示未按下
    static PRESS_TIME_MS: AtomicI64 = AtomicI64::new(0);

    fn current_time_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    unsafe extern "system" fn keyboard_hook(
        code: i32,
        w_param: WPARAM,
        l_param: LPARAM,
    ) -> LRESULT {
        if code >= 0 {
            let kb = *(l_param as *const KBDLLHOOKSTRUCT);

            if kb.vkCode == VK_RMENU {
                match w_param as u32 {
                    WM_KEYDOWN | WM_SYSKEYDOWN => {
                        if !IS_PRESSED.load(Ordering::SeqCst) {
                            IS_PRESSED.store(true, Ordering::SeqCst);
                            LONG_PRESS_TRIGGERED.store(false, Ordering::SeqCst);
                            PRESS_TIME_MS.store(current_time_ms(), Ordering::SeqCst);
                            log::info!("[FnKey] Right Alt PRESSED");
                        } else {
                            // 按键重复时检查是否达到长按阈值
                            if !LONG_PRESS_TRIGGERED.load(Ordering::SeqCst) {
                                let press_time = PRESS_TIME_MS.load(Ordering::SeqCst);
                                if press_time > 0 {
                                    let elapsed = current_time_ms() - press_time;
                                    if elapsed > LONG_PRESS_THRESHOLD_MS as i64 {
                                        LONG_PRESS_TRIGGERED.store(true, Ordering::SeqCst);
                                        log::info!("[FnKey] Right Alt LONG PRESS - Start recording");
                                        if let Some(cb) = CALLBACK.get() {
                                            cb(true);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    WM_KEYUP | WM_SYSKEYUP => {
                        if IS_PRESSED.load(Ordering::SeqCst) {
                            IS_PRESSED.store(false, Ordering::SeqCst);

                            let was_long_press = LONG_PRESS_TRIGGERED.load(Ordering::SeqCst);
                            log::info!(
                                "[FnKey] Right Alt RELEASED (was_long_press={})",
                                was_long_press
                            );

                            if was_long_press {
                                // 长按结束，停止录音
                                if let Some(cb) = CALLBACK.get() {
                                    cb(false);
                                }
                            }
                            PRESS_TIME_MS.store(0, Ordering::SeqCst);
                        }
                    }
                    _ => {}
                }
            }
        }

        let hook = HOOK.get().map(|h| h.0).unwrap_or(std::ptr::null_mut());
        CallNextHookEx(hook, code, w_param, l_param)
    }

    pub fn start_fn_key_monitor<F>(callback: F) -> std::thread::JoinHandle<()>
    where
        F: Fn(bool) + Send + Sync + 'static,
    {
        let _ = CALLBACK.set(Box::new(callback));

        std::thread::spawn(|| unsafe {
            log::info!("[FnKey] Starting Windows keyboard hook...");

            let hook =
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), std::ptr::null_mut(), 0);

            if hook.is_null() {
                let error = std::io::Error::last_os_error();
                log::error!(
                    "[FnKey] Failed to set keyboard hook: {} (error code: {})",
                    error,
                    error.raw_os_error().unwrap_or(-1)
                );
                log::error!("[FnKey] This may be due to security software blocking the hook.");
                log::error!("[FnKey] Try running the application as Administrator.");
                return;
            }

            let _ = HOOK.set(HookHandle(hook));
            log::info!("[FnKey] Right Alt key monitor started (long press to activate)");

            // 标准 Windows 消息循环
            let mut msg = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // 清理钩子
            if let Some(hook_handle) = HOOK.get() {
                if !hook_handle.0.is_null() {
                    UnhookWindowsHookEx(hook_handle.0);
                    log::info!("[FnKey] Keyboard hook uninstalled");
                }
            }

            log::info!("[FnKey] Message loop ended");
        })
    }
}

#[cfg(target_os = "windows")]
pub use windows::start_fn_key_monitor;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn start_fn_key_monitor<F>(_callback: F) -> std::thread::JoinHandle<()>
where
    F: Fn(bool) + Send + Sync + 'static,
{
    std::thread::spawn(|| log::warn!("[FnKey] Key monitoring not supported on this platform"))
}
