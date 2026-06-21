//! Unicorn backend 适配层。
//!
//! 这一层是唯一接触 `unicorn-engine` API 的地方：
//! - 把 [`Arm64Reg`](rundroid_backend::Arm64Reg) 翻译成 `RegisterARM64`
//! - 把 [`MemPerms`](rundroid_backend::MemPerms) 翻译成 `Prot`
//! - 把 `uc_error` 翻译成 [`BackendError`](rundroid_backend::BackendError)
//!
//! 上层只看到 trait 抽象，从而保留未来切换 / 并存其它 backend 的能力。

use rundroid_backend::{
    Arm64Reg, Backend, BackendError, Engine as BackendEngine, MemPerms, GuestCPU, SyscallHook,
};
use rundroid_core::Arch;
use std::cell::RefCell;
use std::rc::Rc;
use unicorn_engine::unicorn_const::{Arch as UcArch, Mode, Prot};
use unicorn_engine::{RegisterARM64, Unicorn, uc_error};

/// 无状态 backend 工厂。一次进程内可以重复构造多个独立 [`UnicornEngine`]。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnicornBackend;

impl UnicornBackend {
    pub const fn new() -> Self {
        Self
    }
}

impl Backend for UnicornBackend {
    fn open(&self, arch: Arch) -> Result<Box<dyn BackendEngine>, BackendError> {
        match arch {
            Arch::Arm64 => {
                // ARM64 在 Unicorn 里固定走 ARM (AArch64) mode。
                let uc = Unicorn::new(UcArch::ARM64, Mode::ARM)
                    .map_err(map_init_err)?;
                Ok(Box::new(UnicornEngine {
                    uc,
                    // 用 Rc<RefCell<>> 让 unicorn 内部 hook 闭包能回调到外部 hook 对象，
                    // 同时保持 'static 生命周期（Unicorn<'static, ()> 要求闭包 'static）。
                    hook_slot: Rc::new(RefCell::new(None)),
                }))
            }
            // `Arch` 是 `#[non_exhaustive]`，未来新增架构在此显式拒绝，
            // 而不是依赖编译器穷尽性检查（那会因为新变体在本 crate 之外而失败）。
            _ => Err(BackendError::Init(
                "unicorn backend does not support this arch yet",
            )),
        }
    }
}

/// Unicorn 引擎实例。持有完整 emulator 状态。
pub struct UnicornEngine {
    uc: Unicorn<'static, ()>,
    /// SVC hook 共享槽位。UnicornEngine 与 unicorn 内部闭包各持一份 Rc。
    hook_slot: Rc<RefCell<Option<Box<dyn SyscallHook>>>>,
}

/// hook 闭包内临时借用 Unicorn，用于读写寄存器、停机。
///
/// 双生命周期：`'a` 是借用本身的，`'u` 是 Unicorn 数据生命周期；
/// unicorn 的 callback 给的是 `&mut Unicorn<'u, ()>`，`'u` 在闭包边界由 Rust 推断。
struct UnicornGuestCPU<'a, 'u: 'a> {
    uc: &'a mut Unicorn<'u, ()>,
    stop_requested: bool,
}

impl<'a, 'u: 'a> GuestCPU for UnicornGuestCPU<'a, 'u> {
    fn reg_read(&self, reg: Arm64Reg) -> u64 {
        // 传错寄存器编号是 bug，应直接 panic（let-it-failed）
        let r = translate_reg(reg).unwrap_or_else(|_| {
            panic!("reg_read: 无效的寄存器 {:?}", reg)
        });
        // unicorn reg_read 失败时也直接 panic — 读不存在的寄存器是异常状态
        self.uc.reg_read(r).unwrap_or_else(|e| {
            panic!("reg_read: unicorn 读寄存器失败: {e:?}")
        })
    }
    fn reg_write(&mut self, reg: Arm64Reg, value: u64) {
        let r = translate_reg(reg).unwrap_or_else(|_| {
            panic!("reg_write: 无效的寄存器 {:?}", reg)
        });
        self.uc.reg_write(r, value).unwrap_or_else(|e| {
            panic!("reg_write: unicorn 写寄存器失败: {e:?}")
        });
    }
    fn mem_read(&self, addr: u64, buf: &mut [u8]) -> bool {
        self.uc.mem_read(addr, buf).is_ok()
    }
    fn mem_write(&mut self, addr: u64, bytes: &[u8]) -> bool {
        self.uc.mem_write(addr, bytes).is_ok()
    }
    fn mem_map(&mut self, addr: u64, size: usize, perms: MemPerms) -> Result<(), BackendError> {
        self.uc
            .mem_map(addr, size as u64, translate_perms(perms))
            .map_err(|_| BackendError::MapFailed {
                addr,
                size: size as u64,
                reason: "syscall mmap rejected by unicorn",
            })
    }
    fn stop(&mut self) {
        self.stop_requested = true;
    }
}

impl BackendEngine for UnicornEngine {
    fn mem_map(&mut self, addr: u64, size: usize, perms: MemPerms) -> Result<(), BackendError> {
        self.uc
            .mem_map(addr, size as u64, translate_perms(perms))
            .map_err(|_| BackendError::MapFailed {
                addr,
                size: size as u64,
                reason: "unicorn rejected mem_map (unaligned / zero / overlap)",
            })
    }

    fn mem_write(&mut self, addr: u64, bytes: &[u8]) -> Result<(), BackendError> {
        self.uc
            .mem_write(addr, bytes)
            .map_err(|_| BackendError::MemoryAccess { addr, len: bytes.len() })
    }

    fn mem_read(&self, addr: u64, buf: &mut [u8]) -> Result<(), BackendError> {
        self.uc
            .mem_read(addr, buf)
            .map_err(|_| BackendError::MemoryAccess { addr, len: buf.len() })
    }

    fn reg_write(&mut self, reg: Arm64Reg, value: u64) -> Result<(), BackendError> {
        let r = translate_reg(reg)?;
        self.uc
            .reg_write(r, value)
            .map_err(|_| BackendError::InvalidRegister(reg_id_raw(reg)))
    }

    fn reg_read(&self, reg: Arm64Reg) -> Result<u64, BackendError> {
        let r = translate_reg(reg)?;
        self.uc
            .reg_read(r)
            .map_err(|_| BackendError::InvalidRegister(reg_id_raw(reg)))
    }

    fn emu_start(
        &mut self,
        begin: u64,
        until: Option<u64>,
        timeout_us: Option<u64>,
        count: Option<usize>,
    ) -> Result<(), BackendError> {
        // 防止误入无限执行：至少需要一个边界条件。
        if until.is_none() && timeout_us.is_none() && count.is_none() {
            return Err(BackendError::Emulation(
                "emu_start requires at least one of until/timeout/count to bound execution",
            ));
        }
        let until_addr = until.unwrap_or(0);
        let timeout = timeout_us.unwrap_or(0);
        let count_opt = count.unwrap_or(0);
        self.uc
            .emu_start(begin, until_addr, timeout, count_opt)
            .map_err(map_emu_err)
    }

    fn emu_stop(&mut self) {
        // emu_stop 的返回值仅在异步上下文有意义，这里忽略。
        let _ = self.uc.emu_stop();
    }

    fn mem_protect(
        &mut self,
        addr: u64,
        size: usize,
        perms: MemPerms,
    ) -> Result<(), BackendError> {
        // Unicorn 的 mem_protect 要求 addr/size page 对齐；
        // 这里向上对齐以容忍 loader 传入精确段边界。
        const PAGE: u64 = 0x1000;
        let aligned_addr = addr & !(PAGE - 1);
        let end = (addr + size as u64 + PAGE - 1) & !(PAGE - 1);
        let aligned_size = end.saturating_sub(aligned_addr);
        self.uc
            .mem_protect(aligned_addr, aligned_size, translate_perms(perms))
            .map_err(|_| BackendError::MemoryAccess {
                addr: aligned_addr,
                len: aligned_size as usize,
            })
    }

    fn install_syscall_hook(
        &mut self,
        hook: Box<dyn SyscallHook>,
    ) -> Result<(), BackendError> {
        // 把 hook 装进共享 slot，再注册一个 unicorn INTR hook。
        // ARM64 上 SVC 触发 UC_HOOK_INTR，intno == 2（UC_INTR_ARM64_SVC）。
        *self.hook_slot.borrow_mut() = Some(hook);
        let slot = self.hook_slot.clone();
        self.uc
            .add_intr_hook(move |uc, intno| {
                // ARM64: SVC = 2, HVC = 3, SMC = 4。bootstrap 只关心 SVC。
                if intno != 2 {
                    return;
                }
                let mut borrow = slot.borrow_mut();
                let Some(hook) = borrow.as_mut() else {
                    return;
                };
                let mut cpu = UnicornGuestCPU {
                    uc,
                    stop_requested: false,
                };
                hook.on_svc(&mut cpu);
                if cpu.stop_requested {
                    let _ = uc.emu_stop();
                }
            })
            .map_err(|_| BackendError::Init("failed to install syscall hook"))?;
        Ok(())
    }
}

fn map_init_err(_e: uc_error) -> BackendError {
    BackendError::Init("unicorn arm64 engine init failed")
}

fn map_emu_err(_e: uc_error) -> BackendError {
    BackendError::Emulation(
        "unicorn emu_start raised an error (unmapped execution, invalid insn, or timeout)",
    )
}

/// 把抽象权限位翻译成 Unicorn 的 `Prot`。
fn translate_perms(perms: MemPerms) -> Prot {
    let mut p = Prot::NONE;
    if perms.readable() {
        p |= Prot::READ;
    }
    if perms.writable() {
        p |= Prot::WRITE;
    }
    if perms.executable() {
        p |= Prot::EXEC;
    }
    p
}

/// 把 [`Arm64Reg`] 翻译成 Unicorn 的 `RegisterARM64`。
/// `idx > 30` 视为非法（XZR / 系统寄存器 bootstrap 暂不暴露）。
fn translate_reg(reg: Arm64Reg) -> Result<RegisterARM64, BackendError> {
    Ok(match reg {
        Arm64Reg::X(idx) => match idx {
            0 => RegisterARM64::X0,
            1 => RegisterARM64::X1,
            2 => RegisterARM64::X2,
            3 => RegisterARM64::X3,
            4 => RegisterARM64::X4,
            5 => RegisterARM64::X5,
            6 => RegisterARM64::X6,
            7 => RegisterARM64::X7,
            8 => RegisterARM64::X8,
            9 => RegisterARM64::X9,
            10 => RegisterARM64::X10,
            11 => RegisterARM64::X11,
            12 => RegisterARM64::X12,
            13 => RegisterARM64::X13,
            14 => RegisterARM64::X14,
            15 => RegisterARM64::X15,
            16 => RegisterARM64::X16,
            17 => RegisterARM64::X17,
            18 => RegisterARM64::X18,
            19 => RegisterARM64::X19,
            20 => RegisterARM64::X20,
            21 => RegisterARM64::X21,
            22 => RegisterARM64::X22,
            23 => RegisterARM64::X23,
            24 => RegisterARM64::X24,
            25 => RegisterARM64::X25,
            26 => RegisterARM64::X26,
            27 => RegisterARM64::X27,
            28 => RegisterARM64::X28,
            29 => RegisterARM64::X29,
            30 => RegisterARM64::X30,
            _ => return Err(BackendError::InvalidRegister(idx as u32)),
        },
        Arm64Reg::Sp => RegisterARM64::SP,
        Arm64Reg::Pc => RegisterARM64::PC,
        Arm64Reg::Lr => RegisterARM64::LR,
    })
}

/// 仅用于 error message 中的稳定 ID（不暴露 Unicorn 的 enum 数值语义）。
fn reg_id_raw(reg: Arm64Reg) -> u32 {
    match reg {
        Arm64Reg::X(i) => i as u32,
        Arm64Reg::Sp => 0x100,
        Arm64Reg::Pc => 0x101,
        Arm64Reg::Lr => 0x102,
    }
}
