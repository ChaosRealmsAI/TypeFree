//! 重采样模块 - 支持线性插值和 Sinc 两种算法的 A/B 测试
//!
//! 通过环境变量 `TYPEFREE_RESAMPLE` 切换:
//! - `linear` (默认): 线性插值，低延迟，质量一般
//! - `sinc`: Sinc 插值 + 抗混叠，高质量，略高延迟

use std::sync::OnceLock;

/// 重采样算法类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResampleMethod {
    Linear,
    Sinc,
}

impl ResampleMethod {
    pub fn from_env() -> Self {
        match std::env::var("TYPEFREE_RESAMPLE").as_deref() {
            Ok("sinc") => Self::Sinc,
            _ => Self::Linear,
        }
    }
}

/// 全局重采样器（Sinc 需要状态）
static SINC_RESAMPLER: OnceLock<std::sync::Mutex<Option<SincResampler>>> = OnceLock::new();

/// Sinc 重采样器封装
struct SincResampler {
    resampler: rubato::SincFixedIn<f32>,
    from_rate: u32,
    to_rate: u32,
}

impl SincResampler {
    fn new(from_rate: u32, to_rate: u32, chunk_size: usize) -> Result<Self, String> {
        use rubato::{SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

        let params = SincInterpolationParameters {
            sinc_len: 64,           // 平衡质量和性能
            f_cutoff: 0.95,         // 截止频率
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window: WindowFunction::Blackman,
        };

        let ratio = to_rate as f64 / from_rate as f64;

        let resampler = SincFixedIn::new(
            ratio,
            2.0,        // max relative ratio
            params,
            chunk_size,
            1,          // mono
        ).map_err(|e| format!("Failed to create Sinc resampler: {}", e))?;

        Ok(Self {
            resampler,
            from_rate,
            to_rate,
        })
    }

    fn process(&mut self, input: &[f32]) -> Result<Vec<f32>, String> {
        use rubato::Resampler;

        if input.is_empty() {
            return Ok(Vec::new());
        }

        let input_frames = vec![input.to_vec()];

        match self.resampler.process(&input_frames, None) {
            Ok(output) => Ok(output.into_iter().next().unwrap_or_default()),
            Err(e) => Err(format!("Sinc resample error: {}", e)),
        }
    }
}

/// 重采样入口函数
///
/// 返回 16kHz mono i16 samples
pub fn resample(input: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    static METHOD: OnceLock<ResampleMethod> = OnceLock::new();
    let method = *METHOD.get_or_init(|| {
        let m = ResampleMethod::from_env();
        log::info!("[Resample] Using {:?} method", m);
        m
    });

    if from_rate == to_rate {
        return input.to_vec();
    }

    match method {
        ResampleMethod::Linear => resample_linear(input, from_rate, to_rate),
        ResampleMethod::Sinc => resample_sinc(input, from_rate, to_rate),
    }
}

/// 线性插值重采样（原实现）
fn resample_linear(input: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if input.is_empty() {
        return Vec::new();
    }
    if input.len() == 1 {
        return input.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let new_len = (input.len() as f64 / ratio) as usize;

    if new_len == 0 {
        return Vec::new();
    }

    let mut output = Vec::with_capacity(new_len);

    for i in 0..new_len {
        let src_pos = i as f64 * ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;

        let sample = if src_idx + 1 < input.len() {
            let y0 = input[src_idx] as f64;
            let y1 = input[src_idx + 1] as f64;
            (y0 + (y1 - y0) * frac) as i16
        } else {
            input[input.len() - 1]
        };

        output.push(sample);
    }

    output
}

/// Sinc 重采样（高质量）
fn resample_sinc(input: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if input.is_empty() {
        return Vec::new();
    }

    // i16 -> f32
    let input_f32: Vec<f32> = input.iter().map(|&s| s as f32 / 32768.0).collect();

    // 获取或创建重采样器
    let mutex = SINC_RESAMPLER.get_or_init(|| std::sync::Mutex::new(None));
    let mut guard = mutex.lock().unwrap();

    // 检查是否需要重新创建（采样率变化或首次使用）
    let need_recreate = match guard.as_ref() {
        Some(r) => r.from_rate != from_rate || r.to_rate != to_rate,
        None => true,
    };

    if need_recreate {
        match SincResampler::new(from_rate, to_rate, input.len().max(1024)) {
            Ok(r) => {
                log::info!(
                    "[Resample] Created Sinc resampler: {}Hz -> {}Hz",
                    from_rate, to_rate
                );
                *guard = Some(r);
            }
            Err(e) => {
                log::error!("[Resample] {}, falling back to linear", e);
                return resample_linear(input, from_rate, to_rate);
            }
        }
    }

    // 执行重采样
    let result = match guard.as_mut() {
        Some(resampler) => resampler.process(&input_f32),
        None => return resample_linear(input, from_rate, to_rate),
    };

    match result {
        Ok(output_f32) => {
            // f32 -> i16
            output_f32
                .iter()
                .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                .collect()
        }
        Err(e) => {
            log::error!("[Resample] {}, falling back to linear", e);
            resample_linear(input, from_rate, to_rate)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_downsample() {
        let input: Vec<i16> = (0..4800).map(|i| (i % 1000) as i16).collect();
        let output = resample_linear(&input, 48000, 16000);
        assert_eq!(output.len(), 1600);
    }

    #[test]
    fn test_sinc_downsample() {
        let input: Vec<i16> = (0..4800).map(|i| ((i as f32 * 0.1).sin() * 10000.0) as i16).collect();
        let output = resample_sinc(&input, 48000, 16000);
        // Sinc 输出长度可能略有差异
        assert!(output.len() >= 1500 && output.len() <= 1700);
    }
}
