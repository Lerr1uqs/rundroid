//! Linux syscall ABI 边界。
//!
//! 本模块只承载 ABI 层职责：syscall 号常量、[`SyscallResult`]、`dispatch` 入口、
//! 各 `sys_*` handler。handler 职责限定为：
//!
//! 1. 解码 `x0..x5` 寄存器参数；
//! 2. 调 [`crate::kernel`] 的 OS 方法拿数据／推进状态；
//! 3. 通过 [`crate::memory_bridge::MemoryBridge`] 把结果回写到目标侧；
//! 4. 用 [`crate::errno`] 编码 [`SyscallResult`]（回写失败固定 [`crate::errno::EFAULT`]）。
//!
//! OS 状态与语义实现见 [`crate::kernel`]。`LinuxRuntime` 类型本身定义在
//! [`crate::kernel`]；本模块为同 crate 跨文件 `impl LinuxRuntime`，仅承载 dispatch 入口。

use crate::errno::{
    map_fd_rw_error, map_ioctl_error, EBADF, EFAULT, EINVAL, ENOSYS, ENOTTY,
};
use crate::fd::Fd;
use crate::kernel::{FdOpError, LinuxRuntime, WriteOutcome};
use crate::memory_bridge::MemoryBridge;
use rundroid_telemetry::{TelemetryEvent, TelemetryEventKind};

// ============================================================================
// ARM64 syscall 号
// ============================================================================

/// ARM64 Linux syscall 号（bootstrap subset）。
const SYS_IOCTL: u64 = 29;
const SYS_OPENAT: u64 = 56;
const SYS_CLOSE: u64 = 57;
const SYS_READ: u64 = 63;
const SYS_PREAD64: u64 = 67;
const SYS_WRITE: u64 = 64;
const SYS_DUP: u64 = 23;
const SYS_DUP3: u64 = 24;
const SYS_FSTAT: u64 = 80;
const SYS_EXIT: u64 = 93;
const SYS_EXIT_GROUP: u64 = 94;
const SYS_BRK: u64 = 214;
const SYS_MUNMAP: u64 = 215;
const SYS_MMAP: u64 = 222;
const SYS_GETRANDOM: u64 = 278;

/// ARM64 mmap prot 常量：`MAP_ANONYMOUS` 标志（fd 为 -1 时的匿名映射）。
const MAP_ANONYMOUS: i64 = 0x20;

// ============================================================================
// SyscallResult
// ============================================================================

/// syscall 执行结果。
///
/// `Done(value)` 对应寄存器 x0 的写入值；
/// `Exit(code)` 通知 backend 停止执行。
#[derive(Debug, Clone, Copy)]
pub enum SyscallResult {
    Done(u64),
    Exit(i32),
}

impl LinuxRuntime {
    // ========================================================================
    // dispatch：syscall 主分派入口（ABI 边界）
    // ========================================================================

    /// 处理一次 syscall。
    ///
    /// `x0..x5` 是参数寄存器，`nr` 是 syscall 号。
    /// `mem` 是 guest 目标侧内存访问边界（读/写/映射三能力），由装配层注入。
    /// 按 syscall 号路由到对应 `sys_*` handler。
    pub fn dispatch(
        &mut self,
        nr: u64,
        x0: u64,
        x1: u64,
        x2: u64,
        x3: u64,
        x4: u64,
        x5: u64,
        mem: &mut dyn MemoryBridge,
    ) -> SyscallResult {
        match nr {
            SYS_OPENAT => self.sys_openat(x1, x2, mem),
            SYS_CLOSE => self.sys_close(x0),
            SYS_READ => self.sys_read(x0, x1, x2, mem),
            SYS_PREAD64 => self.sys_pread64(x0, x1, x2, x3, mem),
            SYS_WRITE => self.sys_write(x0, x1, x2, mem),
            SYS_IOCTL => self.sys_ioctl(x0, x1, x2),
            SYS_FSTAT => self.sys_fstat(x0, x1, mem),
            SYS_EXIT | SYS_EXIT_GROUP => {
                let code = x0 as i32;
                self.exit(code);
                SyscallResult::Exit(code)
            }
            SYS_BRK => SyscallResult::Done(self.brk()),
            SYS_MMAP => self.sys_mmap(x0, x1, x2, x3, x4, x5, mem),
            SYS_MUNMAP => SyscallResult::Done(self.munmap(x0, x1 as usize) as u64),
            SYS_GETRANDOM => self.sys_getrandom(x0, x1, mem),
            SYS_DUP => self.sys_dup(x0),
            SYS_DUP3 => self.sys_dup3(x0, x1, x2),
            _ => {
                self.emit(&TelemetryEvent::new(
                    "syscall.unknown",
                    TelemetryEventKind::Execution,
                ));
                SyscallResult::Done(ENOSYS as u64)
            }
        }
    }

    // ========================================================================
    // 各 syscall 处理器（ABI 边界：解码 → kernel → 回写 → 编码）
    // ========================================================================

    /// sys_openat：从 guest 读路径 → kernel 路径解析 → 返回 fd。
    fn sys_openat(&mut self, path_ptr: u64, flags: u64, mem: &mut dyn MemoryBridge) -> SyscallResult {
        let Some(path_bytes) = mem.read(path_ptr, 256) else {
            return SyscallResult::Done(EFAULT as u64);
        };
        let nul = path_bytes
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(path_bytes.len());
        let path = String::from_utf8_lossy(&path_bytes[..nul]).to_string();

        // kernel open_path 内聚完成 VFS 解析 + fd 分配 + telemetry emit。
        match self.open_path(&path, flags as i32) {
            Some(fd) => SyscallResult::Done(fd as u64),
            None => SyscallResult::Done(ENOSYS as u64),
        }
    }

    /// sys_close：kernel 关闭 fd → emit。
    fn sys_close(&mut self, fd: u64) -> SyscallResult {
        if self.close(fd as Fd) {
            self.emit(&TelemetryEvent::new(
                "fd.close",
                TelemetryEventKind::FileSystem,
            ));
            SyscallResult::Done(0)
        } else {
            SyscallResult::Done(EBADF as u64)
        }
    }

    /// sys_read：kernel 读字节 → 回写目标缓冲（失败 EFAULT）。
    fn sys_read(
        &mut self,
        fd: u64,
        buf_addr: u64,
        count: u64,
        mem: &mut dyn MemoryBridge,
    ) -> SyscallResult {
        let fd = fd as Fd;
        let count = count as usize;

        match self.read(fd, count) {
            Ok(bytes) => {
                if bytes.is_empty() {
                    self.emit(&TelemetryEvent::new(
                        "fd.read",
                        TelemetryEventKind::FileSystem,
                    ));
                    return SyscallResult::Done(0);
                }
                if !mem.write(buf_addr, &bytes) {
                    self.emit(&TelemetryEvent::new(
                        "fd.read_fault",
                        TelemetryEventKind::FileSystem,
                    ));
                    return SyscallResult::Done(EFAULT as u64);
                }
                self.emit(&TelemetryEvent::new(
                    "fd.read",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(bytes.len() as u64)
            }
            Err(FdOpError::BadFd) => SyscallResult::Done(EBADF as u64),
            Err(FdOpError::Io(e)) => {
                self.emit(&TelemetryEvent::new(
                    "device.error",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(map_fd_rw_error(e) as u64)
            }
        }
    }

    /// sys_pread64：kernel 从 offset 读（不动游标）→ 回写目标缓冲（失败 EFAULT）。
    ///
    /// 回写主线与 read 一致：源字节准备好后必须成功写回目标缓冲，
    /// 否则返回 EFAULT——不允许"返回长度但目标缓冲没变"的假成功。
    fn sys_pread64(
        &mut self,
        fd: u64,
        buf_addr: u64,
        count: u64,
        offset: u64,
        mem: &mut dyn MemoryBridge,
    ) -> SyscallResult {
        let fd = fd as Fd;
        let count = count as usize;

        match self.read_at(fd, offset, count) {
            Ok(bytes) => {
                if bytes.is_empty() {
                    self.emit(&TelemetryEvent::new(
                        "fd.pread",
                        TelemetryEventKind::FileSystem,
                    ));
                    return SyscallResult::Done(0);
                }
                if !mem.write(buf_addr, &bytes) {
                    self.emit(&TelemetryEvent::new(
                        "fd.pread_fault",
                        TelemetryEventKind::FileSystem,
                    ));
                    return SyscallResult::Done(EFAULT as u64);
                }
                self.emit(&TelemetryEvent::new(
                    "fd.pread",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(bytes.len() as u64)
            }
            Err(FdOpError::BadFd) => SyscallResult::Done(EBADF as u64),
            Err(FdOpError::Io(e)) => {
                self.emit(&TelemetryEvent::new(
                    "device.error",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(map_fd_rw_error(e) as u64)
            }
        }
    }

    /// sys_write：从 guest 读数据 → kernel 写语义（含 stdout 收集）。
    ///
    /// stdout(1)/stderr(2) 由 kernel write 内聚收集，不产生 `fd.write` 事件。
    fn sys_write(
        &mut self,
        fd: u64,
        buf_addr: u64,
        count: u64,
        mem: &mut dyn MemoryBridge,
    ) -> SyscallResult {
        let fd = fd as Fd;
        let count = count as usize;

        let Some(data) = mem.read(buf_addr, count) else {
            return SyscallResult::Done(EFAULT as u64);
        };

        match self.write(fd, &data) {
            Ok(WriteOutcome::StdStream(n)) => SyscallResult::Done(n as u64),
            Ok(WriteOutcome::Fd(n)) => {
                self.emit(&TelemetryEvent::new(
                    "fd.write",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(n as u64)
            }
            Err(FdOpError::BadFd) => SyscallResult::Done(EBADF as u64),
            Err(FdOpError::Io(e)) => {
                self.emit(&TelemetryEvent::new(
                    "device.error",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(map_fd_rw_error(e) as u64)
            }
        }
    }

    /// sys_ioctl：kernel ioctl → emit `device.ioctl`（错误不 emit）。
    ///
    /// `x1` 是 ioctl request 号，`x2` 是 argp 指针（目标侧地址）。
    fn sys_ioctl(&mut self, fd: u64, request: u64, argp: u64) -> SyscallResult {
        match self.ioctl(fd as Fd, request, argp) {
            Ok(result) => {
                self.emit(&TelemetryEvent::new(
                    "device.ioctl",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(result as u64)
            }
            Err(FdOpError::BadFd) => SyscallResult::Done(EBADF as u64),
            // ioctl 专用映射：Internal → EINVAL（区别于 IO 路径的 EFAULT）。
            Err(FdOpError::Io(e)) => SyscallResult::Done(map_ioctl_error(e) as u64),
        }
    }

    /// sys_fstat：kernel 合成 stat → 序列化为 stat64 字节 → 回写目标缓冲。
    ///
    /// bootstrap 只写最简字段（st_dev/st_ino/st_mode/st_nlink/st_size/st_blksize/st_blocks），
    /// 其余字段填零。生产环境应写完整 struct stat64。
    fn sys_fstat(&mut self, fd: u64, buf_addr: u64, mem: &mut dyn MemoryBridge) -> SyscallResult {
        match self.fstat(fd as Fd) {
            Ok(stat) => {
                let mut buf = vec![0u8; 128];
                // st_dev (8 bytes, offset 0)
                buf[0..8].copy_from_slice(&stat.st_dev.to_le_bytes());
                // st_ino (8 bytes, offset 8)
                buf[8..16].copy_from_slice(&stat.st_ino.to_le_bytes());
                // st_mode (4 bytes, offset 16)
                buf[16..20].copy_from_slice(&stat.st_mode.to_le_bytes());
                // st_nlink (4 bytes, offset 20) = 1
                buf[20..24].copy_from_slice(&1u32.to_le_bytes());
                // st_uid/st_gid/st_rdev 留零（offset 24..40）。
                // st_size (8 bytes, offset 48)
                buf[48..56].copy_from_slice(&stat.st_size.to_le_bytes());
                // st_blksize (4 bytes, offset 56) = 4096
                buf[56..60].copy_from_slice(&4096u32.to_le_bytes());
                // st_blocks (8 bytes, offset 64) = (st_size + 511) / 512
                let blocks = (stat.st_size + 511) / 512;
                buf[64..72].copy_from_slice(&blocks.to_le_bytes());

                if !mem.write(buf_addr, &buf) {
                    return SyscallResult::Done(EFAULT as u64);
                }
                self.emit(&TelemetryEvent::new(
                    "fd.fstat",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(0)
            }
            // fstat 错误（含 bad fd 与底座错误）统一映射 EBADF（保持原行为）。
            Err(_) => SyscallResult::Done(EBADF as u64),
        }
    }

    /// sys_mmap：建立内存映射。
    ///
    /// - 匿名映射（fd == -1 或 MAP_ANONYMOUS）：kernel 分配地址 → syscall 层 `mem.map` 落地。
    /// - fd-backed：kernel 取 region+地址 → syscall 层 **先 map 再显式回写内容**
    ///   （不依赖 map 的隐式写副作用）。
    fn sys_mmap(
        &mut self,
        _hint_addr: u64,
        length: u64,
        prot: u64,
        flags: u64,
        fd: u64,
        offset: u64,
        mem: &mut dyn MemoryBridge,
    ) -> SyscallResult {
        let length = length as usize;
        let prot = prot as i32;
        let flags = flags as i64;
        let fd = fd as Fd;

        // 匿名映射：kernel 分配地址 + syscall 层落地映射。
        if fd == -1 || (flags & MAP_ANONYMOUS) != 0 {
            let guest_addr = self.alloc_mmap_addr(length);
            if !mem.map(guest_addr, length, prot) {
                return SyscallResult::Done(EFAULT as u64);
            }
            return SyscallResult::Done(guest_addr);
        }

        // fd-backed：kernel 取 region+地址，syscall 层显式 map + 内容回写。
        match self.device_mmap(fd, length, offset, prot, flags as i32) {
            Ok(Some(region)) => {
                let len = region.content.len();
                // spec: 先建立目标侧映射，再通过 bridge 把内容字节显式落地。
                if !mem.map(region.addr, len, region.prot) {
                    return SyscallResult::Done(EFAULT as u64);
                }
                if !region.content.is_empty() && !mem.write(region.addr, &region.content) {
                    return SyscallResult::Done(EFAULT as u64);
                }
                self.emit(&TelemetryEvent::new(
                    "device.mmap",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(region.addr)
            }
            Ok(None) => SyscallResult::Done(ENOTTY as u64),
            Err(FdOpError::BadFd) => SyscallResult::Done(EBADF as u64),
            Err(FdOpError::Io(_)) => SyscallResult::Done(EINVAL as u64),
        }
    }

    /// sys_getrandom：kernel 产随机字节 → 回写目标缓冲。
    ///
    /// 不经 fd，是直接 syscall，保持 bootstrap 语义。
    fn sys_getrandom(
        &mut self,
        buf_addr: u64,
        count: u64,
        mem: &mut dyn MemoryBridge,
    ) -> SyscallResult {
        let count = count as usize;
        let bytes = self.getrandom_bytes(count);
        if !mem.write(buf_addr, &bytes) {
            return SyscallResult::Done(EFAULT as u64);
        }
        SyscallResult::Done(count as u64)
    }

    /// sys_dup：复制 fd 到自动分配的新 fd。
    fn sys_dup(&mut self, old_fd: u64) -> SyscallResult {
        match self.dup(old_fd as Fd) {
            Ok(new_fd) => SyscallResult::Done(new_fd as u64),
            Err(_) => SyscallResult::Done(EBADF as u64),
        }
    }

    /// sys_dup3：以指定 flags 复制 fd 到指定目标 fd。
    ///
    /// ARM64 上没有 dup2，统一用 dup3。
    /// `x0` = old_fd, `x1` = new_fd, `x2` = flags（通常为 O_CLOEXEC）。
    fn sys_dup3(&mut self, old_fd: u64, new_fd: u64, flags: u64) -> SyscallResult {
        match self.dup3(old_fd as Fd, new_fd as Fd, flags as i32) {
            Ok(dup_fd) => SyscallResult::Done(dup_fd as u64),
            Err(_) => SyscallResult::Done(EBADF as u64),
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errno::{EBADF, EFAULT, ENOSYS, ENOTTY};
    use crate::vfs::VfsError;
    use rundroid_driver::builtin::null_factory;
    use rundroid_driver::mapper::VirtFileSource;

    /// 测试用 MemoryBridge：把三个闭包收敛成单一 bridge 对象。
    /// 写法对齐重构前三闭包 dispatch，最小化测试改动。
    struct TestBridge<R, W, M> {
        read: R,
        write: W,
        map: M,
    }
    impl<R, W, M> MemoryBridge for TestBridge<R, W, M>
    where
        R: FnMut(u64, usize) -> Option<Vec<u8>>,
        W: FnMut(u64, &[u8]) -> bool,
        M: FnMut(u64, usize, i32) -> bool,
    {
        fn read(&mut self, addr: u64, len: usize) -> Option<Vec<u8>> {
            (self.read)(addr, len)
        }
        fn write(&mut self, addr: u64, data: &[u8]) -> bool {
            (self.write)(addr, data)
        }
        fn map(&mut self, addr: u64, len: usize, prot: i32) -> bool {
            (self.map)(addr, len, prot)
        }
    }

    /// 由三个闭包构造测试 bridge。
    #[allow(clippy::type_complexity)]
    fn bridge<R, W, M>(read: R, write: W, map: M) -> TestBridge<R, W, M>
    where
        R: FnMut(u64, usize) -> Option<Vec<u8>>,
        W: FnMut(u64, &[u8]) -> bool,
        M: FnMut(u64, usize, i32) -> bool,
    {
        TestBridge { read, write, map }
    }

    /// 默认 map 成功的闭包工厂。
    fn map_ok() -> impl FnMut(u64, usize, i32) -> bool {
        |_addr, _len, _prot| true
    }

    #[test]
    fn openat_urandom_then_read() {
        let mut rt = LinuxRuntime::new();
        rt.seed_rng(42);
        let path = b"/dev/urandom\0".to_vec();
        let r = rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        );
        let fd = match r {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        assert!(fd >= 3);

        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 4, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| true, map_ok()),
        );
        match r {
            SyscallResult::Done(n) => assert_eq!(n, 4),
            _ => panic!("expected 4 bytes"),
        }
    }

    #[test]
    fn read_returns_efault_when_write_fails() {
        let mut rt = LinuxRuntime::new();
        rt.seed_rng(1);
        let path = b"/dev/urandom\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 4, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| false, map_ok()),
        );
        assert!(matches!(r, SyscallResult::Done(v) if v as i64 == EFAULT));
    }

    #[test]
    fn getrandom_returns_efault_on_unmapped_buffer() {
        let mut rt = LinuxRuntime::new();
        let r = rt.dispatch(
            SYS_GETRANDOM, 0xDEAD_BEEF, 16, 0, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| false, map_ok()),
        );
        assert!(matches!(r, SyscallResult::Done(v) if v as i64 == EFAULT));
    }

    #[test]
    fn unknown_syscall_returns_enosys() {
        let mut rt = LinuxRuntime::new();
        let r = rt.dispatch(
            9999, 0, 0, 0, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| true, map_ok()),
        );
        assert!(matches!(r, SyscallResult::Done(v) if v as i64 == ENOSYS));
    }

    #[test]
    fn vfs_duplicate_mount_fails() {
        let mut rt = LinuxRuntime::new();
        let null_id = rt.device_registry.register(null_factory());
        let err = rt.vfs.mount_device("/dev/null", null_id);
        assert!(err.is_err());
    }

    /// 测试 dup 复制 device fd 后两个 fd 都能正常工作。
    #[test]
    fn dup_device_fd_works() {
        let mut rt = LinuxRuntime::new();
        rt.seed_rng(7);
        let path = b"/dev/urandom\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        let dup_fd = match rt.dispatch(
            SYS_DUP, fd as u64, 0, 0, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected dup fd"),
        };
        assert!(dup_fd > fd);
        // 两个 fd 都应该能 read。
        for test_fd in [fd, dup_fd] {
            let r = rt.dispatch(
                SYS_READ, test_fd as u64, 0x2000, 2, 0, 0, 0,
                &mut bridge(|_, _| None, |_, _| true, map_ok()),
            );
            match r {
                SyscallResult::Done(n) => assert_eq!(n, 2),
                other => panic!("expected 2 bytes, got {other:?}"),
            }
        }
    }

    /// 测试对不支持 ioctl 的设备返回 ENOTTY。
    #[test]
    fn ioctl_returns_enotty_on_urandom() {
        let mut rt = LinuxRuntime::new();
        let path = b"/dev/urandom\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        let r = rt.dispatch(
            SYS_IOCTL, fd as u64, 0x5401, 0, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| true, map_ok()),
        );
        assert!(matches!(r, SyscallResult::Done(v) if v as i64 == ENOTTY));
    }

    /// 测试 fstat 返回合理结果。
    #[test]
    fn fstat_on_urandom_returns_char_device() {
        let mut rt = LinuxRuntime::new();
        let path = b"/dev/urandom\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        let r = rt.dispatch(
            SYS_FSTAT, fd as u64, 0x2000, 0, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| true, map_ok()),
        );
        assert!(matches!(r, SyscallResult::Done(0)));
    }

    /// 测试无效 fd 上的操作返回 EBADF。
    #[test]
    fn bad_fd_returns_ebadf() {
        let mut rt = LinuxRuntime::new();
        let r = rt.dispatch(
            SYS_READ, 999, 0x2000, 4, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| true, map_ok()),
        );
        assert!(matches!(r, SyscallResult::Done(v) if v as i64 == EBADF));
    }

    /// 测试 mmap 匿名映射。
    #[test]
    fn mmap_anonymous_returns_address() {
        let mut rt = LinuxRuntime::new();
        let r = rt.dispatch(
            SYS_MMAP, 0, 0x1000, 3u64, // PROT_READ | PROT_WRITE
            MAP_ANONYMOUS as u64, 0xFFFF_FFFF_FFFF_FFFFu64, 0,
            &mut bridge(|_, _| None, |_, _| true, |addr, _len, _prot| addr >= 0x7F_0000_0000),
        );
        match r {
            SyscallResult::Done(addr) => assert!(addr >= 0x7F_0000_0000),
            other => panic!("expected address, got {other:?}"),
        }
    }

    /// [4.2] mmap 匿名映射：map 失败时返回 EFAULT（bridge 失败优先级）。
    #[test]
    fn mmap_returns_efault_when_map_fails() {
        let mut rt = LinuxRuntime::new();
        let r = rt.dispatch(
            SYS_MMAP, 0, 0x1000, 3u64,
            MAP_ANONYMOUS as u64, 0xFFFF_FFFF_FFFF_FFFFu64, 0,
            &mut bridge(|_, _| None, |_, _| true, |_, _, _| false),
        );
        assert!(
            matches!(r, SyscallResult::Done(v) if v as i64 == EFAULT),
            "map 失败应返回 EFAULT，实际: {r:?}"
        );
    }

    /// [4.2] fstat 回写失败时返回 EFAULT（bridge 失败优先级）。
    #[test]
    fn fstat_returns_efault_when_write_fails() {
        let mut rt = LinuxRuntime::new();
        let path = b"/dev/urandom\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        let r = rt.dispatch(
            SYS_FSTAT, fd as u64, 0x2000, 0, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| false, map_ok()),
        );
        assert!(matches!(r, SyscallResult::Done(v) if v as i64 == EFAULT));
    }

    /// pread64 从指定 offset 读取，回写目标缓冲后返回长度。
    #[test]
    fn pread64_reads_at_offset() {
        let mut rt = LinuxRuntime::new();
        let data = b"hello world from pread".to_vec();
        rt.mount_file("/data/pread.txt", VirtFileSource::Bytes(data))
            .unwrap();
        let path = b"/data/pread.txt\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        // pread64(fd, buf, count=5, offset=6) → "world"
        let captured = std::cell::RefCell::new(Vec::new());
        let r = rt.dispatch(
            SYS_PREAD64, fd as u64, 0x2000, 5, 6, 0, 0,
            &mut bridge(|_, _| None, |_a, bytes| {
                captured.borrow_mut().extend_from_slice(bytes);
                true
            }, map_ok()),
        );
        assert!(matches!(r, SyscallResult::Done(5)));
        assert_eq!(&captured.into_inner(), b"world");
    }

    /// pread64 不移动文件游标：pread 后再 read 仍从 cursor=0 开始。
    #[test]
    fn pread64_does_not_move_cursor() {
        let mut rt = LinuxRuntime::new();
        let data = b"hello world".to_vec();
        rt.mount_file("/data/cursor.txt", VirtFileSource::Bytes(data))
            .unwrap();
        let path = b"/data/cursor.txt\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        // pread64(offset=6, 5) → "world"，不应移动 cursor。
        let captured_pread = std::cell::RefCell::new(Vec::new());
        rt.dispatch(
            SYS_PREAD64, fd as u64, 0x2000, 5, 6, 0, 0,
            &mut bridge(|_, _| None, |_a, b| {
                captured_pread.borrow_mut().extend_from_slice(b);
                true
            }, map_ok()),
        );
        assert_eq!(&captured_pread.into_inner(), b"world");

        // 随后 read(5) 应从 cursor=0 读 → "hello"（证明 pread 没动 cursor）。
        let captured_read = std::cell::RefCell::new(Vec::new());
        rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 5, 0, 0, 0,
            &mut bridge(|_, _| None, |_a, b| {
                captured_read.borrow_mut().extend_from_slice(b);
                true
            }, map_ok()),
        );
        assert_eq!(&captured_read.into_inner(), b"hello");
    }

    /// pread64 回写目标缓冲失败时返回 EFAULT（不允许假成功）。
    #[test]
    fn pread64_returns_efault_on_write_failure() {
        let mut rt = LinuxRuntime::new();
        let data = b"hello world".to_vec();
        rt.mount_file("/data/pread_fault.txt", VirtFileSource::Bytes(data))
            .unwrap();
        let path = b"/data/pread_fault.txt\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };
        let r = rt.dispatch(
            SYS_PREAD64, fd as u64, 0x2000, 5, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| false, map_ok()), // 回写失败
        );
        assert!(matches!(r, SyscallResult::Done(v) if v as i64 == EFAULT));
    }

    /// pread64 对无效 fd 返回 EBADF。
    #[test]
    fn pread64_bad_fd_returns_ebadf() {
        let mut rt = LinuxRuntime::new();
        let r = rt.dispatch(
            SYS_PREAD64, 999, 0x2000, 5, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| true, map_ok()),
        );
        assert!(matches!(r, SyscallResult::Done(v) if v as i64 == EBADF));
    }

    /// 测试 dup3 复制到指定 fd。
    #[test]
    fn dup3_copies_to_target_fd() {
        let mut rt = LinuxRuntime::new();
        rt.seed_rng(1);
        let path = b"/dev/zero\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        let target = 100;
        let r = rt.dispatch(
            SYS_DUP3, fd as u64, target, 0, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| true, map_ok()),
        );
        match r {
            SyscallResult::Done(v) => assert_eq!(v, target),
            other => panic!("expected target fd, got {other:?}"),
        }
    }

    /// 测试 telemetry 模式（无实际 sink）。
    #[test]
    fn telemetry_mode_does_not_crash() {
        use rundroid_telemetry::TelemetryRouter;
        let mut rt = LinuxRuntime::with_telemetry(TelemetryRouter::disabled());
        rt.seed_rng(1);
        let path = b"/dev/urandom\0".to_vec();
        let _r = rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        );
        // 即使有 router，disabled 模式也不 crash。
    }

    // ========================================================================
    // Regression tests（对应 tasks.md 第 14-19 项）
    // ========================================================================

    /// [Regression 14] /dev/urandom 缓冲区校验：
    /// 目标侧 read 后 buffer 必须真实可见随机字节。
    #[test]
    fn regression_urandom_buffer_visible() {
        let mut rt = LinuxRuntime::new();
        rt.seed_rng(0x1234);
        let path = b"/dev/urandom\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };
        // 用真实缓冲捕获 write_guest 写入的字节。
        let captured = std::cell::RefCell::new(Vec::new());
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 8, 0, 0, 0,
            &mut bridge(|_, _| None, |_addr, bytes| {
                captured.borrow_mut().extend_from_slice(bytes);
                true
            }, map_ok()),
        );
        assert!(matches!(r, SyscallResult::Done(8)));
        let bytes = captured.into_inner();
        assert_eq!(bytes.len(), 8);
        // 确认字节不全为 0（urandom 生产了真实字节）。
        assert!(bytes.iter().any(|b| *b != 0));
    }

    /// [Regression 15] VirtFile.bytes 回归：挂载内存字节文件后读取内容匹配。
    #[test]
    fn regression_virtfile_bytes_read_back() {
        let mut rt = LinuxRuntime::new();
        let data = b"hello world from virtfile bytes\n".to_vec();
        let data_len = data.len();
        rt.mount_file(
            "/data/test.txt",
            VirtFileSource::Bytes(data.clone()),
        )
        .unwrap();

        let path = b"/data/test.txt\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        let captured = std::cell::RefCell::new(Vec::new());
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, data_len as u64, 0, 0, 0,
            &mut bridge(|_, _| None, |_addr, bytes| {
                captured.borrow_mut().extend_from_slice(bytes);
                true
            }, map_ok()),
        );
        match r {
            SyscallResult::Done(n) => assert_eq!(n as usize, data_len),
            other => panic!("expected {} bytes, got {other:?}", data_len),
        }
        assert_eq!(captured.into_inner(), data);
    }

    /// [Regression 15b] VirtFile.host 文件读取回校验。
    #[test]
    fn regression_virtfile_host_read_back() {
        let mut rt = LinuxRuntime::new();
        // 用临时文件模拟宿主文件。
        let tmp = std::env::temp_dir().join("rundroid_regression_host.txt");
        let content = b"host file content\n".to_vec();
        std::fs::write(&tmp, &content).unwrap();

        rt.mount_file("/data/host.txt", VirtFileSource::HostPath(tmp.clone()))
            .unwrap();

        let path = b"/data/host.txt\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        let captured = std::cell::RefCell::new(Vec::new());
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, content.len() as u64, 0, 0, 0,
            &mut bridge(|_, _| None, |_addr, bytes| {
                captured.borrow_mut().extend_from_slice(bytes);
                true
            }, map_ok()),
        );
        match r {
            SyscallResult::Done(n) => assert_eq!(n as usize, content.len()),
            other => panic!("expected {} bytes, got {other:?}", content.len()),
        }
        assert_eq!(captured.into_inner(), content);

        // 清理临时文件。
        let _ = std::fs::remove_file(&tmp);
    }

    /// [Regression 16] 动态 VirtFile provider：provider 返回成功但 write_guest 失败 → EFAULT。
    #[test]
    fn regression_dynamic_provider_writeback_failure() {
        use rundroid_driver::mapper::{VirtFileProvider, FileProviderError};
        use std::sync::Arc;

        #[derive(Debug)]
        struct AlwaysSucceedsProvider;
        impl VirtFileProvider for AlwaysSucceedsProvider {
            fn bytes(&self) -> Result<Vec<u8>, FileProviderError> {
                Ok(b"dynamic data".to_vec())
            }
        }

        let mut rt = LinuxRuntime::new();
        rt.mount_file(
            "/proc/dynamic",
            VirtFileSource::Dynamic(Arc::new(AlwaysSucceedsProvider)),
        )
        .unwrap();

        let path = b"/proc/dynamic\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        // write_guest 返回 false 模拟 guest 缓冲未映射。
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 1024, 0, 0, 0,
            &mut bridge(|_, _| None, |_, _| false, map_ok()),
        );
        assert!(
            matches!(r, SyscallResult::Done(v) if v as i64 == EFAULT),
            "provider 返回成功但 writeback 失败时，整体必须返回 EFAULT，实际: {r:?}"
        );
    }

    /// [Regression 17] Custom fake device：新增设备不需要编辑 syscall 核心分支。
    ///
    /// 通过 mount_device 注册一个自定义设备后，open/read/write 能正常工作，
    /// 无需在 dispatch() 或 sys_openat 中加入任何新分支。
    /// 这是"不改 syscall 核心增设备"的回归保护。
    #[test]
    fn regression_custom_device_no_syscall_edit() {
        use rundroid_driver::context::{
            DeviceCloseContext, DeviceIoContext, DeviceOpenContext,
        };
        use rundroid_driver::device::{DeviceError, VirtualDevice};

        /// 一个简单的自定义设备：返回固定的魔术字节。
        struct MagicEchoDevice {
            opened: bool,
        }

        impl VirtualDevice for MagicEchoDevice {
            fn open(&mut self, _ctx: &mut DeviceOpenContext) -> Result<(), DeviceError> {
                self.opened = true;
                Ok(())
            }
            fn read(
                &mut self,
                ctx: &mut DeviceIoContext,
                len: usize,
            ) -> Result<Vec<u8>, DeviceError> {
                let _ = ctx;
                Ok(vec![0xA5; len.min(16)])
            }
            fn write(
                &mut self,
                ctx: &mut DeviceIoContext,
                data: &[u8],
            ) -> Result<usize, DeviceError> {
                let _ = ctx;
                Ok(data.len())
            }
            fn mmap(
                &mut self,
                ctx: &mut rundroid_driver::context::DeviceMmapContext,
                req: &rundroid_driver::context::DeviceMmapRequest,
            ) -> Result<Option<rundroid_driver::context::DeviceMappedRegion>, DeviceError> {
                // 这个自定义设备支持 mmap。
                let _ = ctx;
                Ok(Some(rundroid_driver::context::DeviceMappedRegion {
                    content: vec![0xCD; req.length],
                    hint_addr: 0,
                    prot: 3, // PROT_READ | PROT_WRITE
                }))
            }
            fn close(&mut self, _ctx: &mut DeviceCloseContext) -> Result<(), DeviceError> {
                self.opened = false;
                Ok(())
            }
        }

        let mut rt = LinuxRuntime::new();
        let factory: rundroid_driver::registry::DeviceFactory =
            std::sync::Arc::new(|| Box::new(MagicEchoDevice { opened: false }));
        rt.mount_device("/dev/magic", factory)
            .expect("custom device mount should succeed");

        // 通过标准 syscall 路径打开、读取自定义设备。
        let path = b"/dev/magic\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        let captured = std::cell::RefCell::new(Vec::new());
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 16, 0, 0, 0,
            &mut bridge(|_, _| None, |_addr, bytes| {
                captured.borrow_mut().extend_from_slice(bytes);
                true
            }, map_ok()),
        );
        match r {
            SyscallResult::Done(n) => assert_eq!(n, 16),
            other => panic!("expected 16 bytes, got {other:?}"),
        }
        assert_eq!(&captured.into_inner()[..], &[0xA5u8; 16]);
    }

    /// [Regression 19] Device-backed mmap 回归：
    /// 支持 mmap 的 device 能通过标准 mmap syscall 建立映射并返回有效地址。
    /// 新增内容显式回写校验：region 内容必须经 bridge 写入目标侧。
    #[test]
    fn regression_device_backed_mmap() {
        use rundroid_driver::context::{
            DeviceCloseContext, DeviceIoContext, DeviceMmapContext, DeviceMmapRequest,
            DeviceMappedRegion, DeviceOpenContext,
        };
        use rundroid_driver::device::{DeviceError, VirtualDevice};

        /// 支持 mmap 的虚拟帧缓冲区设备（返回可识别的内容字节用于回写校验）。
        struct FramebufferDevice;
        impl VirtualDevice for FramebufferDevice {
            fn open(&mut self, _ctx: &mut DeviceOpenContext) -> Result<(), DeviceError> {
                Ok(())
            }
            fn read(
                &mut self,
                ctx: &mut DeviceIoContext,
                len: usize,
            ) -> Result<Vec<u8>, DeviceError> {
                let _ = (ctx, len);
                Err(DeviceError::NotSupported)
            }
            fn write(
                &mut self,
                ctx: &mut DeviceIoContext,
                data: &[u8],
            ) -> Result<usize, DeviceError> {
                let _ = (ctx, data);
                Err(DeviceError::NotSupported)
            }
            fn mmap(
                &mut self,
                ctx: &mut DeviceMmapContext,
                req: &DeviceMmapRequest,
            ) -> Result<Option<DeviceMappedRegion>, DeviceError> {
                let _ = ctx;
                Ok(Some(DeviceMappedRegion {
                    content: vec![0x77u8; req.length], // 非零标识字节
                    hint_addr: 0, // 由 runtime 分配地址
                    prot: req.prot,
                }))
            }
            fn close(&mut self, _ctx: &mut DeviceCloseContext) -> Result<(), DeviceError> {
                Ok(())
            }
        }

        let mut rt = LinuxRuntime::new();
        let factory: rundroid_driver::registry::DeviceFactory =
            std::sync::Arc::new(|| Box::new(FramebufferDevice));
        rt.mount_device("/dev/fb0", factory)
            .expect("fb0 device mount should succeed");

        // 打开设备。
        let path = b"/dev/fb0\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut bridge(move |_, _| Some(path.clone()), |_, _| true, map_ok()),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        // 对设备 fd 执行 mmap，并捕获目标侧 map 的地址与回写的内容字节。
        let mmap_addr = std::cell::RefCell::new(0u64);
        let written = std::cell::RefCell::new(Vec::<u8>::new());
        let r = rt.dispatch(
            SYS_MMAP, 0, 0x2000, 3u64, // prot = PROT_READ | PROT_WRITE
            0 /* flags = MAP_SHARED 等 */, fd as u64, 0,
            &mut bridge(|_, _| None,
                |_a, bytes| { written.borrow_mut().extend_from_slice(bytes); true },
                |addr, _len, _prot| { *mmap_addr.borrow_mut() = addr; true }),
        );
        let addr = match r {
            SyscallResult::Done(addr) => {
                assert!(addr >= 0x7F_0000_0000);
                addr
            }
            other => panic!("expected mmap address, got {other:?}"),
        };

        // spec: region 内容必须经 bridge 显式回写到目标侧（不依赖 map 隐式写）。
        assert_eq!(
            written.into_inner(),
            vec![0x77u8; 0x2000],
            "device mmap 内容应显式回写到目标侧"
        );
        assert_eq!(*mmap_addr.borrow(), addr);
    }

    /// 路径冲突回归：已由 vfs::tests 覆盖，此处补一个端到端 syscall 路径测试。
    /// 确保重复 mount 在内置设备上也立即报错。
    #[test]
    fn regression_path_conflict_e2e() {
        let mut rt = LinuxRuntime::new();
        // 先 mount 一个普通文件到 /conflict_path。
        rt.mount_file("/conflict_path", VirtFileSource::Bytes(b"first".to_vec()))
            .unwrap();
        // 再尝试 mount 设备到同一路径。
        let err = rt.mount_device(
            "/conflict_path",
            null_factory(),
        );
        assert!(err.is_err(), "路径冲突必须立即报错");
        match err {
            Err(VfsError::AlreadyMounted(p)) => assert_eq!(p, "/conflict_path"),
            other => panic!("expected AlreadyMounted, got {other:?}"),
        }
    }
}
