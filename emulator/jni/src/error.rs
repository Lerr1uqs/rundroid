//! JNI shim foundation 统一错误类型。
//!
//! 所有 JNI 路径上的错误都归一化到此枚举，确保 register/verify/dispatch
//! 各阶段都有明确的错误归因信息。

use crate::types::{JType, MethodSig};

/// JNI foundation 错误枚举。
///
/// 每个变体携带足够的上下文信息，以便 telemetry 和日志能准确定位问题。
#[derive(Debug, thiserror::Error)]
pub enum JniError {
    /// descriptor 格式不合法。
    /// 携带原始 descriptor 字符串。
    #[error("无效的 descriptor: `{0}`")]
    InvalidDescriptor(String),

    /// class 未在 registry 中注册。
    /// 携带 slash-separated class name。
    #[error("class 未注册: `{0}`")]
    ClassNotFound(String),

    /// method 未在 registry 中注册。
    /// 携带完整 MethodSig。
    #[error("method 未注册: {0}")]
    MethodNotFound(MethodSig),

    /// field 名称在 class 中未找到。
    /// 携带 class name 和 field 名称。
    #[error("field 未注册: `{0}.{1}`")]
    FieldNotFound(String, String),

    /// 重复注册。携带冲突的签名描述。
    #[error("重复注册: `{0}`")]
    DuplicateRegistration(String),

    /// 类型不匹配。携带期望类型和实际类型。
    #[error("类型不匹配: 期望 `{expected:?}`, 实际 `{actual:?}`")]
    TypeMismatch {
        expected: JType,
        actual: JType,
    },

    /// Null 值出现在不允许 Null 的位置（如 primitive 参数）。
    #[error("Null 值不允许在此位置: `{0}`")]
    NullNotAllowed(String),

    /// Python 注解与 Java descriptor 校验失败。
    /// 携带 class / member 名称、descriptor、期望和实际信息。
    #[error("校验失败: class=`{class_name}`, member=`{member}`, descriptor=`{descriptor}`, 期望=`{expected:?}`, 实际=`{actual:?}`")]
    VerifyFailed {
        class_name: String,
        member: String,
        descriptor: String,
        expected: String,
        actual: String,
    },

    /// 方法调用时传入的参数数量不匹配。
    #[error("参数数量不匹配: 期望 {expected} 个, 实际 {actual} 个")]
    ArgCountMismatch {
        expected: usize,
        actual: usize,
    },

    /// 内部错误（不应该发生的异常状态）。
    #[error("内部错误: `{0}`")]
    Internal(String),

    /// static method/field 只能通过 call_static / get_static 访问。
    #[error("method/field 是 static，请使用 call_static/get_static: `{0}`")]
    StaticOnly(String),

    /// instance method/field 只能通过 call / get 访问。
    #[error("method/field 不是 static，请使用 call_method/get_field: `{0}`")]
    InstanceOnly(String),
}
