//! 音频采集 - 累积到 4096 samples 再发送
//!
//! 重采样算法通过环境变量切换:
//! - TYPEFREE_RESAMPLE=linear (默认)
//! - TYPEFREE_RESAMPLE=sinc (高质量)

use crate::resample;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

const CHUNK_SIZE: usize = 4096;

/// 预热麦克风 - 在启动时调用，触发系统权限弹窗
/// 这样用户第一次使用时就不会卡掉语音
pub fn warmup_microphone() {
    log::info!("[Audio] Warming up microphone to trigger permission prompt...");

    std::thread::spawn(|| {
        let host = cpal::default_host();

        let device = match host.default_input_device() {
            Some(d) => d,
            None => {
                log::warn!("[Audio] No input device found during warmup");
                return;
            }
        };

        let config = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[Audio] Failed to get input config during warmup: {}", e);
                return;
            }
        };

        // 创建一个短暂的输入流来触发权限弹窗
        let stream = device.build_input_stream(
            &cpal::StreamConfig {
                channels: config.channels(),
                sample_rate: config.sample_rate(),
                buffer_size: cpal::BufferSize::Default,
            },
            |_data: &[f32], _: &cpal::InputCallbackInfo| {
                // 不做任何处理，只是为了触发权限
            },
            |err| log::warn!("[Audio] Warmup stream error: {}", err),
            None,
        );

        match stream {
            Ok(s) => {
                if let Err(e) = s.play() {
                    log::warn!("[Audio] Failed to play warmup stream: {}", e);
                    return;
                }
                // 运行 100ms 就够了
                std::thread::sleep(std::time::Duration::from_millis(100));
                log::info!("[Audio] Microphone warmup complete");
            }
            Err(e) => {
                log::warn!("[Audio] Failed to build warmup stream: {}", e);
            }
        }
    });
}

pub fn start_recording(
    tx: Sender<Vec<u8>>,
    stop_flag: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>, Box<dyn std::error::Error + Send + Sync>> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or("No input device")?;

    log::info!("[Audio] Device: {}", device.name()?);

    let config = device.default_input_config()?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels();

    log::info!(
        "[Audio] Config: {}Hz, {} channels, format: {:?}",
        sample_rate,
        channels,
        config.sample_format()
    );

    let handle = std::thread::spawn(move || {
        // 累积 buffer
        let buffer: Arc<Mutex<Vec<i16>>> =
            Arc::new(Mutex::new(Vec::with_capacity(CHUNK_SIZE * 2)));

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                // 每个分支独立 clone，避免变量被多个 move 闭包捕获
                let buffer_clone = buffer.clone();
                let tx_clone = tx.clone();

                device.build_input_stream(
                    &cpal::StreamConfig {
                        channels,
                        sample_rate: config.sample_rate(),
                        buffer_size: cpal::BufferSize::Default,
                    },
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        // f32 → i16, 48kHz → 16kHz, stereo → mono
                        let samples = convert_to_16k_mono(data, sample_rate, channels);

                        let mut buf = buffer_clone.lock().unwrap();
                        buf.extend(samples);

                        // 达到 CHUNK_SIZE 就发送
                        while buf.len() >= CHUNK_SIZE {
                            let chunk: Vec<i16> = buf.drain(..CHUNK_SIZE).collect();
                            let bytes: Vec<u8> =
                                chunk.iter().flat_map(|&s| s.to_le_bytes()).collect();
                            let _ = tx_clone.send(bytes);
                        }
                    },
                    |err| log::error!("[Audio] Stream error (F32): {}", err),
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                // 每个分支独立 clone，避免变量被多个 move 闭包捕获
                let buffer_clone = buffer.clone();
                let tx_clone = tx.clone();

                device.build_input_stream(
                    &cpal::StreamConfig {
                        channels,
                        sample_rate: config.sample_rate(),
                        buffer_size: cpal::BufferSize::Default,
                    },
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let samples = convert_i16_to_16k_mono(data, sample_rate, channels);

                        let mut buf = buffer_clone.lock().unwrap();
                        buf.extend(samples);

                        while buf.len() >= CHUNK_SIZE {
                            let chunk: Vec<i16> = buf.drain(..CHUNK_SIZE).collect();
                            let bytes: Vec<u8> =
                                chunk.iter().flat_map(|&s| s.to_le_bytes()).collect();
                            let _ = tx_clone.send(bytes);
                        }
                    },
                    |err| log::error!("[Audio] Stream error (I16): {}", err),
                    None,
                )
            }
            format => {
                log::error!("[Audio] Unsupported sample format: {:?}", format);
                return;
            }
        };

        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                log::error!("[Audio] Failed to build stream: {}", e);
                return;
            }
        };

        if let Err(e) = stream.play() {
            log::error!("[Audio] Failed to play stream: {}", e);
            return;
        }

        log::info!("[Audio] Recording started");

        while !stop_flag.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        log::info!("[Audio] Stop flag received, flushing buffer");

        // 发送剩余数据
        let buf = buffer.lock().unwrap();
        if !buf.is_empty() {
            log::info!("[Audio] Sending remaining {} samples", buf.len());
            let bytes: Vec<u8> = buf.iter().flat_map(|&s| s.to_le_bytes()).collect();
            let _ = tx.send(bytes);
        }

        log::info!("[Audio] Recording stopped");
    });

    Ok(handle)
}

/// f32 → 16kHz mono samples
fn convert_to_16k_mono(data: &[f32], sample_rate: u32, channels: u16) -> Vec<i16> {
    // f32 → i16 (with clamp to prevent overflow)
    let i16_data: Vec<i16> = data.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    convert_i16_to_16k_mono(&i16_data, sample_rate, channels)
}

/// i16 → 16kHz mono samples (使用 resample 模块)
fn convert_i16_to_16k_mono(data: &[i16], sample_rate: u32, channels: u16) -> Vec<i16> {
    // stereo → mono
    let mono: Vec<i16> = if channels > 1 {
        data.chunks(channels as usize)
            .map(|chunk| (chunk.iter().map(|&s| s as i32).sum::<i32>() / channels as i32) as i16)
            .collect()
    } else {
        data.to_vec()
    };

    // resample to 16kHz (算法由环境变量 TYPEFREE_RESAMPLE 控制)
    resample::resample(&mono, sample_rate, 16000)
}
