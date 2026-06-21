//! backend trait 定义。
//!
//! [`Backend`] 是工厂，[`Engine`] 是一次会话句柄。
//! 拆成两个 trait 是为了让上层在装配阶段选择 backend（工厂），
//! 但具体执行在 session 内通过 [`Engine`] 完成，互不耦合。

use crate::error::BackendError;
use crate::mem::MemPerms;
use crate::reg::Arm64Reg;
use rundroid_core::Arch;

/// backend 工厂。
///
/// 实现方（`UnicornBackend` 等）是无状态入口，
/// 真正的引擎状态由 [`Engine`] 实例持有。
pub trait Backend: Send + Sync {
    /// 按 `arch` 打开一个引擎实例。
    ///
    /// `arch` 与 backend 支持矩阵不匹配时返回 `Err`，
    /// 而不是 panic，让上层能在 case 中动态降级。
    fn open(&self, arch: Arch) -> Result<Box<dyn Engine>, BackendError>;
}

/// 一次 backend 执行句柄。
///
/// 所有方法都接收 `&mut self`，因为 emulator 内部维护大量可变状态（内存、寄存器、hook 表）。
/// `Send` 暂不要求，避免给 future async runtime 增加错误的暗示。
pub trait Engine {
    /// 在 guest 地址空间映射一段内存。
    fn mem_map(&mut self, addr: u64, size: usize, perms: MemPerms) -> Result<(), BackendError>;

    /// 向 guest 地址写入字节。
    fn mem_write(&mut self, addr: u64, bytes: &[u8]) -> Result<(), BackendError>;

    /// 从 guest 地址读取字节到调用方提供的缓冲区。
    fn mem_read(&self, addr: u64, buf: &mut [u8]) -> Result<(), BackendError>;

    /// 写入一个 64 位寄存器。
    fn reg_write(&mut self, reg: Arm64Reg, value: u64) -> Result<(), BackendError>;

    /// 读取一个 64 位寄存器。
    fn reg_read(&self, reg: Arm64Reg) -> Result<u64, BackendError>;

    /// 开始执行。
    ///
    /// - `begin`: 起始 PC。
    /// - `until`: 遇到该地址停止（"until" 模式）。`None` 表示不基于地址停止。
    /// - `timeout_us`: 超时（微秒），`None` 表示不限。
    /// - `count`: 最多执行多少条指令，`None` 表示不限。
    ///
    /// 至少需要 `until` / `timeout_us` / `count` 之一非 `None`，否则实现应返回 `Emulation` 错误，
    /// 避免误入无限执行。
    fn emu_start(
        &mut self,
        begin: u64,
        until: Option<u64>,
        timeout_us: Option<u64>,
        count: Option<usize>,
    ) -> Result<(), BackendError>;

    /// 请求当前执行停止（通常在 hook 回调中调用）。
    fn emu_stop(&mut self);

    /// 修改一段已映射 guest 内存的权限。
    ///
    /// 用于 loader 装载完段数据后按 ELF p_flags 精确收紧权限
    /// （例如把 RX 段从 footprint 的 RWX 改成 RX、把 RELRO 区改成 R）。
    /// 区段必须先 `mem_map`；未映射时返回 `MemoryAccess` 错误。
    fn mem_protect(&mut self, addr: u64, size: usize, perms: MemPerms) -> Result<(), BackendError>;

    /// 注册 SVC 指令 hook。
    ///
    /// 当 guest 执行到 `svc #0` 时，backend 会回调 [`SyscallHook`]。
    /// hook 内部负责：
    /// - 读取 x8（syscall 号）与 x0..x5（参数）
    /// - 把结果写回 x0
    /// - 如果返回 `Exit`，调用 [`Self::emu_stop`] 让 emu_start 返回
    ///
    /// 注册必须在 `emu_start` 之前完成；同一段时间内只能挂一个 syscall hook。
    fn install_syscall_hook(&mut self, hook: Box<dyn SyscallHook>) -> Result<(), BackendError>;
}

/// SVC hook 抽象。
///
/// 故意设计成"backend 调用 hook"的反向控制流：
/// backend 拿到 emulator 句柄后调用 hook.on_svc，
/// hook 用 [`GuestCPU`] 读写寄存器并决定是否停止。
/// 这样上层（case-runner）可以在 hook 里把 syscall 分派到 LinuxRuntime，
/// 而不需要让 backend 知道 LinuxRuntime 的存在。
pub trait SyscallHook: Send {
    fn on_svc(&mut self, cpu: &mut dyn GuestCPU);
}

/// hook 内可对 CPU 做的操作子集。
///
/// 不直接给 `&mut dyn Engine` 是为了让 backend 在 hook 期间
/// 仍能保持对 emulator 的独占借用——backend 暴露一个受限视图即可。
///
/// mem_read / mem_write 故意设计为可失败：
/// syscall 路径上 guest 给的缓冲地址可能未映射，必须把失败上报成 EFAULT，
/// 否则会出现"写没写成功都返回 N"的假阳性。
pub trait GuestCPU {
    fn reg_read(&self, reg: Arm64Reg) -> u64;
    fn reg_write(&mut self, reg: Arm64Reg, value: u64);
    /// 从 guest 地址读 `buf.len()` 字节填入 `buf`。
    /// 失败（地址未映射 / 权限不足）时返回 `false`。
    fn mem_read(&self, addr: u64, buf: &mut [u8]) -> bool;
    /// 向 guest 地址写字节。失败时返回 `false`，调用方据此返回 EFAULT。
    fn mem_write(&mut self, addr: u64, bytes: &[u8]) -> bool;
    /// 在 guest 地址空间建立映射。
    ///
    /// syscall 层的 mmap 通过此方法在目标侧建立真实映射。
    /// 如果地址已被占用或其他原因失败，返回 `BackendError`。
    fn mem_map(&mut self, addr: u64, size: usize, perms: MemPerms) -> Result<(), BackendError>;
    fn stop(&mut self);
}
