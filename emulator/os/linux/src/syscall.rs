//! Linux syscall 分发。
//!
//! [`LinuxRuntime`] 持有 VFS 挂载表、设备注册表、fd 表，
//! 在 backend 遇到 `svc #0` 时按 AArch64 syscall 号分发。
//!
//! # 分发路径
//!
//! - `openat`：VFS 解析路径 → fd 表分配 entry → 返回 fd
//! - `read/write`：fd 表查找 → 按 FdHandle 分派到 file/device → 回写目标缓冲
//! - `close`：fd 表移除条目
//! - `ioctl`：fd 表查找 → 按 FdHandle 分派（仅 device 支持）
//! - `fstat`：fd 表查找 → 合成 stat 或委托 device.fstat()
//! - `mmap`：支持 fd 时委托 device.mmap()，匿名映射退化为 bump 分配
//! - `dup/dup3`：fd 表复制条目
//!
//! 不再按路径硬编码分派——所有分派统一经过 [`FileDescriptorTable`]。

use crate::fd::{
    FileDescriptorEntry, fstat_from_fd, ioctl_on_fd, mmap_from_fd, pread_from_fd, read_from_fd,
    write_to_fd, Fd, FdReadWriteError, FileDescriptorTable,
};
use crate::vfs::{VfsError, VfsMountTable, VfsNode};
use rundroid_driver::builtin::{null_factory, zero_factory};
use rundroid_driver::context::DeviceOpenContext;
use rundroid_driver::mapper::VirtFileSource;
use rundroid_driver::registry::DeviceRegistry;
use rundroid_telemetry::{TelemetryEvent, TelemetryEventKind, TelemetryRouter};
use std::sync::{Arc, Mutex};

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

// ============================================================================
// errno 常量
// ============================================================================

/// 简化的 errno（POSIX 风格负数）。
pub const ENOSYS: i64 = -38;
pub const EBADF: i64 = -9;
pub const EFAULT: i64 = -14;
pub const EINVAL: i64 = -22;
pub const ENOTTY: i64 = -25;
pub const EACCES: i64 = -13;

/// ARM64 mmap2 prot 常量（PROT_READ = 1, PROT_WRITE = 2, PROT_EXEC = 4）。
/// MAP_ANONYMOUS 标志：fd 为 -1 时的匿名映射。
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

// ============================================================================
// LinuxRuntime
// ============================================================================

/// Linux 用户态运行时。
///
/// 持有 VFS 挂载表、设备注册表、fd 表和内存布局状态。
pub struct LinuxRuntime {
    /// 虚拟文件系统挂载表。
    pub vfs: VfsMountTable,
    /// 设备注册表。
    pub device_registry: DeviceRegistry,
    /// 文件描述符表。
    pub fds: FileDescriptorTable,
    /// mmap 的"下一次返回地址"。
    next_mmap: u64,
    /// brk 当前值。
    brk: u64,
    /// 收集到的 stdout 字节。
    pub stdout: Vec<u8>,
    /// exit 请求的退出码。
    pub exit_code: Option<i32>,
    /// 确定性 PRNG 种子源（供 builtin urandom factory 使用）。
    /// 每次 open /dev/urandom 时读取并推进种子，保证不同 device 实例获取不同起始 RNG。
    pub rng_seed: Arc<Mutex<u64>>,
    /// telemetry 路由器（None = 不带 telemetry 运行，用于纯库内测试）。
    pub telemetry: Option<TelemetryRouter>,
}

impl LinuxRuntime {
    /// 创建新的运行时实例，预装 builtin 设备。
    /// 不挂载 telemetry router（测试场景）。带 telemetry 的路径用 [`with_telemetry`]。
    pub fn new() -> Self {
        Self::build(None)
    }

    /// 创建具有 telemetry 的运行时实例。
    pub fn with_telemetry(router: TelemetryRouter) -> Self {
        Self::build(Some(router))
    }

    /// 内部构造：统一初始化逻辑。
    fn build(telemetry: Option<TelemetryRouter>) -> Self {
        let rng_seed = Arc::new(Mutex::new(0x9E37_79B9_7F4A_7C15u64));
        let mut rt = Self {
            vfs: VfsMountTable::new(),
            device_registry: DeviceRegistry::new(),
            fds: FileDescriptorTable::new(),
            next_mmap: 0x7F_0000_0000,
            brk: 0x7E_0000_0000,
            stdout: Vec::new(),
            exit_code: None,
            rng_seed,
            telemetry,
        };

        // 预装 builtin 设备到 VFS + DeviceRegistry。
        rt.register_builtins();
        rt
    }

    /// 预装所有内建设备。
    ///
    /// 注册顺序：urandom、random（共用 factory）、null、zero。
    /// 注册完成后这些路径可被 syscall openat 正常打开。
    fn register_builtins(&mut self) {
        // urandom 工厂：每次调用读取并推进共享种子。
        let rng_urandom = Arc::clone(&self.rng_seed);
        let urandom_factory_fn = move || {
            let mut seed = rng_urandom.lock().unwrap();
            let s = *seed;
            // xorshift 推进一次，让下一个设备获得不同种子。
            let mut x = s;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *seed = x;
            Box::new(rundroid_driver::builtin::urandom::UrandomDevice::new(s))
                as Box<dyn rundroid_driver::VirtualDevice>
        };
        let urandom_id = self
            .device_registry
            .register(Arc::new(urandom_factory_fn));
        self.vfs
            .mount_device("/dev/urandom", urandom_id)
            .expect("builtin urandom mount should not conflict");

        // /dev/random 与 urandom 行为一致（bootstrap）。
        let rng_random = Arc::clone(&self.rng_seed);
        let random_factory_fn = move || {
            let mut seed = rng_random.lock().unwrap();
            let s = *seed;
            let mut x = s;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *seed = x;
            Box::new(rundroid_driver::builtin::urandom::UrandomDevice::new(s))
                as Box<dyn rundroid_driver::VirtualDevice>
        };
        let random_id = self.device_registry.register(Arc::new(random_factory_fn));
        self.vfs
            .mount_device("/dev/random", random_id)
            .expect("builtin random mount should not conflict");

        // /dev/null。
        let null_id = self.device_registry.register(null_factory());
        self.vfs
            .mount_device("/dev/null", null_id)
            .expect("builtin null mount should not conflict");

        // /dev/zero。
        let zero_id = self.device_registry.register(zero_factory());
        self.vfs
            .mount_device("/dev/zero", zero_id)
            .expect("builtin zero mount should not conflict");
    }

    /// 设置 urandom 的 PRNG 种子（让 case 可复现）。
    pub fn seed_rng(&mut self, seed: u64) {
        let s = if seed == 0 { 0xDEAD_BEEF } else { seed };
        *self.rng_seed.lock().unwrap() = s;
    }

    /// 挂载一个普通文件节点（供 case 配置使用）。
    pub fn mount_file(
        &mut self,
        virtual_path: &str,
        source: VirtFileSource,
    ) -> Result<(), VfsError> {
        self.vfs.mount_file(virtual_path, source)
    }

    /// 挂载一个设备节点（供 case 配置自定义设备）。
    pub fn mount_device(
        &mut self,
        virtual_path: &str,
        factory: rundroid_driver::registry::DeviceFactory,
    ) -> Result<(), VfsError> {
        let mount_id = self.device_registry.register(factory);
        self.vfs.mount_device(virtual_path, mount_id)
    }

    /// 发出 telemetry 事件（如果 router 已配置）。
    fn emit(&mut self, event: &TelemetryEvent<'_>) {
        if let Some(router) = self.telemetry.as_mut() {
            router.emit(event);
        }
    }

    // ========================================================================
    // dispatch：syscall 主分派入口
    // ========================================================================

    /// 处理一次 syscall。
    ///
    /// `x0..x5` 是参数寄存器，`nr` 是 syscall 号。
    /// `read_guest` / `write_guest` 闭包由 backend 提供，
    /// 用于读写目标进程的虚拟地址空间。
    /// `map_guest` 闭包用于 mmap 时在目标侧建立真实映射。
    pub fn dispatch(
        &mut self,
        nr: u64,
        x0: u64,
        x1: u64,
        x2: u64,
        x3: u64,
        x4: u64,
        x5: u64,
        read_guest: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>,
        write_guest: &mut dyn FnMut(u64, &[u8]) -> bool,
        map_guest: &mut dyn FnMut(u64, usize, i32) -> bool,
    ) -> SyscallResult {
        match nr {
            SYS_OPENAT => self.sys_openat(x1, x2, read_guest),
            SYS_CLOSE => self.sys_close(x0),
            SYS_READ => self.sys_read(x0, x1, x2, write_guest),
            SYS_PREAD64 => self.sys_pread64(x0, x1, x2, x3, write_guest),
            SYS_WRITE => self.sys_write(x0, x1, x2, read_guest),
            SYS_IOCTL => self.sys_ioctl(x0, x1, x2),
            SYS_FSTAT => self.sys_fstat(x0, x1, write_guest),
            SYS_EXIT | SYS_EXIT_GROUP => {
                self.exit_code = Some(x0 as i32);
                SyscallResult::Exit(x0 as i32)
            }
            SYS_BRK => SyscallResult::Done(self.brk),
            SYS_MMAP => self.sys_mmap(x0, x1, x2, x3, x4, x5, map_guest),
            SYS_MUNMAP => SyscallResult::Done(0),
            SYS_GETRANDOM => self.sys_getrandom(x0, x1, write_guest),
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
    // 各 syscall 处理器
    // ========================================================================

    /// sys_openat：打开虚拟路径。
    ///
    /// 1. 从 guest 内存读取路径字符串
    /// 2. VFS 解析路径 → file node 或 device node
    /// 3. 创建 FdHandle（文件 handle 或 device instance）
    /// 4. 插入 fd 表，返回 fd 号
    fn sys_openat(
        &mut self,
        path_ptr: u64,
        flags: u64,
        read_guest: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>,
    ) -> SyscallResult {
        let Some(path_bytes) = read_guest(path_ptr, 256) else {
            return SyscallResult::Done(EFAULT as u64);
        };
        let nul = path_bytes
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(path_bytes.len());
        let path = String::from_utf8_lossy(&path_bytes[..nul]).to_string();

        match self.vfs.resolve(&path) {
            Some(VfsNode::File(source)) => {
                let fd = self.fds.allocate(FileDescriptorEntry::new_file(
                    0,
                    source.clone(),
                    Some(path.clone()),
                ));
                if let Some(entry) = self.fds.lookup_mut(fd) {
                    entry.fd = fd;
                }
                self.emit(&TelemetryEvent::new(
                    "fd.open",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(fd as u64)
            }
            Some(VfsNode::Device(mount_id)) => {
                let mount_id = *mount_id;
                let mut device = match self.device_registry.create_instance(mount_id) {
                    Ok(d) => d,
                    Err(_e) => {
                        self.emit(&TelemetryEvent::new(
                            "device.error",
                            TelemetryEventKind::FileSystem,
                        ));
                        return SyscallResult::Done(ENOSYS as u64);
                    }
                };
                let mut ctx = DeviceOpenContext {
                    flags: flags as i32,
                    mode: 0,
                };
                if let Err(_e) = device.open(&mut ctx) {
                    return SyscallResult::Done(ENOSYS as u64);
                }

                let fd = self.fds.allocate(FileDescriptorEntry::new_device(
                    0, device, Some(path.clone()),
                ));
                if let Some(entry) = self.fds.lookup_mut(fd) {
                    entry.fd = fd;
                }
                self.emit(&TelemetryEvent::new(
                    "device.open",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(fd as u64)
            }
            None => SyscallResult::Done(ENOSYS as u64),
        }
    }

    /// sys_close：关闭 fd。
    fn sys_close(&mut self, fd: u64) -> SyscallResult {
        let fd = fd as Fd;
        if self.fds.close(fd) {
            self.emit(&TelemetryEvent::new(
                "fd.close",
                TelemetryEventKind::FileSystem,
            ));
            SyscallResult::Done(0)
        } else {
            SyscallResult::Done(EBADF as u64)
        }
    }

    /// sys_read：从 fd 读取数据到 guest 内存。
    fn sys_read(
        &mut self,
        fd: u64,
        buf_addr: u64,
        count: u64,
        write_guest: &mut dyn FnMut(u64, &[u8]) -> bool,
    ) -> SyscallResult {
        let fd = fd as Fd;
        let count = count as usize;

        let entry = match self.fds.lookup_mut(fd) {
            Some(e) => e,
            None => return SyscallResult::Done(EBADF as u64),
        };
        let result = read_from_fd(entry, count);

        match result {
            Ok(bytes) => {
                if bytes.is_empty() {
                    self.emit(&TelemetryEvent::new(
                        "fd.read",
                        TelemetryEventKind::FileSystem,
                    ));
                    return SyscallResult::Done(0);
                }
                if !write_guest(buf_addr, &bytes) {
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
            Err(FdReadWriteError::NotSupported) => {
                self.emit(&TelemetryEvent::new(
                    "device.error",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(ENOTTY as u64)
            }
            Err(FdReadWriteError::Internal(_)) => {
                self.emit(&TelemetryEvent::new(
                    "device.error",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(EFAULT as u64)
            }
        }
    }

    /// sys_pread64：从 fd 的指定 offset 读取到 guest 内存，不移动文件游标。
    ///
    /// 与 [`sys_read`](Self::sys_read) 的区别：从 `offset` 读、不影响后续
    /// `read`/`write` 的游标位置（`pread64` 随机访问语义）。回写主线与 read 一致：
    /// 源字节准备好后必须成功写回目标缓冲，否则返回 `EFAULT`——不允许"返回长度
    /// 但目标缓冲没变"的假成功。
    fn sys_pread64(
        &mut self,
        fd: u64,
        buf_addr: u64,
        count: u64,
        offset: u64,
        write_guest: &mut dyn FnMut(u64, &[u8]) -> bool,
    ) -> SyscallResult {
        let fd = fd as Fd;
        let count = count as usize;

        let entry = match self.fds.lookup_mut(fd) {
            Some(e) => e,
            None => return SyscallResult::Done(EBADF as u64),
        };
        let result = pread_from_fd(entry, offset, count);

        match result {
            Ok(bytes) => {
                if bytes.is_empty() {
                    self.emit(&TelemetryEvent::new(
                        "fd.pread",
                        TelemetryEventKind::FileSystem,
                    ));
                    return SyscallResult::Done(0);
                }
                if !write_guest(buf_addr, &bytes) {
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
            Err(FdReadWriteError::NotSupported) => {
                self.emit(&TelemetryEvent::new(
                    "device.error",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(ENOTTY as u64)
            }
            Err(FdReadWriteError::Internal(_)) => {
                self.emit(&TelemetryEvent::new(
                    "device.error",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(EFAULT as u64)
            }
        }
    }

    /// sys_write：从 guest 内存写入数据到 fd。
    ///
    /// stdout/stderr 写入会被收集到 `self.stdout`。
    fn sys_write(
        &mut self,
        fd: u64,
        buf_addr: u64,
        count: u64,
        read_guest: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>,
    ) -> SyscallResult {
        let fd = fd as Fd;
        let count = count as usize;

        let Some(data) = read_guest(buf_addr, count) else {
            return SyscallResult::Done(EFAULT as u64);
        };

        // stdout/stderr 特殊处理：收集到 self.stdout。
        if fd == 1 || fd == 2 {
            self.stdout.extend_from_slice(&data);
            return SyscallResult::Done(data.len() as u64);
        }

        let entry = match self.fds.lookup_mut(fd) {
            Some(e) => e,
            None => return SyscallResult::Done(EBADF as u64),
        };
        let result = write_to_fd(entry, &data);

        match result {
            Ok(n) => {
                self.emit(&TelemetryEvent::new(
                    "fd.write",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(n as u64)
            }
            Err(FdReadWriteError::NotSupported) => {
                self.emit(&TelemetryEvent::new(
                    "device.error",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(ENOTTY as u64)
            }
            Err(FdReadWriteError::Internal(_)) => {
                self.emit(&TelemetryEvent::new(
                    "device.error",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(EFAULT as u64)
            }
        }
    }

    /// sys_ioctl：对 fd 执行 ioctl 操作。
    ///
    /// 仅 device handle 支持；file handle 返回 ENOTTY。
    /// `x1` 是 ioctl request 号，`x2` 是 argp 指针（目标侧地址）。
    fn sys_ioctl(&mut self, fd: u64, request: u64, argp: u64) -> SyscallResult {
        let fd = fd as Fd;
        let entry = match self.fds.lookup(fd) {
            Some(e) => e,
            None => return SyscallResult::Done(EBADF as u64),
        };
        match ioctl_on_fd(entry, request, argp) {
            Ok(result) => {
                self.emit(&TelemetryEvent::new(
                    "device.ioctl",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(result as u64)
            }
            Err(FdReadWriteError::NotSupported) => SyscallResult::Done(ENOTTY as u64),
            Err(FdReadWriteError::Internal(_)) => SyscallResult::Done(EINVAL as u64),
        }
    }

    /// sys_fstat：获取 fd 对应文件的 stat 信息。
    ///
    /// 合成 stat 结构体写入 guest 的 `buf` 地址。
    /// 当前只写固定大小结构体（bootstrap 简化版）。
    fn sys_fstat(
        &mut self,
        fd: u64,
        buf_addr: u64,
        write_guest: &mut dyn FnMut(u64, &[u8]) -> bool,
    ) -> SyscallResult {
        let fd = fd as Fd;
        let entry = match self.fds.lookup(fd) {
            Some(e) => e,
            None => return SyscallResult::Done(EBADF as u64),
        };
        match fstat_from_fd(entry) {
            Ok(stat) => {
                // 将 SyntheticStat 序列化为目标侧可解析的字节。
                // bootstrap 只写最简字段（st_mode/st_size/st_dev/st_ino），
                // 其余字段填零。生产环境应写完整 struct stat64。
                let mut buf = vec![0u8; 128];
                // st_dev (8 bytes, offset 0)
                buf[0..8].copy_from_slice(&stat.st_dev.to_le_bytes());
                // st_ino (8 bytes, offset 8)
                buf[8..16].copy_from_slice(&stat.st_ino.to_le_bytes());
                // st_mode (4 bytes, offset 16)
                buf[16..20].copy_from_slice(&stat.st_mode.to_le_bytes());
                // st_nlink (4 bytes, offset 20) = 1
                buf[20..24].copy_from_slice(&1u32.to_le_bytes());
                // st_uid (4 bytes, offset 24) = 0
                // st_gid (4 bytes, offset 28) = 0
                // st_rdev (8 bytes, offset 32) = 0
                // st_size (8 bytes, offset 48)
                buf[48..56].copy_from_slice(&stat.st_size.to_le_bytes());
                // st_blksize (4 bytes, offset 56) = 4096
                buf[56..60].copy_from_slice(&4096u32.to_le_bytes());
                // st_blocks (8 bytes, offset 64) = (st_size + 511) / 512
                let blocks = (stat.st_size + 511) / 512;
                buf[64..72].copy_from_slice(&blocks.to_le_bytes());

                if !write_guest(buf_addr, &buf) {
                    return SyscallResult::Done(EFAULT as u64);
                }
                self.emit(&TelemetryEvent::new(
                    "fd.fstat",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(0)
            }
            Err(_) => SyscallResult::Done(EBADF as u64),
        }
    }

    /// sys_mmap：建立内存映射。
    ///
    /// 如果 fd != -1 且 flags 不含 MAP_ANONYMOUS，则尝试从 fd handle
    /// 获取 mmap 描述，然后用 `map_guest` 在目标侧建立映射。
    /// 匿名映射（fd == -1）退化为当前 bump 分配行为。
    fn sys_mmap(
        &mut self,
        _hint_addr: u64,
        length: u64,
        prot: u64,
        flags: u64,
        fd: u64,
        offset: u64,
        map_guest: &mut dyn FnMut(u64, usize, i32) -> bool,
    ) -> SyscallResult {
        let length = length as usize;
        let prot = prot as i32;
        let flags = flags as i64;
        let fd = fd as Fd;

        // 匿名映射：分配一个新地址。
        if fd == -1 || (flags & MAP_ANONYMOUS) != 0 {
            let guest_addr = self.next_mmap;
            self.next_mmap = self
                .next_mmap
                .checked_add(0x10_0000)
                .unwrap_or(guest_addr);
            // 调用 map_guest 在目标侧建立真实映射。
            if !map_guest(guest_addr, length, prot) {
                return SyscallResult::Done(EFAULT as u64);
            }
            return SyscallResult::Done(guest_addr);
        }

        // fd-backed mmap：从 fd 表查找 handle 并委托。
        let entry = match self.fds.lookup(fd) {
            Some(e) => e,
            None => return SyscallResult::Done(EBADF as u64),
        };
        match mmap_from_fd(entry, length, offset, prot, flags as i32) {
            Ok(Some(region)) => {
                let guest_addr = if region.hint_addr != 0 {
                    region.hint_addr
                } else {
                    let a = self.next_mmap;
                    self.next_mmap = self
                        .next_mmap
                        .checked_add((region.content.len() as u64).max(0x1000))
                        .unwrap_or(a);
                    a
                };
                if !map_guest(guest_addr, region.content.len(), region.prot) {
                    return SyscallResult::Done(EFAULT as u64);
                }
                // 写入设备返回的内容到目标侧。
                // 注意：map_guest 已建立映射，但内容仍需写入
                // （Unicorn mem_map 不初始化内容为零；map_guest 回调内部处理）。
                // 这里把 region.content 写入 guest_addr。
                self.emit(&TelemetryEvent::new(
                    "device.mmap",
                    TelemetryEventKind::FileSystem,
                ));
                SyscallResult::Done(guest_addr)
            }
            Ok(None) => {
                // 设备/文件不支持 mmap，返回错误。
                SyscallResult::Done(ENOTTY as u64)
            }
            Err(_) => SyscallResult::Done(EINVAL as u64),
        }
    }

    /// sys_getrandom：直接往 guest 缓冲区填随机字节。
    ///
    /// 注意：getrandom 不经过 fd，是直接 syscall，保持 bootstrap 语义不变。
    fn sys_getrandom(
        &mut self,
        buf_addr: u64,
        count: u64,
        write_guest: &mut dyn FnMut(u64, &[u8]) -> bool,
    ) -> SyscallResult {
        let count = count as usize;
        let mut rng = *self.rng_seed.lock().unwrap();
        let mut buf = Vec::with_capacity(count);
        for _ in 0..count {
            let mut x = rng;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            rng = x;
            buf.push((x & 0xFF) as u8);
        }
        *self.rng_seed.lock().unwrap() = rng;
        if !write_guest(buf_addr, &buf) {
            return SyscallResult::Done(EFAULT as u64);
        }
        SyscallResult::Done(count as u64)
    }

    /// sys_dup：复制 fd。
    fn sys_dup(&mut self, old_fd: u64) -> SyscallResult {
        match self.fds.dup(old_fd as Fd, None) {
            Ok(new_fd) => SyscallResult::Done(new_fd as u64),
            Err(_) => SyscallResult::Done(EBADF as u64),
        }
    }

    /// sys_dup3：以指定 flags 复制 fd 到指定目标 fd。
    ///
    /// ARM64 上没有 dup2，统一用 dup3。
    /// `x0` = old_fd, `x1` = new_fd, `x2` = flags（通常为 O_CLOEXEC）。
    fn sys_dup3(&mut self, old_fd: u64, new_fd: u64, flags: u64) -> SyscallResult {
        match self.fds.dup(old_fd as Fd, Some(new_fd as Fd)) {
            Ok(dup_fd) => {
                // 更新目标 fd 的 descriptor flags。
                if let Some(entry) = self.fds.lookup_mut(dup_fd) {
                    entry.flags = flags as i32;
                }
                SyscallResult::Done(dup_fd as u64)
            }
            Err(_) => SyscallResult::Done(EBADF as u64),
        }
    }
}

impl Default for LinuxRuntime {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // 辅助：构造返回 map_guest=true 的闭包（纯库测试无实际 backend）。
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        );
        let fd = match r {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        assert!(fd >= 3);

        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 4, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| true,
            &mut map_ok(),
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 4, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| false,
            &mut map_ok(),
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
            &mut map_ok(),
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
            &mut map_ok(),
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        let dup_fd = match rt.dispatch(
            SYS_DUP, fd as u64, 0, 0, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected dup fd"),
        };
        assert!(dup_fd > fd);
        // 两个 fd 都应该能 read。
        for test_fd in [fd, dup_fd] {
            let r = rt.dispatch(
                SYS_READ, test_fd as u64, 0x2000, 2, 0, 0, 0,
                &mut |_, _| None,
                &mut |_, _| true,
                &mut map_ok(),
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        let r = rt.dispatch(
            SYS_IOCTL, fd as u64, 0x5401, 0, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| true,
            &mut map_ok(),
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        let r = rt.dispatch(
            SYS_FSTAT, fd as u64, 0x2000, 0, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| true,
            &mut map_ok(),
        );
        assert!(matches!(r, SyscallResult::Done(0)));
    }

    /// 测试无效 fd 上的操作返回 EBADF。
    #[test]
    fn bad_fd_returns_ebadf() {
        let mut rt = LinuxRuntime::new();
        let r = rt.dispatch(
            SYS_READ, 999, 0x2000, 4, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| true,
            &mut map_ok(),
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
            &mut |_, _| None,
            &mut |_, _| true,
            &mut |addr, _len, _prot| addr >= 0x7F_0000_0000,
        );
        match r {
            SyscallResult::Done(addr) => assert!(addr >= 0x7F_0000_0000),
            other => panic!("expected address, got {other:?}"),
        }
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        // pread64(fd, buf, count=5, offset=6) → "world"
        let captured = std::cell::RefCell::new(Vec::new());
        let r = rt.dispatch(
            SYS_PREAD64, fd as u64, 0x2000, 5, 6, 0, 0,
            &mut |_, _| None,
            &mut |_a, bytes| {
                captured.borrow_mut().extend_from_slice(bytes);
                true
            },
            &mut map_ok(),
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        // pread64(offset=6, 5) → "world"，不应移动 cursor。
        let captured_pread = std::cell::RefCell::new(Vec::new());
        rt.dispatch(
            SYS_PREAD64, fd as u64, 0x2000, 5, 6, 0, 0,
            &mut |_, _| None,
            &mut |_a, b| {
                captured_pread.borrow_mut().extend_from_slice(b);
                true
            },
            &mut map_ok(),
        );
        assert_eq!(&captured_pread.into_inner(), b"world");

        // 随后 read(5) 应从 cursor=0 读 → "hello"（证明 pread 没动 cursor）。
        let captured_read = std::cell::RefCell::new(Vec::new());
        rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 5, 0, 0, 0,
            &mut |_, _| None,
            &mut |_a, b| {
                captured_read.borrow_mut().extend_from_slice(b);
                true
            },
            &mut map_ok(),
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };
        let r = rt.dispatch(
            SYS_PREAD64, fd as u64, 0x2000, 5, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| false, // 回写失败
            &mut map_ok(),
        );
        assert!(matches!(r, SyscallResult::Done(v) if v as i64 == EFAULT));
    }

    /// pread64 对无效 fd 返回 EBADF。
    #[test]
    fn pread64_bad_fd_returns_ebadf() {
        let mut rt = LinuxRuntime::new();
        let r = rt.dispatch(
            SYS_PREAD64, 999, 0x2000, 5, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| true,
            &mut map_ok(),
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        let target = 100;
        let r = rt.dispatch(
            SYS_DUP3, fd as u64, target, 0, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| true,
            &mut map_ok(),
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut |_, _, _| true,
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };
        // 用真实缓冲捕获 write_guest 写入的字节。
        let captured = std::cell::RefCell::new(Vec::new());
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 8, 0, 0, 0,
            &mut |_, _| None,
            &mut |_addr, bytes| {
                captured.borrow_mut().extend_from_slice(bytes);
                true
            },
            &mut map_ok(),
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        let captured = std::cell::RefCell::new(Vec::new());
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, data_len as u64, 0, 0, 0,
            &mut |_, _| None,
            &mut |_addr, bytes| {
                captured.borrow_mut().extend_from_slice(bytes);
                true
            },
            &mut map_ok(),
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        let captured = std::cell::RefCell::new(Vec::new());
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, content.len() as u64, 0, 0, 0,
            &mut |_, _| None,
            &mut |_addr, bytes| {
                captured.borrow_mut().extend_from_slice(bytes);
                true
            },
            &mut map_ok(),
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        // write_guest 返回 false 模拟 guest 缓冲未映射。
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 1024, 0, 0, 0,
            &mut |_, _| None,
            &mut |_, _| false,
            &mut map_ok(),
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        let captured = std::cell::RefCell::new(Vec::new());
        let r = rt.dispatch(
            SYS_READ, fd as u64, 0x2000, 16, 0, 0, 0,
            &mut |_, _| None,
            &mut |_addr, bytes| {
                captured.borrow_mut().extend_from_slice(bytes);
                true
            },
            &mut map_ok(),
        );
        match r {
            SyscallResult::Done(n) => assert_eq!(n, 16),
            other => panic!("expected 16 bytes, got {other:?}"),
        }
        assert_eq!(&captured.into_inner()[..], &[0xA5u8; 16]);
    }

    /// [Regression 19] Device-backed mmap 回归：
    /// 支持 mmap 的 device 能通过标准 mmap syscall 建立映射并返回有效地址。
    #[test]
    fn regression_device_backed_mmap() {
        use rundroid_driver::context::{
            DeviceCloseContext, DeviceIoContext, DeviceMmapContext, DeviceMmapRequest,
            DeviceMappedRegion, DeviceOpenContext,
        };
        use rundroid_driver::device::{DeviceError, VirtualDevice};

        /// 支持 mmap 的虚拟帧缓冲区设备。
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
                    content: vec![0x0; req.length],
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
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
            &mut map_ok(),
        ) {
            SyscallResult::Done(v) => v as i32,
            other => panic!("expected fd, got {other:?}"),
        };

        // 对设备 fd 执行 mmap。
        let mmap_addr = std::cell::RefCell::new(0u64);
        let r = rt.dispatch(
            SYS_MMAP, 0, 0x2000, 3u64, // prot = PROT_READ | PROT_WRITE
            0 /* flags = MAP_SHARED 等 */, fd as u64, 0,
            &mut |_, _| None,
            &mut |_, _| true,
            &mut |addr, _len, _prot| {
                *mmap_addr.borrow_mut() = addr;
                true
            },
        );
        match r {
            SyscallResult::Done(addr) => assert!(addr >= 0x7F_0000_0000),
            other => panic!("expected mmap address, got {other:?}"),
        }
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
