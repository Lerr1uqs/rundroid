//! backend 选型。
//!
//! 与 `Arch` 同理用 enum 表达，避免散落的字符串配置。
//! runtime core 只引用 [`BackendKind`] 这个枚举本身，
//! 不绑定任何具体 backend 的句柄类型——那是 `rundroid-backend` 的事。

/// 可选的 CPU emulator backend。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum BackendKind {
    /// Unicorn 引擎，bootstrap 阶段的默认且唯一实现。
    #[default]
    Unicorn,
}
