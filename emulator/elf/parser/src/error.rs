//! ELF 解析层错误模型。
//!
//! 与 loader / linker 错误严格分工（spec: Typed error separation）：
//! 这里只描述"输入字节流不符合 ELF 格式"类问题，
//! 不掺杂映射失败、符号找不到、relocation 无法写回等运行时问题。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ElfParseError {
    /// 字节流连 ELF 魔数都不匹配，根本不是 ELF。
    #[error("not an ELF file: bad magic")]
    BadMagic,

    /// 字节流过短，header / section / program header 越界。
    #[error("truncated ELF input: {0}")]
    Truncated(&'static str),

    /// 解析出来的架构 / class / endianness 不被 bootstrap 支持。
    /// 例如 ARM32、x86_64、ELF32 在 bootstrap 阶段都会落到这里。
    #[error("unsupported ELF configuration: {0}")]
    Unsupported(&'static str),

    /// dynamic table 自相矛盾（例如 `DT_STRTAB` 指向的 offset 越界）。
    #[error("malformed dynamic table: {0}")]
    MalformedDynamic(&'static str),

    /// `ParsePolicy` 要求严格而输入违反了某条策略（例如不允许未对齐段）。
    #[error("policy violation: {0}")]
    Policy(&'static str),
}
