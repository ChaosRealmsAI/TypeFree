//! 豆包 ASR (语音识别) 客户端
//!
//! 使用 Rust WebSocket 直接连接豆包 ASR 服务

use crate::doubao_cdp;
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::Message;

/// ASR 结果回调
pub type ResultCallback = Box<dyn Fn(&str, bool) + Send + Sync>;

/// 获取 ASR 请求信息（优先使用缓存，否则用默认值）
fn get_asr_request_info() -> doubao_cdp::AsrRequestInfo {
    doubao_cdp::get_cached_asr_request().unwrap_or_default()
}

/// 运行 ASR 会话
///
/// - `audio_rx`: 音频数据接收端 (PCM 16-bit, 16kHz, mono)
/// - `stop_flag`: 停止标志
/// - `on_result`: 结果回调 (text, is_final)
pub async fn run_asr_session(
    audio_rx: Receiver<Vec<u8>>,
    stop_flag: Arc<AtomicBool>,
    on_partial: impl Fn(&str) + Send + 'static,
    on_final: impl Fn(&str) + Send + 'static,
) -> Result<(), String> {
    // 获取 Cookie 和 ASR 信息（自动获取所有参数）
    let (cookie, asr_info) = match (doubao_cdp::get_cached_cookies(), doubao_cdp::get_cached_asr_request()) {
        (Some(c), Some(info)) => {
            log::info!("[DoubaoASR] Using cached cookie and ASR info");
            (c, info)
        }
        _ => {
            log::info!("[DoubaoASR] No cache, auto fetching from Doubao desktop...");
            doubao_cdp::fetch_asr_info_auto().await?
        }
    };

    log::info!("[DoubaoASR] Connecting to: {}", asr_info.url);

    // 构建请求
    let request = http::Request::builder()
        .uri(&asr_info.url)
        .header("Origin", &asr_info.origin)
        .header("Cookie", &cookie)
        .header("User-Agent", &asr_info.user_agent)
        .header("Host", "ws-samantha.doubao.com")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", tokio_tungstenite::tungstenite::handshake::client::generate_key())
        .body(())
        .map_err(|e| format!("Failed to build request: {}", e))?;

    // 连接 WebSocket
    let (ws_stream, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| format!("Failed to connect ASR WebSocket: {}", e))?;

    log::info!("[DoubaoASR] WebSocket connected!");

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // 用于在任务间传递音频数据
    let (audio_tx, mut audio_rx_async) = tokio_mpsc::channel::<Vec<u8>>(100);

    // 启动音频转发任务 (sync -> async)
    let stop_flag_audio = stop_flag.clone();
    let forward_task = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Handle::current();
        loop {
            match audio_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(data) => {
                    let tx = audio_tx.clone();
                    rt.block_on(async move {
                        let _ = tx.send(data).await;
                    });
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if stop_flag_audio.load(Ordering::SeqCst) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        log::info!("[DoubaoASR] Audio forward task ended");
    });

    // 发送任务
    let stop_flag_send = stop_flag.clone();
    let send_task = tokio::spawn(async move {
        let mut chunk_count = 0;

        loop {
            tokio::select! {
                Some(data) = audio_rx_async.recv() => {
                    if let Err(e) = ws_tx.send(Message::Binary(data)).await {
                        log::error!("[DoubaoASR] Send error: {}", e);
                        break;
                    }
                    chunk_count += 1;
                    if chunk_count % 10 == 0 {
                        log::debug!("[DoubaoASR] Sent {} chunks", chunk_count);
                    }
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(50)) => {
                    if stop_flag_send.load(Ordering::SeqCst) {
                        // 发送 finish 信号
                        log::info!("[DoubaoASR] Sending finish signal...");
                        let finish_msg = serde_json::json!({"event": "finish"});
                        let _ = ws_tx.send(Message::Text(finish_msg.to_string())).await;
                        break;
                    }
                }
            }
        }

        log::info!("[DoubaoASR] Send task ended, total chunks: {}", chunk_count);
    });

    // 接收任务
    let stop_flag_recv = stop_flag.clone();
    let recv_task = tokio::spawn(async move {
        let mut final_text = String::new();
        let mut finish_timeout: Option<tokio::time::Instant> = None;

        loop {
            // 检查是否已停止录音，启动1秒超时
            if stop_flag_recv.load(Ordering::SeqCst) && finish_timeout.is_none() {
                finish_timeout = Some(tokio::time::Instant::now() + tokio::time::Duration::from_secs(1));
                log::info!("[DoubaoASR] Stop detected, waiting 1s for final result...");
            }

            // 检查超时
            if let Some(deadline) = finish_timeout {
                if tokio::time::Instant::now() >= deadline {
                    log::info!("[DoubaoASR] Timeout, using partial as final: {}", final_text);
                    if !final_text.is_empty() {
                        on_final(&final_text);
                    }
                    break;
                }
            }

            // 使用 timeout 接收消息，避免阻塞
            let recv_result = tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                ws_rx.next()
            ).await;

            match recv_result {
                Ok(Some(msg_result)) => {
                    match msg_result {
                        Ok(Message::Text(text)) => {
                            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                                let event = data.get("event").and_then(|e| e.as_str()).unwrap_or("");

                                match event {
                                    "result" => {
                                        if let Some(result_text) = data
                                            .get("result")
                                            .and_then(|r| r.get("Text"))
                                            .and_then(|t| t.as_str())
                                        {
                                            if !result_text.is_empty() {
                                                final_text = result_text.to_string();
                                                log::info!("[DoubaoASR] Partial: {}", result_text);
                                                on_partial(result_text);
                                            }
                                        }
                                    }
                                    "finish" => {
                                        log::info!("[DoubaoASR] Finish received, final: {}", final_text);
                                        if !final_text.is_empty() {
                                            on_final(&final_text);
                                        }
                                        return; // 直接返回
                                    }
                                    "" => {
                                        // 检查是否是 block 错误
                                        if let Some(code) = data.get("code").and_then(|c| c.as_i64()) {
                                            if code != 0 {
                                                let msg = data.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
                                                log::error!("[DoubaoASR] Error: code={}, message={}", code, msg);
                                                // 清除缓存的 Cookie，下次会重新获取
                                                doubao_cdp::clear_cached_cookies();
                                                break;
                                            }
                                        }
                                    }
                                    _ => {
                                        log::debug!("[DoubaoASR] Unknown event: {}", event);
                                    }
                                }
                            }
                        }
                        Ok(Message::Close(_)) => {
                            log::info!("[DoubaoASR] WebSocket closed");
                            if !final_text.is_empty() {
                                on_final(&final_text);
                            }
                            break;
                        }
                        Err(e) => {
                            log::error!("[DoubaoASR] Receive error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
                Ok(None) => {
                    // WebSocket 流结束
                    log::info!("[DoubaoASR] WebSocket stream ended");
                    if !final_text.is_empty() {
                        on_final(&final_text);
                    }
                    break;
                }
                Err(_) => {
                    // 超时，继续循环检查
                }
            }
        }

        log::info!("[DoubaoASR] Receive task ended");
    });

    // 等待任务完成
    let _ = tokio::join!(forward_task, send_task, recv_task);

    log::info!("[DoubaoASR] Session ended");
    Ok(())
}

/// 检查 ASR 是否可用
/// 优先检查缓存，有缓存就可用；否则检查豆包是否运行
pub async fn is_available() -> bool {
    // 有缓存的 Cookie 和 URL 参数就可以使用
    if doubao_cdp::get_cached_cookies().is_some() && doubao_cdp::get_cached_url_params().is_some() {
        return true;
    }
    // 没有缓存，检查豆包是否运行
    doubao_cdp::is_doubao_debug_available().await
}

/// 测试 WebSocket 连接是否可用
///
/// 真正尝试连接 WebSocket，验证 Cookie 有效性
pub async fn test_connection() -> Result<(), String> {
    log::info!("[DoubaoASR] Testing WebSocket connection...");

    // 获取 Cookie 和 ASR 信息（自动获取所有参数）
    let (cookie, asr_info) = match (doubao_cdp::get_cached_cookies(), doubao_cdp::get_cached_asr_request()) {
        (Some(c), Some(info)) => {
            log::info!("[DoubaoASR] Using cached cookie and ASR info");
            (c, info)
        }
        _ => {
            log::info!("[DoubaoASR] No cache, auto fetching...");
            doubao_cdp::fetch_asr_info_auto().await?
        }
    };

    log::info!("[DoubaoASR] Test connecting to: {}", asr_info.url);

    // 构建请求
    let request = http::Request::builder()
        .uri(&asr_info.url)
        .header("Origin", &asr_info.origin)
        .header("Cookie", &cookie)
        .header("User-Agent", &asr_info.user_agent)
        .header("Host", "ws-samantha.doubao.com")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", tokio_tungstenite::tungstenite::handshake::client::generate_key())
        .body(())
        .map_err(|e| format!("Failed to build request: {}", e))?;

    // 尝试连接
    let (ws_stream, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| format!("WebSocket connection failed: {}", e))?;

    log::info!("[DoubaoASR] WebSocket connected, testing with finish signal...");

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // 发送 finish 信号测试
    let finish_msg = serde_json::json!({"event": "finish"});
    ws_tx.send(Message::Text(finish_msg.to_string()))
        .await
        .map_err(|e| format!("Failed to send test message: {}", e))?;

    // 等待响应，检查是否有 block 错误
    let timeout = tokio::time::Duration::from_secs(5);
    let result = tokio::time::timeout(timeout, async {
        while let Some(msg_result) = ws_rx.next().await {
            match msg_result {
                Ok(Message::Text(text)) => {
                    log::info!("[DoubaoASR] Test response: {}", text);
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                        // 检查是否有错误码
                        if let Some(code) = data.get("code").and_then(|c| c.as_i64()) {
                            if code != 0 {
                                let msg = data.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
                                doubao_cdp::clear_cached_cookies();
                                return Err(format!("ASR error: code={}, message={}", code, msg));
                            }
                        }
                        // 检查是否是 finish 响应
                        let event = data.get("event").and_then(|e| e.as_str()).unwrap_or("");
                        if event == "finish" {
                            return Ok(());
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    return Ok(());
                }
                Err(e) => {
                    return Err(format!("WebSocket error: {}", e));
                }
                _ => {}
            }
        }
        Ok(())
    }).await;

    match result {
        Ok(Ok(())) => {
            log::info!("[DoubaoASR] Connection test PASSED");
            Ok(())
        }
        Ok(Err(e)) => {
            log::error!("[DoubaoASR] Connection test FAILED: {}", e);
            Err(e)
        }
        Err(_) => {
            log::warn!("[DoubaoASR] Connection test timeout (might be OK)");
            Ok(()) // 超时不一定是错误
        }
    }
}

