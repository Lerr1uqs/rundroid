//! syscall 分发。
//!
//! bootstrap 阶段维护一个 [`LinuxRuntime`]，
//! backend 在执行过程中遇到 `svc #0` 时把寄存器状态交给本层，
//! 本层按 AArch64 syscall 号分发。
//!
//! 已覆盖的 syscall：
//! | 编号 | 名字 | 行为 |
//! |---|---|---|
//! | 56  | openat | 按 path 解析 VFS，分配 fd |
//! | 57  | close  | 关闭 fd |
//! | 63  | read   | 从 fd 读字节到 guest 缓冲 |
//! | 64  | write  | 把 guest 缓冲字节记录到 stdout ring |
//! | 93  | exit   | 请求停止执行 |
//! | 94  | exit_group | 同 exit |
//! | 160 | ugetrlwind / getuid 等 | 简化返回 0 |
//! | 200 | getrandom | 直接填 guest 缓冲 |
//! | 222  | mmap   | 简化：返回固定地址（guest 端不真正 mmap，仅占位） |
//! | 215 | munmap | no-op |
//! | 214 | brk    | 返回当前 brk |
//!
//! 未识别的 syscall 返回 `-ENOSYS`，让 case 显式失败而不是静默错。

use crate::fd::{Fd, FdTable, FdType};
use crate::vfs::{VfsSource, resolve};

/// ARM64 Linux syscall 号（仅 bootstrap 用到的子集）。
const SYS_OPENAT: u64 = 56;
const SYS_CLOSE: u64 = 57;
const SYS_READ: u64 = 63;
const SYS_WRITE: u64 = 64;
const SYS_EXIT: u64 = 93;
const SYS_EXIT_GROUP: u64 = 94;
const SYS_BRK: u64 = 214;
const SYS_MMAP: u64 = 222;
const SYS_MUNMAP: u64 = 215;
const SYS_GETRANDOM: u64 = 278;

/// 简化的 errno（POSIX 风格负数）。
pub const ENOSYS: i64 = -38;
pub const EBADF: i64 = -9;
pub const EFAULT: i64 = -14;

/// syscall 执行结果。
///
/// `Done(value)` 对应寄存器 x0 的写入值；
/// `Exit(code)` 通知 backend 停止执行。
#[derive(Debug, Clone, Copy)]
pub enum SyscallResult {
    Done(u64),
    Exit(i32),
}

/// Linux 用户态运行时。
pub struct LinuxRuntime {
    pub fds: FdTable,
    /// mmap 的"下一次返回地址"。简化：每次 mmap 返回单调递增的固定区域。
    next_mmap: u64,
    /// brk 当前值。
    brk: u64,
    /// 收集到的 stdout 字节，供 artifact 输出。
    pub stdout: Vec<u8>,
    /// exit 请求的退出码（如果 case 调用过 exit）。
    pub exit_code: Option<i32>,
    /// 确定性 PRNG 状态，供 `/dev/urandom` / getrandom 使用。
    /// 用 xorshift64，保证 case 在不同 host 上结果一致。
    rng: u64,
}

impl LinuxRuntime {
    pub fn new() -> Self {
        Self {
            fds: FdTable::new(),
            next_mmap: 0x7F_0000_0000,
            brk: 0x7E_0000_0000,
            stdout: Vec::new(),
            exit_code: None,
            // 非零种子；确定性 case 默认从固定种子开始。
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// 设置 urandom 的 PRNG 种子（让 case 可复现）。
    pub fn seed_rng(&mut self, seed: u64) {
        // 0 种子会让 xorshift 退化，强制非零。
        self.rng = if seed == 0 { 0xDEAD_BEEF } else { seed };
    }

    /// 处理一次 syscall。
    ///
    /// `x0..x5` 是参数寄存器，`nr` 是 syscall 号。
    /// 返回的 [`SyscallResult`] 由 caller（backend hook）写回 x0 / 决定是否 stop。
    ///
    /// `read_guest` / `write_guest` 闭包由 backend 提供，让本层不直接持有 Unicorn 句柄。
    /// 闭包的可失败返回值是故意为之：guest 给的缓冲地址可能未映射，
    /// 必须把失败上报成 EFAULT，否则会出现"写没写成功都返回 N"的假阳性。
    pub fn dispatch(
        &mut self,
        nr: u64,
        x0: u64,
        x1: u64,
        x2: u64,
        _x3: u64,
        _x4: u64,
        _x5: u64,
        read_guest: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>,
        write_guest: &mut dyn FnMut(u64, &[u8]) -> bool,
    ) -> SyscallResult {
        match nr {
            SYS_OPENAT => {
                // x1 = path 指针；从 guest 读字符串到首个 NUL。
                let Some(path_bytes) = read_guest(x1, 256) else {
                    return SyscallResult::Done(EFAULT as u64);
                };
                let nul = path_bytes
                    .iter()
                    .position(|b| *b == 0)
                    .unwrap_or(path_bytes.len());
                let path = String::from_utf8_lossy(&path_bytes[..nul]);
                match resolve(path.as_ref()) {
                    Some(src) => {
                        let fd = self.fds.allocate(FdType::Vfs(src));
                        SyscallResult::Done(fd as u64)
                    }
                    None => SyscallResult::Done(ENOSYS as u64),
                }
            }
            SYS_CLOSE => {
                let ok = self.fds.close(x0 as Fd);
                SyscallResult::Done(if ok { 0 } else { EBADF as u64 })
            }
            SYS_READ => {
                let fd = x0 as Fd;
                let buf_addr = x1;
                let count = x2 as usize;
                let Some(bytes) = self.read_from_fd(fd, count) else {
                    return SyscallResult::Done(EBADF as u64);
                };
                if bytes.is_empty() {
                    return SyscallResult::Done(0);
                }
                if !write_guest(buf_addr, &bytes) {
                    return SyscallResult::Done(EFAULT as u64);
                }
                SyscallResult::Done(bytes.len() as u64)
            }
            SYS_WRITE => {
                let fd = x0 as Fd;
                let buf_addr = x1;
                let count = x2 as usize;
                let Some(data) = read_guest(buf_addr, count) else {
                    return SyscallResult::Done(EFAULT as u64);
                };
                let n = self.write_to_fd(fd, &data);
                SyscallResult::Done(n as u64)
            }
            SYS_EXIT | SYS_EXIT_GROUP => {
                self.exit_code = Some(x0 as i32);
                SyscallResult::Exit(x0 as i32)
            }
            SYS_BRK => SyscallResult::Done(self.brk),
            SYS_MMAP => {
                // 简化：忽略 length/flags 校验，固定返回递增地址。
                let addr = self.next_mmap;
                // 每次预借 1 MiB 对齐块，足够 smoke case。
                self.next_mmap = self.next_mmap.checked_add(0x10_0000).unwrap_or(addr);
                SyscallResult::Done(addr)
            }
            SYS_MUNMAP => SyscallResult::Done(0),
            SYS_GETRANDOM => {
                let buf_addr = x0;
                let count = x1 as usize;
                let mut buf = Vec::with_capacity(count);
                for _ in 0..count {
                    buf.push(self.next_random_byte());
                }
                if !write_guest(buf_addr, &buf) {
                    return SyscallResult::Done(EFAULT as u64);
                }
                SyscallResult::Done(count as u64)
            }
            _ => SyscallResult::Done(ENOSYS as u64),
        }
    }

    fn read_from_fd(&mut self, fd: Fd, count: usize) -> Option<Vec<u8>> {
        let ty = self.fds.get(fd)?;
        match ty {
            FdType::Vfs(VfsSource::Urandom) => {
                let mut buf = Vec::with_capacity(count);
                for _ in 0..count {
                    buf.push(self.next_random_byte());
                }
                Some(buf)
            }
            FdType::Vfs(VfsSource::Null) | FdType::Stdin => Some(Vec::new()),
            FdType::Stdout | FdType::Stderr => Some(Vec::new()),
        }
    }

    fn write_to_fd(&mut self, fd: Fd, data: &[u8]) -> usize {
        let ty = match self.fds.get(fd) {
            Some(t) => t.clone(),
            None => return 0,
        };
        match ty {
            FdType::Stdout | FdType::Stderr => {
                self.stdout.extend_from_slice(data);
                data.len()
            }
            FdType::Vfs(VfsSource::Null) => data.len(),
            _ => 0,
        }
    }

    /// xorshift64 推进一字节。
    fn next_random_byte(&mut self) -> u8 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x & 0xFF) as u8
    }
}

impl Default for LinuxRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openat_urandom_then_read() {
        let mut rt = LinuxRuntime::new();
        rt.seed_rng(42);
        let path = b"/dev/urandom\0".to_vec();
        let r = rt.dispatch(
            SYS_OPENAT,
            0,
            0x1000,
            0,
            0,
            0,
            0,
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
        );
        let fd = match r {
            SyscallResult::Done(v) => v as Fd,
            _ => panic!("expected fd"),
        };
        assert!(fd >= 3);

        // read 4 字节：write_guest 返回 true 才算成功。
        let r = rt.dispatch(
            SYS_READ,
            fd as u64,
            0x2000,
            4,
            0,
            0,
            0,
            &mut |_, _| None,
            &mut |_, _| true,
        );
        match r {
            SyscallResult::Done(n) => assert_eq!(n, 4),
            _ => panic!("expected 4 bytes"),
        }
    }

    #[test]
    fn read_returns_efault_when_write_fails() {
        // 覆盖 finding 1：write_guest 失败必须上报成 EFAULT，而不是返回字节数。
        let mut rt = LinuxRuntime::new();
        rt.seed_rng(1);
        let path = b"/dev/urandom\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, 0x1000, 0, 0, 0, 0,
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
        ) {
            SyscallResult::Done(v) => v as Fd,
            _ => panic!("expected fd"),
        };
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 4, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| false, // 模拟 guest 缓冲未映射
        );
        assert!(matches!(r, SyscallResult::Done(v) if v as i64 == EFAULT));
    }

    #[test]
    fn getrandom_returns_efault_on_unmapped_buffer() {
        let mut rt = LinuxRuntime::new();
        let r = rt.dispatch(
            SYS_GETRANDOM, 0xDEAD_BEEF, 16, 0, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| false,
        );
        assert!(matches!(r, SyscallResult::Done(v) if v as i64 == EFAULT));
    }

    #[test]
    fn unknown_syscall_returns_enosys() {
        let mut rt = LinuxRuntime::new();
        let r = rt.dispatch(
            9999, 0, 0, 0, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| true,
        );
        assert!(matches!(r, SyscallResult::Done(v) if v as i64 == ENOSYS));
    }
}
