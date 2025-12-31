//! 豆包桌面端 CDP (Chrome DevTools Protocol) 模块
//!
//! 从豆包桌面端（以调试模式运行）获取 Cookie 和 ASR 请求参数

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

const CDP_LIST_URL: &str = "http://127.0.0.1:9222/json/list";

/// 缓存的 Cookie
static CACHED_COOKIES: RwLock<Option<String>> = RwLock::new(None);

/// 缓存的登录状态
static CACHED_LOGIN_STATUS: RwLock<Option<bool>> = RwLock::new(None);

/// 缓存的 ASR 请求信息
static CACHED_ASR_REQUEST: RwLock<Option<AsrRequestInfo>> = RwLock::new(None);

/// 缓存的 URL 参数模板（从真实请求捕获）
static CACHED_URL_PARAMS: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

/// ASR 请求信息（从豆包桌面端抓取）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrRequestInfo {
    pub url: String,
    pub user_agent: String,
    pub origin: String,
}

/// 从 Cookie 列表中提取特定值
fn extract_cookie_value(cookies: &[CdpCookie], name: &str) -> Option<String> {
    cookies.iter()
        .find(|c| c.name == name)
        .map(|c| c.value.clone())
}

/// 从 User-Agent 解析版本信息
fn parse_user_agent(ua: &str) -> (String, String) {
    // 解析 SamanthaDoubao/x.xx.x
    let pc_version = ua
        .split("SamanthaDoubao/")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("1.85.8")
        .to_string();

    // 解析 Chrome/xxx.x.xxxx.xx
    let chromium_version = ua
        .split("Chrome/")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("135.0.0.0")
        .to_string();

    (pc_version, chromium_version)
}

/// 构建完整的 ASR URL
fn build_asr_url(device_id: &str, web_id: &str, pc_version: &str, chromium_version: &str) -> String {
    let web_tab_id = uuid::Uuid::new_v4().to_string();

    format!(
        "wss://ws-samantha.doubao.com/samantha/audio/asr?\
         version_code=20800&\
         language=zh&\
         device_platform=web&\
         aid=582478&\
         real_aid=582478&\
         pkg_type=release_version&\
         device_id={device_id}&\
         pc_version={pc_version}&\
         web_id={web_id}&\
         tea_uuid={device_id}&\
         region=&\
         sys_region=&\
         samantha_web=1&\
         use-olympus-account=1&\
         runtime=web&\
         runtime_version=2.51.3&\
         client_platform=pc_client&\
         chromium_version={chromium_version}&\
         fp=verify_{web_id}&\
         web_tab_id={web_tab_id}&\
         format=pcm"
    )
}

impl Default for AsrRequestInfo {
    fn default() -> Self {
        Self {
            url: "wss://ws-samantha.doubao.com/samantha/audio/asr?version_code=20800&language=zh&device_platform=web&aid=582478&real_aid=582478&format=pcm".to_string(),
            user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36 SamanthaDoubao/1.85.8".to_string(),
            origin: "https://www.doubao.com".to_string(),
        }
    }
}

/// CDP 页面信息
#[derive(Debug, Deserialize)]
struct CdpPage {
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    websocket_debugger_url: Option<String>,
}

/// CDP 响应
#[derive(Debug, Deserialize)]
struct CdpResponse {
    result: Option<CdpResult>,
}

#[derive(Debug, Deserialize)]
struct CdpResult {
    cookies: Option<Vec<CdpCookie>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CdpCookie {
    name: String,
    value: String,
    domain: String,
}

/// 从豆包桌面端获取 Cookie
pub async fn fetch_cookies() -> Result<String, String> {
    log::info!("[DoubaoCDP] Fetching cookies from Doubao desktop...");

    // 获取页面列表
    let pages: Vec<CdpPage> = reqwest::get(CDP_LIST_URL)
        .await
        .map_err(|e| format!("Failed to connect to CDP: {}. Is Doubao running with --remote-debugging-port=9222?", e))?
        .json()
        .await
        .map_err(|e| format!("Failed to parse CDP response: {}", e))?;

    log::info!("[DoubaoCDP] Found {} pages", pages.len());

    // 找到 doubao.com/chat 页面
    let chat_page = pages
        .iter()
        .find(|p| p.url.contains("doubao.com") && p.url.contains("chat"))
        .ok_or("No doubao.com/chat page found")?;

    let ws_url = chat_page
        .websocket_debugger_url
        .as_ref()
        .ok_or("No WebSocket debugger URL")?;

    log::info!("[DoubaoCDP] Connecting to: {}", ws_url);

    // 连接 CDP WebSocket
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| format!("Failed to connect CDP WebSocket: {}", e))?;

    use futures_util::{SinkExt, StreamExt};

    // 发送 getCookies 请求
    let request = serde_json::json!({
        "id": 1,
        "method": "Network.getCookies",
        "params": {
            "urls": ["https://www.doubao.com", "https://ws-samantha.doubao.com"]
        }
    });

    ws.send(tokio_tungstenite::tungstenite::Message::Text(request.to_string()))
        .await
        .map_err(|e| format!("Failed to send CDP request: {}", e))?;

    // 接收响应
    let msg = ws
        .next()
        .await
        .ok_or("No response from CDP")?
        .map_err(|e| format!("CDP WebSocket error: {}", e))?;

    let response: CdpResponse = match msg {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str(&text).map_err(|e| format!("Failed to parse CDP response: {}", e))?
        }
        _ => return Err("Unexpected CDP response type".to_string()),
    };

    let cookies = response
        .result
        .ok_or("No result in CDP response")?
        .cookies
        .ok_or("No cookies in CDP response")?;

    log::info!("[DoubaoCDP] Got {} cookies", cookies.len());

    // 构建 Cookie 字符串
    let cookie_str: String = cookies
        .iter()
        .filter(|c| c.domain.ends_with("doubao.com"))
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ");

    if cookie_str.is_empty() {
        return Err("No valid cookies found".to_string());
    }

    // 缓存 Cookie
    if let Ok(mut cache) = CACHED_COOKIES.write() {
        *cache = Some(cookie_str.clone());
    }

    log::info!("[DoubaoCDP] Cookie string length: {}", cookie_str.len());
    Ok(cookie_str)
}

/// 获取缓存的 Cookie
pub fn get_cached_cookies() -> Option<String> {
    CACHED_COOKIES.read().ok().and_then(|c| c.clone())
}

/// 获取缓存的登录状态
pub fn get_cached_login_status() -> Option<bool> {
    CACHED_LOGIN_STATUS.read().ok().and_then(|s| *s)
}

/// 设置缓存的登录状态
pub fn set_cached_login_status(status: bool) {
    if let Ok(mut cache) = CACHED_LOGIN_STATUS.write() {
        *cache = Some(status);
    }
}

/// 清除缓存的 Cookie
pub fn clear_cached_cookies() {
    if let Ok(mut cache) = CACHED_COOKIES.write() {
        *cache = None;
    }
}

/// 获取缓存的 ASR 请求信息
pub fn get_cached_asr_request() -> Option<AsrRequestInfo> {
    CACHED_ASR_REQUEST.read().ok().and_then(|r| r.clone())
}

/// 设置 ASR 请求信息缓存
pub fn set_cached_asr_request(info: AsrRequestInfo) {
    if let Ok(mut cache) = CACHED_ASR_REQUEST.write() {
        *cache = Some(info);
    }
}

/// 获取缓存的 URL 参数模板
pub fn get_cached_url_params() -> Option<HashMap<String, String>> {
    CACHED_URL_PARAMS.read().ok().and_then(|p| p.clone())
}

/// 设置 URL 参数模板缓存
pub fn set_cached_url_params(params: HashMap<String, String>) {
    if let Ok(mut cache) = CACHED_URL_PARAMS.write() {
        *cache = Some(params);
    }
}

/// 清除 URL 参数缓存
pub fn clear_cached_url_params() {
    if let Ok(mut cache) = CACHED_URL_PARAMS.write() {
        *cache = None;
    }
}

/// 解析 ASR URL 中的参数
pub fn parse_asr_url_params(url: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();

    // 提取 query string
    if let Some(query_start) = url.find('?') {
        let query = &url[query_start + 1..];
        for pair in query.split('&') {
            if let Some(eq_pos) = pair.find('=') {
                let key = &pair[..eq_pos];
                let value = &pair[eq_pos + 1..];
                params.insert(key.to_string(), value.to_string());
            }
        }
    }

    params
}

/// 使用缓存的参数模板构建 URL
///
/// template_params: 从真实请求捕获的参数模板
/// 实时替换: device_id, web_id, web_tab_id, pc_version, chromium_version, tea_uuid, fp
fn build_asr_url_from_template(
    template_params: &HashMap<String, String>,
    device_id: &str,
    web_id: &str,
    pc_version: &str,
    chromium_version: &str,
) -> String {
    let web_tab_id = uuid::Uuid::new_v4().to_string();

    // 构建参数列表，优先使用模板中的值，实时参数覆盖
    let mut final_params: Vec<(String, String)> = Vec::new();

    // 需要实时替换的参数
    let realtime_params: HashMap<&str, &str> = [
        ("device_id", device_id),
        ("web_id", web_id),
        ("tea_uuid", device_id),
        ("pc_version", pc_version),
        ("chromium_version", chromium_version),
        ("web_tab_id", &web_tab_id),
    ].into_iter().collect();

    // fp 参数特殊处理
    let fp_value = format!("verify_{}", web_id);

    // 遍历模板参数
    for (key, value) in template_params {
        if key == "fp" {
            final_params.push((key.clone(), fp_value.clone()));
        } else if let Some(&realtime_value) = realtime_params.get(key.as_str()) {
            final_params.push((key.clone(), realtime_value.to_string()));
        } else {
            final_params.push((key.clone(), value.clone()));
        }
    }

    // 构建 URL
    let query: String = final_params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&");

    format!("wss://ws-samantha.doubao.com/samantha/audio/asr?{}", query)
}

/// 通过模拟点击捕获真实 ASR URL
///
/// 流程：
/// 1. 连接 CDP
/// 2. 启用网络监控
/// 3. 执行 JS 模拟点击语音按钮
/// 4. 监听 Network.webSocketCreated 捕获 URL
/// 5. 执行 JS 模拟点击停止按钮
/// 6. 返回捕获的 URL
pub async fn capture_asr_url_by_click() -> Result<String, String> {
    log::info!("[DoubaoCDP] Capturing ASR URL by simulating click...");

    // 获取页面列表
    let pages: Vec<CdpPage> = reqwest::get(CDP_LIST_URL)
        .await
        .map_err(|e| format!("Failed to connect to CDP: {}", e))?
        .json()
        .await
        .map_err(|e| format!("Failed to parse CDP response: {}", e))?;

    // 打印所有页面
    log::info!("[DoubaoCDP] Found {} pages:", pages.len());
    for p in &pages {
        log::info!("[DoubaoCDP]   - {}", p.url);
    }

    // 找到 doubao.com/chat 页面
    let chat_page = pages
        .iter()
        .find(|p| p.url.contains("doubao.com") && p.url.contains("chat"))
        .ok_or("No doubao.com/chat page found. Please open a chat in Doubao first.")?;

    let ws_url = chat_page
        .websocket_debugger_url
        .as_ref()
        .ok_or("No WebSocket debugger URL")?;

    log::info!("[DoubaoCDP] Using chat page: {}", chat_page.url);
    log::info!("[DoubaoCDP] Connecting to CDP: {}", ws_url);

    // 连接 CDP WebSocket
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| format!("Failed to connect CDP WebSocket: {}", e))?;

    use futures_util::{SinkExt, StreamExt};

    // 1. 启用网络监控
    let enable_network = serde_json::json!({
        "id": 1,
        "method": "Network.enable"
    });
    ws.send(tokio_tungstenite::tungstenite::Message::Text(enable_network.to_string()))
        .await
        .map_err(|e| format!("Failed to enable network: {}", e))?;

    // 等待响应
    let _ = ws.next().await;

    // 2. 点击语音按钮开始录音（toggle 按钮：点一次开始，再点一次停止）
    let voice_btn_js = r#"
        (function() {
            const btn = document.querySelector('[data-testid="asr_btn"]');
            if (btn) {
                console.log('[TypeFree] Clicking asr_btn to START, current state:', btn.getAttribute('data-state'));
                btn.click();
                return 'clicked';
            }
            console.error('[TypeFree] asr_btn not found!');
            return 'not_found';
        })()
    "#;

    let click_cmd = serde_json::json!({
        "id": 2,
        "method": "Runtime.evaluate",
        "params": {
            "expression": voice_btn_js,
            "returnByValue": true
        }
    });

    log::info!("[DoubaoCDP] Clicking voice button to START...");
    ws.send(tokio_tungstenite::tungstenite::Message::Text(click_cmd.to_string()))
        .await
        .map_err(|e| format!("Failed to send click command: {}", e))?;

    // 等待点击响应
    if let Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) =
        tokio::time::timeout(tokio::time::Duration::from_secs(2), ws.next()).await
    {
        log::info!("[DoubaoCDP] Click response: {}", text);
    }

    // 3. 监听 Network.webSocketCreated 捕获 ASR URL
    let _timeout = tokio::time::Duration::from_secs(10);
    let _start = std::time::Instant::now();
    let mut captured_url: Option<String> = None;

    log::info!("[DoubaoCDP] Waiting for ASR WebSocket (2s)...");

    // 固定等待 2 秒，同时监听 WebSocket 创建事件
    let wait_duration = tokio::time::Duration::from_secs(2);
    let wait_start = std::time::Instant::now();

    while wait_start.elapsed() < wait_duration {
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(50),
            ws.next()
        ).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) => {
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                    let method = data.get("method").and_then(|m| m.as_str()).unwrap_or("");
                    if method == "Network.webSocketCreated" {
                        if let Some(params) = data.get("params") {
                            let url = params.get("url").and_then(|u| u.as_str()).unwrap_or("");
                            if url.contains("samantha") && url.contains("asr") {
                                log::info!("[DoubaoCDP] Captured ASR URL");
                                captured_url = Some(url.to_string());
                                // 继续等待完整的 2 秒
                            }
                        }
                    }
                }
            }
            _ => continue,
        }
    }

    // 固定 2 秒后点击停止
    log::info!("[DoubaoCDP] Clicking to STOP...");

    let click_stop_js = r#"
        (function() {
            const btn = document.querySelector('[data-testid="asr_btn"]');
            if (btn) {
                const rect = btn.getBoundingClientRect();
                const x = rect.left + rect.width / 2;
                const y = rect.top + rect.height / 2;
                const opts = { bubbles: true, cancelable: true, view: window, clientX: x, clientY: y, button: 0 };
                btn.dispatchEvent(new MouseEvent('mousedown', opts));
                btn.dispatchEvent(new MouseEvent('mouseup', opts));
                btn.dispatchEvent(new MouseEvent('click', opts));
                return 'stopped';
            }
            return 'not_found';
        })()
    "#;

    let stop_cmd = serde_json::json!({
        "id": 99,
        "method": "Runtime.evaluate",
        "params": { "expression": click_stop_js, "returnByValue": true }
    });

    let _ = ws.send(tokio_tungstenite::tungstenite::Message::Text(stop_cmd.to_string())).await;

    // 等待停止命令执行
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    log::info!("[DoubaoCDP] Stop command sent");

    match captured_url {
        Some(url) => {
            log::info!("[DoubaoCDP] Successfully captured ASR URL");
            Ok(url)
        }
        None => {
            Err("Failed to capture ASR URL. Voice button may not be found or click failed.".to_string())
        }
    }
}

/// 检查豆包桌面端是否以调试模式运行
pub async fn is_doubao_debug_available() -> bool {
    match reqwest::get(CDP_LIST_URL).await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// 检查用户是否已登录豆包
///
/// 通过 CDP 注入 JS 检测页面 DOM 是否有"登录"按钮
pub async fn check_login_status() -> Result<bool, String> {
    log::info!("[DoubaoCDP] Checking login status via DOM...");

    // 获取页面列表
    let pages: Vec<CdpPage> = reqwest::get(CDP_LIST_URL)
        .await
        .map_err(|e| format!("Failed to connect to CDP: {}", e))?
        .json()
        .await
        .map_err(|e| format!("Failed to parse CDP response: {}", e))?;

    // 找到 doubao.com 页面
    let doubao_page = pages
        .iter()
        .find(|p| p.url.contains("doubao.com"))
        .ok_or("No doubao.com page found")?;

    let ws_url = doubao_page
        .websocket_debugger_url
        .as_ref()
        .ok_or("No WebSocket debugger URL")?;

    // 连接 CDP WebSocket
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| format!("Failed to connect CDP WebSocket: {}", e))?;

    use futures_util::{SinkExt, StreamExt};

    // 注入 JS 检测是否有"登录"按钮（和以前 webview 方式一样）
    let check_login_js = r#"
        (function() {
            // 查找所有按钮，检查是否有"登录"按钮
            const btns = [...document.querySelectorAll('button')];
            const loginBtn = btns.find(b => b.textContent.trim() === '登录');
            // 如果找到登录按钮，说明未登录；否则已登录
            return !loginBtn;
        })()
    "#;

    let request = serde_json::json!({
        "id": 1,
        "method": "Runtime.evaluate",
        "params": {
            "expression": check_login_js,
            "returnByValue": true
        }
    });

    ws.send(tokio_tungstenite::tungstenite::Message::Text(request.to_string()))
        .await
        .map_err(|e| format!("Failed to send CDP request: {}", e))?;

    // 接收响应
    let msg = ws
        .next()
        .await
        .ok_or("No response from CDP")?
        .map_err(|e| format!("CDP WebSocket error: {}", e))?;

    let is_logged_in = match msg {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            let data: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| format!("Failed to parse CDP response: {}", e))?;

            // 提取返回值
            data.get("result")
                .and_then(|r| r.get("result"))
                .and_then(|r| r.get("value"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        }
        _ => return Err("Unexpected CDP response type".to_string()),
    };

    log::info!("[DoubaoCDP] Login status (DOM check): {}", is_logged_in);

    Ok(is_logged_in)
}

/// 自动获取完整的 ASR 请求信息
///
/// 通过 CDP 自动获取：
/// 1. Cookie（用于认证）
/// 2. User-Agent（用于解析版本号）
/// 3. device_id, web_id（从 Cookie 中提取）
/// 4. 构建完整的 ASR URL
pub async fn fetch_asr_info_auto() -> Result<(String, AsrRequestInfo), String> {
    log::info!("[DoubaoCDP] Auto fetching ASR info...");

    // 获取页面列表
    let pages: Vec<CdpPage> = reqwest::get(CDP_LIST_URL)
        .await
        .map_err(|e| format!("Failed to connect to CDP: {}. Is Doubao running with --remote-debugging-port=9222?", e))?
        .json()
        .await
        .map_err(|e| format!("Failed to parse CDP response: {}", e))?;

    log::info!("[DoubaoCDP] Found {} pages", pages.len());

    // 找到 doubao.com/chat 页面
    let chat_page = pages
        .iter()
        .find(|p| p.url.contains("doubao.com") && p.url.contains("chat"))
        .ok_or("No doubao.com/chat page found")?;

    let ws_url = chat_page
        .websocket_debugger_url
        .as_ref()
        .ok_or("No WebSocket debugger URL")?;

    log::info!("[DoubaoCDP] Connecting to: {}", ws_url);

    // 连接 CDP WebSocket
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| format!("Failed to connect CDP WebSocket: {}", e))?;

    use futures_util::{SinkExt, StreamExt};

    // 1. 获取 Cookie
    let get_cookies = serde_json::json!({
        "id": 1,
        "method": "Network.getCookies",
        "params": {
            "urls": ["https://www.doubao.com", "https://ws-samantha.doubao.com"]
        }
    });

    ws.send(tokio_tungstenite::tungstenite::Message::Text(get_cookies.to_string()))
        .await
        .map_err(|e| format!("Failed to send getCookies: {}", e))?;

    let msg = ws
        .next()
        .await
        .ok_or("No response from CDP")?
        .map_err(|e| format!("CDP WebSocket error: {}", e))?;

    let cookies: Vec<CdpCookie> = match msg {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            let response: CdpResponse = serde_json::from_str(&text)
                .map_err(|e| format!("Failed to parse CDP response: {}", e))?;
            response
                .result
                .ok_or("No result in CDP response")?
                .cookies
                .ok_or("No cookies in CDP response")?
        }
        _ => return Err("Unexpected CDP response type".to_string()),
    };

    log::info!("[DoubaoCDP] Got {} cookies", cookies.len());

    // 构建 Cookie 字符串
    let cookie_str: String = cookies
        .iter()
        .filter(|c| c.domain.ends_with("doubao.com"))
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ");

    if cookie_str.is_empty() {
        return Err("No valid cookies found".to_string());
    }

    // 缓存 Cookie
    if let Ok(mut cache) = CACHED_COOKIES.write() {
        *cache = Some(cookie_str.clone());
    }

    // 提取 device_id 和 web_id
    let device_id = extract_cookie_value(&cookies, "device_id")
        .or_else(|| extract_cookie_value(&cookies, "tt_webid"))
        .or_else(|| extract_cookie_value(&cookies, "s_v_web_id").map(|s| {
            // s_v_web_id 格式可能是 verify_xxx，提取数字部分
            s.replace("verify_", "")
        }))
        .unwrap_or_else(|| "1707977353229076".to_string());

    let web_id = extract_cookie_value(&cookies, "s_v_web_id")
        .map(|s| s.replace("verify_", ""))
        .or_else(|| extract_cookie_value(&cookies, "tt_webid"))
        .unwrap_or_else(|| "7589709632207275535".to_string());

    log::info!("[DoubaoCDP] Extracted device_id: {}, web_id: {}", device_id, web_id);

    // 2. 获取 User-Agent
    let get_ua = serde_json::json!({
        "id": 2,
        "method": "Runtime.evaluate",
        "params": {
            "expression": "navigator.userAgent"
        }
    });

    ws.send(tokio_tungstenite::tungstenite::Message::Text(get_ua.to_string()))
        .await
        .map_err(|e| format!("Failed to send evaluate: {}", e))?;

    let msg = ws
        .next()
        .await
        .ok_or("No response from CDP")?
        .map_err(|e| format!("CDP WebSocket error: {}", e))?;

    let user_agent: String = match msg {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            let data: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| format!("Failed to parse CDP response: {}", e))?;
            data.get("result")
                .and_then(|r| r.get("result"))
                .and_then(|r| r.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or(&AsrRequestInfo::default().user_agent)
                .to_string()
        }
        _ => AsrRequestInfo::default().user_agent,
    };

    log::info!("[DoubaoCDP] Got User-Agent: {}", user_agent);

    // 解析版本号
    let (pc_version, chromium_version) = parse_user_agent(&user_agent);
    log::info!("[DoubaoCDP] Parsed pc_version: {}, chromium_version: {}", pc_version, chromium_version);

    // 3. 获取 URL 参数模板（优先使用缓存，否则通过模拟点击捕获）
    let url = match get_cached_url_params() {
        Some(template_params) => {
            log::info!("[DoubaoCDP] Using cached URL params template");
            build_asr_url_from_template(&template_params, &device_id, &web_id, &pc_version, &chromium_version)
        }
        None => {
            log::info!("[DoubaoCDP] No cached URL params, trying to capture by click...");

            // 尝试通过模拟点击捕获真实 URL
            match capture_asr_url_by_click().await {
                Ok(captured_url) => {
                    log::info!("[DoubaoCDP] Captured real ASR URL, parsing params...");
                    let params = parse_asr_url_params(&captured_url);
                    log::info!("[DoubaoCDP] Parsed {} params from captured URL", params.len());

                    // 缓存参数模板
                    set_cached_url_params(params.clone());

                    // 使用捕获的参数模板构建 URL
                    build_asr_url_from_template(&params, &device_id, &web_id, &pc_version, &chromium_version)
                }
                Err(e) => {
                    log::warn!("[DoubaoCDP] Failed to capture URL by click: {}, using fallback", e);
                    // Fallback: 使用硬编码参数
                    build_asr_url(&device_id, &web_id, &pc_version, &chromium_version)
                }
            }
        }
    };

    log::info!("[DoubaoCDP] Final ASR URL: {}", url);

    let asr_info = AsrRequestInfo {
        url,
        user_agent,
        origin: "https://www.doubao.com".to_string(),
    };

    // 缓存 ASR 信息
    set_cached_asr_request(asr_info.clone());

    Ok((cookie_str, asr_info))
}
