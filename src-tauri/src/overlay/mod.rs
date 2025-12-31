//! Overlay 模块
//!
//! 纯 UI 浮层，显示识别状态和结果

pub mod panel;

pub use panel::{hide, preload, show, update_status, update_text};
