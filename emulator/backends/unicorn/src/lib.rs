//! `rundroid-backend-unicorn`
//!
//! 基于 [`unicorn-engine`] 的 backend 实现，是 bootstrap 阶段的默认且唯一 backend。
//! 负责把 [`rundroid_backend`] 的抽象 trait 落到 Unicorn 引擎上，覆盖 ARM64 路径。
//!
//! 本 crate 必然包含 FFI 封装（Unicorn 是 C 库），因此不像 core / backend 那样 `forbid(unsafe_code)`，
//! 但所有 `unsafe` 都被限制在 `engine.rs` 内的薄适配层。

pub mod engine;

pub use engine::UnicornBackend;
