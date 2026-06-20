//! ARM64 寄存器标识。
//!
//! 用我们自己的枚举而不是直接 re-export Unicorn 的 `RegisterARM64`，
//! 这样 backend 抽象层不依赖具体引擎，未来切换 backend 时上层代码无感。
//!
//! bootstrap 只列出跑通 stub 需要的寄存器：通用寄存器 X0..X30、SP、PC。
//! XZR（零寄存器）和向量 / 系统寄存器按需后续补。

/// ARM64 寄存器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arm64Reg {
    /// 通用寄存器 X0..X30。`idx` 超出范围由 backend 实现翻译为 `InvalidRegister`。
    X(u8),
    /// 栈指针。
    Sp,
    /// 程序计数器。
    Pc,
    /// 链接寄存器（X30 的别名，单独提供便于阅读）。
    Lr,
}
