//! LinuxRuntime 聚合根：OS 状态与核心配置入口。
//!
//! 本模块定义 [`LinuxRuntime`] 类型本身（字段 + 构造 + 配置 API + 路径解析 + lifecycle），
//! 各 OS 子系统域的语义方法以分文件 `impl LinuxRuntime` 形式落在：
//! - [`fd_io`]：fd IO（read / read_at / write / ioctl / fstat / dup / dup3 / close）
//! - [`mem`]：内存管理（alloc_mmap_addr / brk / munmap / device_mmap）
//! - [`random`]：PRNG（getrandom_bytes）
//!
//! 所有 kernel 方法只产出数据／推进 OS 状态，**不接收**
//! [`crate::memory_bridge::MemoryBridge`]——目标侧回写由 [`crate::syscall`] 层统一负责。

pub mod fd_io;
pub mod mem;
pub mod random;

// 域共享类型对外收敛到 kernel 根。
pub use fd_io::{FdOpError, WriteOutcome};
pub use mem::MmapOutcome;

use crate::fd::{Fd, FileDescriptorEntry, FileDescriptorTable};
use crate::vfs::{VfsMountTable, VfsNode};
use rundroid_driver::builtin::{null_factory, zero_factory};
use rundroid_driver::context::DeviceOpenContext;
use rundroid_driver::mapper::VirtFileSource;
use rundroid_driver::registry::{DeviceFactory, DeviceRegistry};
use rundroid_driver::VirtualDevice;
use rundroid_telemetry::{TelemetryEvent, TelemetryEventKind, TelemetryRouter};
use std::sync::{Arc, Mutex};

/// Linux 用户态运行时（OS 聚合根）。
///
/// 持有 VFS 挂载表、设备注册表、fd 表和内存布局状态。
/// 字段保持扁平（bootstrap 阶段状态少，扁平更直读）；各域方法以分文件 impl 实现。
pub struct LinuxRuntime {
    /// 虚拟文件系统挂载表。
    pub vfs: VfsMountTable,
    /// 设备注册表。
    pub device_registry: DeviceRegistry,
    /// 文件描述符表。
    pub fds: FileDescriptorTable,
    /// mmap 的"下一次返回地址"。私有：仅 kernel mem 域推进。
    next_mmap: u64,
    /// brk 当前值。私有：仅 kernel mem 域读取。
    brk: u64,
    /// 收集到的 stdout 字节（write(1/2) 语义落地处）。
    pub stdout: Vec<u8>,
    /// exit 请求的退出码。
    pub exit_code: Option<i32>,
    /// 确定性 PRNG 种子源（getrandom 与 builtin urandom factory 共用）。
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
                as Box<dyn VirtualDevice>
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
                as Box<dyn VirtualDevice>
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
    ) -> Result<(), crate::vfs::VfsError> {
        self.vfs.mount_file(virtual_path, source)
    }

    /// 挂载一个设备节点（供 case 配置自定义设备）。
    pub fn mount_device(
        &mut self,
        virtual_path: &str,
        factory: DeviceFactory,
    ) -> Result<(), crate::vfs::VfsError> {
        let mount_id = self.device_registry.register(factory);
        self.vfs.mount_device(virtual_path, mount_id)
    }

    /// 发出 telemetry 事件（如果 router 已配置）。
    ///
    /// `pub(crate)`：syscall 层各 handler 也需要 emit 操作级事件（fd.read / device.error 等）。
    pub(crate) fn emit(&mut self, event: &TelemetryEvent<'_>) {
        if let Some(router) = self.telemetry.as_mut() {
            router.emit(event);
        }
    }

    /// 解析虚拟路径并打开（VFS → fd 表），推进 OS 状态返回新 fd。
    ///
    /// 接收已解码的路径字符串（syscall 层负责从 guest 读出路径字节）。
    /// openat 没有 guest 回写，故本方法内聚完成 telemetry emit：
    /// - file 成功 → emit `fd.open`
    /// - device 成功 → emit `device.open`
    /// - device create 失败 → emit `device.error`
    /// - device open 失败／路径未解析 → 不 emit（与原实现一致）
    ///
    /// 返回 `None` 表示路径无法解析／设备打开失败（syscall 层统一映射为 ENOSYS）。
    pub fn open_path(&mut self, path: &str, flags: i32) -> Option<Fd> {
        match self.vfs.resolve(path) {
            Some(VfsNode::File(source)) => {
                let fd = self.fds.allocate(FileDescriptorEntry::new_file(
                    0,
                    source.clone(),
                    Some(path.to_string()),
                ));
                if let Some(entry) = self.fds.lookup_mut(fd) {
                    entry.fd = fd;
                }
                self.emit(&TelemetryEvent::new(
                    "fd.open",
                    TelemetryEventKind::FileSystem,
                ));
                Some(fd)
            }
            Some(VfsNode::Device(mount_id)) => {
                let mount_id = *mount_id;
                let mut device = match self.device_registry.create_instance(mount_id) {
                    Ok(d) => d,
                    Err(_) => {
                        self.emit(&TelemetryEvent::new(
                            "device.error",
                            TelemetryEventKind::FileSystem,
                        ));
                        return None;
                    }
                };
                let mut ctx = DeviceOpenContext { flags, mode: 0 };
                // device.open 失败：保持与原实现一致（不 emit，直接 None → ENOSYS）。
                if device.open(&mut ctx).is_err() {
                    return None;
                }

                let fd = self.fds.allocate(FileDescriptorEntry::new_device(
                    0,
                    device,
                    Some(path.to_string()),
                ));
                if let Some(entry) = self.fds.lookup_mut(fd) {
                    entry.fd = fd;
                }
                self.emit(&TelemetryEvent::new(
                    "device.open",
                    TelemetryEventKind::FileSystem,
                ));
                Some(fd)
            }
            None => None,
        }
    }

    /// lifecycle：记录 exit 退出码。
    pub fn exit(&mut self, code: i32) {
        self.exit_code = Some(code);
    }
}

impl Default for LinuxRuntime {
    fn default() -> Self {
        Self::new()
    }
}
