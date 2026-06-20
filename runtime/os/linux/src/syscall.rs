//! Linux syscall 分发。
//!
//! [`LinuxRuntime`] 持有 VFS 挂载表、设备注册表、fd 表，
//! 在 backend 遇到 `svc #0` 时按 AArch64 syscall 号分发。
//!
//! # 分发路径
//!
//! - `openat`：VFS 解析路径 → fd 表分配 entry → 返回 fd
//! - `read`：fd 表查找 → 按 FdHandle 分派到 file/device → 回写目标缓冲
//! - `write`：读目标缓冲 → fd 表查找 → 按 FdHandle 分派
//! - `close`：fd 表移除条目
//! - `mmap/munmap/brk/exit/getrandom`：保持现有逻辑
//!
//! 不再按 [`VfsSource`] 硬编码路径分派——所有分派统一经过 [`FileDescriptorTable`]。

use crate::fd::{
    FileDescriptorEntry, read_from_fd, write_to_fd, Fd, FdReadWriteError, FileDescriptorTable,
};
use crate::vfs::{VfsError, VfsMountTable, VfsNode};
use rundroid_driver::builtin::{null_factory, zero_factory};
use rundroid_driver::context::DeviceOpenContext;
use rundroid_driver::mapper::VirtFileSource;
use rundroid_driver::registry::DeviceRegistry;
use std::sync::{Arc, Mutex};

/// ARM64 Linux syscall 号（bootstrap subset）。
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
const SYS_DUP: u64 = 23;

/// 简化的 errno（POSIX 风格负数）。
pub const ENOSYS: i64 = -38;
pub const EBADF: i64 = -9;
pub const EFAULT: i64 = -14;
pub const EINVAL: i64 = -22;
pub const ENOTTY: i64 = -25;
pub const EACCES: i64 = -13;

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
}

impl LinuxRuntime {
    /// 创建新的运行时实例，预装 builtin 设备与路由配置。
    pub fn new() -> Self {
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
    ///
    /// 失败（例如路径冲突）直接 panic，让 case 配置错误显式暴露。
    pub fn mount_file(&mut self, virtual_path: &str, source: VirtFileSource) -> Result<(), VfsError> {
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

    /// 处理一次 syscall。
    ///
    /// `x0..x5` 是参数寄存器，`nr` 是 syscall 号。
    /// `read_guest` / `write_guest` 闭包由 backend 提供，
    /// 用于读写目标进程的虚拟地址空间。
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
            SYS_OPENAT => self.sys_openat(x1, x2, read_guest),
            SYS_CLOSE => self.sys_close(x0),
            SYS_READ => self.sys_read(x0, x1, x2, write_guest),
            SYS_WRITE => self.sys_write(x0, x1, x2, read_guest),
            SYS_EXIT | SYS_EXIT_GROUP => {
                self.exit_code = Some(x0 as i32);
                SyscallResult::Exit(x0 as i32)
            }
            SYS_BRK => SyscallResult::Done(self.brk),
            SYS_MMAP => {
                let addr = self.next_mmap;
                self.next_mmap = self.next_mmap.checked_add(0x10_0000).unwrap_or(addr);
                SyscallResult::Done(addr)
            }
            SYS_MUNMAP => SyscallResult::Done(0),
            SYS_GETRANDOM => self.sys_getrandom(x0, x1, write_guest),
            SYS_DUP => self.sys_dup(x0),
            _ => SyscallResult::Done(ENOSYS as u64),
        }
    }

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
                    0, // fd 由 allocate 填充
                    source.clone(),
                    Some(path),
                ));
                // 修正 fd 号（allocate 已分配）。
                // 注意：需要更新 entry 中的 fd 字段。
                if let Some(entry) = self.fds.lookup_mut(fd) {
                    entry.fd = fd;
                }
                SyscallResult::Done(fd as u64)
            }
            Some(VfsNode::Device(mount_id)) => {
                let mount_id = *mount_id;
                let mut device = match self.device_registry.create_instance(mount_id) {
                    Ok(d) => d,
                    Err(_e) => {
                        return SyscallResult::Done(ENOSYS as u64);
                    }
                };
                // 调用 device.open 初始化 per-fd 状态。
                let mut ctx = DeviceOpenContext {
                    flags: flags as i32,
                    mode: 0,
                };
                if let Err(_e) = device.open(&mut ctx) {
                    return SyscallResult::Done(ENOSYS as u64);
                }

                let fd = self.fds.allocate(FileDescriptorEntry::new_device(
                    0,
                    device,
                    Some(path),
                ));
                if let Some(entry) = self.fds.lookup_mut(fd) {
                    entry.fd = fd;
                }
                SyscallResult::Done(fd as u64)
            }
            None => SyscallResult::Done(ENOSYS as u64),
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

        // 查找 fd 条目并执行读操作。
        let result = {
            let entry = match self.fds.lookup_mut(fd) {
                Some(e) => e,
                None => return SyscallResult::Done(EBADF as u64),
            };
            read_from_fd(entry, count)
        };

        match result {
            Ok(bytes) => {
                if bytes.is_empty() {
                    return SyscallResult::Done(0);
                }
                if !write_guest(buf_addr, &bytes) {
                    return SyscallResult::Done(EFAULT as u64);
                }
                SyscallResult::Done(bytes.len() as u64)
            }
            Err(FdReadWriteError::NotSupported) => SyscallResult::Done(ENOTTY as u64),
            Err(FdReadWriteError::Internal(_)) => SyscallResult::Done(EFAULT as u64),
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

        // 查找 fd 条目并执行写操作。
        let result = {
            let entry = match self.fds.lookup_mut(fd) {
                Some(e) => e,
                None => return SyscallResult::Done(EBADF as u64),
            };
            write_to_fd(entry, &data)
        };

        match result {
            Ok(n) => SyscallResult::Done(n as u64),
            Err(FdReadWriteError::NotSupported) => SyscallResult::Done(ENOTTY as u64),
            Err(FdReadWriteError::Internal(_)) => SyscallResult::Done(EFAULT as u64),
        }
    }

    /// sys_close：关闭 fd。
    fn sys_close(&mut self, fd: u64) -> SyscallResult {
        let fd = fd as Fd;
        if self.fds.close(fd) {
            SyscallResult::Done(0)
        } else {
            SyscallResult::Done(EBADF as u64)
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
            path.as_ptr() as u64,
            0,
            0,
            0,
            0,
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
        );
        let fd = match r {
            SyscallResult::Done(v) => v as i32,
            _ => panic!("expected fd"),
        };
        assert!(fd >= 3);

        // read 4 字节：write_guest 必须返回 true。
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
        let mut rt = LinuxRuntime::new();
        rt.seed_rng(1);
        let path = b"/dev/urandom\0".to_vec();
        let fd = match rt.dispatch(
            SYS_OPENAT, 0, path.as_ptr() as u64, 0, 0, 0, 0,
            &mut move |_, _| Some(path.clone()),
            &mut |_, _| true,
        ) {
            SyscallResult::Done(v) => v as i32,
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

    #[test]
    fn vfs_duplicate_mount_fails() {
        let mut rt = LinuxRuntime::new();
        // 内置 /dev/null 已经挂载，尝试重复挂载应失败。
        let null_id = rt.device_registry.register(null_factory());
        let err = rt.vfs.mount_device("/dev/null", null_id);
        assert!(err.is_err());
    }
}
