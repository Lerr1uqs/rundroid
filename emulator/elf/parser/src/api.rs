//! parser trait 与输入 / 输出契约。
//!
//! [`ElfParser`] 是抽象 trait，默认实现 [`crate::parser_elf::ElfCrateParser`]
//! 基于 `elf` crate。trait 的存在让未来可以插入 `goblin` / 自写 parser 作为对照，
//! 而不动 loader / linker。

/// 解析策略开关。
///
/// bootstrap 阶段只暴露"严格度"一个维度：
/// 严格模式下任何非致命可疑点都会变成 `Policy` 错误，
/// 宽松模式下则只记录 [`crate::model::ParseNote`] 让上层决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParsePolicy {
    pub strict: bool,
}

impl ParsePolicy {
    pub fn lenient() -> Self {
        Self { strict: false }
    }
    pub fn strict() -> Self {
        Self { strict: true }
    }
}

/// parser 输入。
#[derive(Debug, Clone, Copy)]
pub struct ParseInput<'a> {
    /// 模块名（通常是 .so 文件名），用于日志 / telemetry 归因。
    pub module_name: &'a str,
    /// 待解析的 ELF 字节流。
    pub bytes: &'a [u8],
    pub policy: ParsePolicy,
}

impl<'a> ParseInput<'a> {
    pub fn new(module_name: &'a str, bytes: &'a [u8]) -> Self {
        Self {
            module_name,
            bytes,
            policy: ParsePolicy::default(),
        }
    }
}

/// ELF parser 抽象。
///
/// 实现方必须 [`Send + Sync`]：parser 应是无状态的，可以在线程间共享。
pub trait ElfParser: Send + Sync {
    fn parse(&self, input: ParseInput<'_>) -> Result<crate::model::ParsedElf, crate::ElfParseError>;
}
