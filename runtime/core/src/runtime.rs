//! runtime 装配点。
//!
//! [`Runtime`] 是 bootstrap 主线的入口对象，它的职责刻意保持窄：
//! 持有 [`RuntimeConfig`](crate::config::RuntimeConfig) 并提供 session 工厂。
//!
//! backend / memory / telemetry router 的具体实例由后续 task 在装配阶段注入，
//! core 不持有这些类型，从而避免循环依赖。

use crate::config::RuntimeConfig;
use crate::session::Session;

/// runtime 实例。
pub struct Runtime {
    config: RuntimeConfig,
}

impl Runtime {
    pub fn new(config: RuntimeConfig) -> Self {
        Self { config }
    }

    /// 当前 runtime 的配置（不可变视图）。
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
