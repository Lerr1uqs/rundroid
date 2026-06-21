//! emulator 装配点。
//!
//! [`Emulator`] 是 bootstrap 主线的入口对象，它的职责刻意保持窄：
//! 持有 [`RuntimeConfig`](crate::config::RuntimeConfig)、
//! [`AndroidRuntime`](rundroid_jni::AndroidRuntime) 并提供 session 工厂。
//!
//! backend / memory / telemetry router 的具体实例由后续 task 在装配阶段注入，
//! core 不持有这些类型，从而避免循环依赖。

use crate::config::RuntimeConfig;
use crate::session::Session;
use rundroid_jni::AndroidRuntime;

/// emulator 实例。
///
/// 持有运行时配置和 Android VM 状态。
/// `AndroidRuntime` 是 Python decorator / Rust builtin 注册链路的最终同步点。
pub struct Emulator {
    config: RuntimeConfig,
    /// Android VM 运行时——class / object / ref / exception / apk 的权威容器。
    pub android: AndroidRuntime,
}

impl Emulator {
    /// 创建新的 emulator 实例。
    pub fn new(config: RuntimeConfig) -> Self {
        Self {
            config,
            android: AndroidRuntime::new(),
        }
    }

    /// 创建带已有 `AndroidRuntime` 的 emulator 实例。
    ///
    /// 用于装配层已经初始化好 VM 状态（如已注册 framework class）的场景。
    pub fn with_android_runtime(config: RuntimeConfig, android: AndroidRuntime) -> Self {
        Self { config, android }
    }

    /// 当前 emulator 的配置（不可变视图）。
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// 启动一个新的执行 session。
    ///
    /// 每次 case 执行都应通过这里获取 session，保证 id 边界清晰。
    pub fn start_session(&self) -> Session {
        Session::new()
    }
}
