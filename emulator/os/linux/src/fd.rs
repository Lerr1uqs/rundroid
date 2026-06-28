//! guest fd 表与文件描述符条目。
//!
//! [`FileDescriptorTable`] 是 OS 持有的 fd 句柄表，映射整数 fd →
//! [`FileDescriptorEntry`]。统一承接 `open`、`socket`、`pipe`、`eventfd` 等来源。
//!
//! # 职责
//!
//! - 分配、查询、替换、移除 [`FileDescriptorEntry`]
//! - 保证 syscall 先经由 fd 查到条目，再分发到已打开 handle
//! - 统一收纳有路径对象和无路径对象
//!
//! # 不负责
//!
//! - 路径挂载（由 VFS 负责）
//! - 设备注册（由 DeviceRegistry 负责）
//! - 直接实现 `read/write/ioctl/mmap`

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use rundroid_driver::context::{
    DeviceCloseContext, DeviceIoContext, DeviceIoctlContext, DeviceMmapContext, DeviceMmapRequest,
    DeviceMappedRegion, SyntheticStat,
};
use rundroid_driver::device::DeviceError;
use rundroid_driver::mapper::VirtFileSource;
use rundroid_driver::VirtualDevice;

/// 共享设备句柄。使用 Arc<Mutex<>> 包装，使 dup 可以浅克隆引用而不依赖设备 Clone。
pub type SharedDevice = Arc<Mutex<Box<dyn VirtualDevice>>>;

// ============================================================================
// 基础类型：FileHandle / FdHandle / FdKind / FileDescriptorEntry
// ============================================================================

/// 已打开文件的运行时状态。
///
/// 持有数据来源和当前读写游标。
pub struct FileHandle {
    /// 数据来源（宿主文件 / 内存 / 动态 provider）。
    pub source: VirtFileSource,
    /// 当前文件读写游标。
    pub cursor: u64,
}

/// 已打开对象的统一 handle。
///
/// syscall 层按此 enum 分发到对应后端，不再按路径硬编码分支。
pub enum FdHandle {
    /// 普通文件（宿主/内存/动态）。
    File(FileHandle),
    /// 虚拟设备（每 open 一次的新实例）。
    /// 用 Arc<Mutex<>> 包装以支持 dup：多个 fd slot 可共享同一设备实例。
    Device(SharedDevice),
    // 后续扩展：Socket, Pipe, Eventfd
}

/// fd 种类，用于分类统计和调试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdKind {
    /// 标准输入。
    Stdin,
    /// 标准输出。
    Stdout,
    /// 标准错误。
    Stderr,
    /// 普通文件（VirtFile 挂载）。
    File,
    /// 虚拟设备。
    Device,
}

/// 文件描述符槽位。
///
/// 保存 fd 编号、handle 引用和描述符元数据。
/// 它不是 file/device 行为对象；实际行为由 [`FdHandle`] 内部持有。
pub struct FileDescriptorEntry {
    /// 文件描述符编号。
    pub fd: i32,
    /// fd 种类。
    pub kind: FdKind,
    /// 已打开对象的 handle。
    pub handle: FdHandle,
    /// 描述符标志（如 O_CLOEXEC）。
    pub flags: i32,
    /// 虚拟路径（仅路径来源对象填写，用于诊断）。
    pub virtual_path: Option<String>,
}

impl FileDescriptorEntry {
    /// 为普通文件创建一个新的描述符条目。
    pub fn new_file(fd: i32, source: VirtFileSource, virtual_path: Option<String>) -> Self {
        Self {
            fd,
            kind: FdKind::File,
            handle: FdHandle::File(FileHandle { source, cursor: 0 }),
            flags: 0,
            virtual_path,
        }
    }

    /// 为虚拟设备创建一个新的描述符条目。
    ///
    /// 设备被包装为 Arc<Mutex<>> 共享句柄，使 dup 可以通过 Arc::clone 共享同一实例。
    pub fn new_device(
        fd: i32,
        device: Box<dyn VirtualDevice>,
        virtual_path: Option<String>,
    ) -> Self {
        Self {
            fd,
            kind: FdKind::Device,
            handle: FdHandle::Device(Arc::new(Mutex::new(device))),
            flags: 0,
            virtual_path,
        }
    }

    /// 为标准流创建一个新的描述符条目。
    pub fn new_stream(fd: i32, kind: FdKind) -> Self {
        assert!(
            matches!(kind, FdKind::Stdin | FdKind::Stdout | FdKind::Stderr),
            "new_stream only for stdin/stdout/stderr"
        );
        Self {
            fd,
            kind,
            handle: FdHandle::File(FileHandle {
                source: VirtFileSource::Bytes(Vec::new()),
                cursor: 0,
            }),
            flags: 0,
            virtual_path: None,
        }
    }
}

// ============================================================================
// FileHandle 上的读/写操作
// ============================================================================

/// FileHandle 上的读操作。
///
/// 按 VirtFileSource 变体统一分派，返回从当前游标开始的 `len` 字节。
/// 调用方负责把返回字节回写到目标侧内存。
pub fn file_read(handle: &mut FileHandle, len: usize) -> Result<Vec<u8>, FileReadError> {
    let result = match &handle.source {
        VirtFileSource::Bytes(bytes) => {
            let start = handle.cursor as usize;
            if start >= bytes.len() {
                Vec::new() // EOF
            } else {
                let end = (start + len).min(bytes.len());
                bytes[start..end].to_vec()
            }
        }
        VirtFileSource::HostPath(path) => {
            let mut file = std::fs::File::open(path)
                .map_err(|e| FileReadError::HostIo(e.to_string()))?;
            use std::io::{Read, Seek, SeekFrom};
            file.seek(SeekFrom::Start(handle.cursor))
                .map_err(|e| FileReadError::HostIo(e.to_string()))?;
            let mut buf = vec![0u8; len];
            let n = file
                .read(&mut buf)
                .map_err(|e| FileReadError::HostIo(e.to_string()))?;
            buf.truncate(n);
            buf
        }
        VirtFileSource::Dynamic(provider) => {
            provider
                .read_at(handle.cursor, len)
                .map_err(|e| FileReadError::ProviderError(e.message))?
        }
    };
    handle.cursor += result.len() as u64;
    Ok(result)
}

/// FileHandle 上的 pread：从指定 `offset` 读取 `len` 字节，**不移动游标**。
///
/// 对应 `pread64` 语义：随机访问读取，不影响后续 `read`/`write` 的游标位置。
/// 与 [`file_read`] 共用同一套源字节获取逻辑（Bytes/HostPath/Dynamic），
/// 差别仅在于用传入的 `offset` 替代 `handle.cursor` 且不写回游标。
pub fn file_pread(handle: &FileHandle, offset: u64, len: usize) -> Result<Vec<u8>, FileReadError> {
    let result = match &handle.source {
        VirtFileSource::Bytes(bytes) => {
            let start = offset as usize;
            if start >= bytes.len() {
                Vec::new() // EOF
            } else {
                let end = (start + len).min(bytes.len());
                bytes[start..end].to_vec()
            }
        }
        VirtFileSource::HostPath(path) => {
            let mut file = std::fs::File::open(path)
                .map_err(|e| FileReadError::HostIo(e.to_string()))?;
            use std::io::{Read, Seek, SeekFrom};
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| FileReadError::HostIo(e.to_string()))?;
            let mut buf = vec![0u8; len];
            let n = file
                .read(&mut buf)
                .map_err(|e| FileReadError::HostIo(e.to_string()))?;
            buf.truncate(n);
            buf
        }
        VirtFileSource::Dynamic(provider) => {
            provider
                .read_at(offset, len)
                .map_err(|e| FileReadError::ProviderError(e.message))?
        }
    };
    // 注意：pread 不修改 handle.cursor（与 read 的关键区别）
    Ok(result)
}

/// FileHandle 上的写操作。
///
/// 按 VirtFileSource 变体统一分派，返回实际写入字节数。
pub fn file_write(handle: &mut FileHandle, data: &[u8]) -> Result<usize, FileWriteError> {
    match &mut handle.source {
        VirtFileSource::Bytes(bytes) => {
            let start = handle.cursor as usize;
            if start + data.len() > bytes.len() {
                bytes.resize(start + data.len(), 0);
            }
            bytes[start..start + data.len()].copy_from_slice(data);
            handle.cursor += data.len() as u64;
            Ok(data.len())
        }
        VirtFileSource::HostPath(path) => {
            use std::io::{Seek, SeekFrom, Write};
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(false)
                .open(path)
                .map_err(|e| FileWriteError::HostIo(e.to_string()))?;
            file.seek(SeekFrom::Start(handle.cursor))
                .map_err(|e| FileWriteError::HostIo(e.to_string()))?;
            file.write_all(data)
                .map_err(|e| FileWriteError::HostIo(e.to_string()))?;
            handle.cursor += data.len() as u64;
            Ok(data.len())
        }
        VirtFileSource::Dynamic(_provider) => {
            Err(FileWriteError::NotWritable)
        }
    }
}

// ============================================================================
// 错误类型
// ============================================================================

/// 文件读取错误。
#[derive(Debug)]
pub enum FileReadError {
    /// 宿主文件 IO 错误。
    HostIo(String),
    /// 动态 provider 错误。
    ProviderError(String),
}

/// 文件写入错误。
#[derive(Debug)]
pub enum FileWriteError {
    /// 宿主文件 IO 错误。
    HostIo(String),
    /// 该文件源不支持写入。
    NotWritable,
}

/// dup 相关错误。
#[derive(Debug)]
pub enum DupError {
    /// 源 fd 不存在。
    BadFd,
    /// 当前不支持对该类 handle 执行 dup。
    NotSupported,
}

/// fd 读/写操作的统一错误类型。
#[derive(Debug)]
pub enum FdReadWriteError {
    /// 操作不支持。
    NotSupported,
    /// 内部错误。
    Internal(String),
}

// ============================================================================
// FileDescriptorTable
// ============================================================================

/// POSIX 文件描述符类型。
pub type Fd = i32;

/// 文件描述符表。
///
/// 所有 fd 来源（open/socket/pipe/eventfd）最终都进入这张表。
/// fd 0/1/2 预留为标准输入/输出/错误。
pub struct FileDescriptorTable {
    /// 下一个可分配的 fd 号。
    next_fd: Fd,
    /// fd → entry 映射。
    table: HashMap<Fd, FileDescriptorEntry>,
}

impl FileDescriptorTable {
    /// 创建一张新的 fd 表，预分配标准流 0/1/2。
    pub fn new() -> Self {
        let mut t = Self {
            next_fd: 3,
            table: HashMap::new(),
        };
        t.table.insert(0, FileDescriptorEntry::new_stream(0, FdKind::Stdin));
        t.table.insert(1, FileDescriptorEntry::new_stream(1, FdKind::Stdout));
        t.table.insert(2, FileDescriptorEntry::new_stream(2, FdKind::Stderr));
        t
    }

    /// 分配一个新 fd 并插入条目。
    pub fn allocate(&mut self, entry: FileDescriptorEntry) -> Fd {
        let fd = self.next_fd;
        self.next_fd += 1;
        self.table.insert(fd, entry);
        fd
    }

    /// 查找 fd 对应的条目（不可变借用）。
    pub fn lookup(&self, fd: Fd) -> Option<&FileDescriptorEntry> {
        self.table.get(&fd)
    }

    /// 查找 fd 对应的条目（可变借用）。
    pub fn lookup_mut(&mut self, fd: Fd) -> Option<&mut FileDescriptorEntry> {
        self.table.get_mut(&fd)
    }

    /// 关闭 fd：移除对应条目。
    ///
    /// 移除前若 entry 持有 device handle，先调用 [`VirtualDevice::close`]。
    /// 返回 `true` 表示已成功移除，`false` 表示 fd 无效。
    /// 标准流 0/1/2 不可关闭（bootstrap 简化语义）。
    pub fn close(&mut self, fd: Fd) -> bool {
        if (0..=2).contains(&fd) {
            return false;
        }
        if let Some(entry) = self.table.get_mut(&fd) {
            if let FdHandle::Device(dev) = &entry.handle {
                let mut ctx = DeviceCloseContext { fd };
                let _ = dev.lock().unwrap().close(&mut ctx);
            }
        }
        self.table.remove(&fd).is_some()
    }

    /// 复制 fd：创建新的 entry 指向同一 handle。
    ///
    /// 对应 `dup` / `dup2` 语义。
    /// `target_fd` 为 `None` 时自动分配新 fd。
    /// 标准流 0/1/2 不可被替换（bootstrap 简化语义）。
    pub fn dup(
        &mut self,
        source_fd: Fd,
        target_fd: Option<Fd>,
    ) -> Result<Fd, DupError> {
        let source = self
            .table
            .get(&source_fd)
            .ok_or(DupError::BadFd)?;
        if let Some(tgt) = target_fd {
            if source_fd == tgt {
                return Ok(source_fd);
            }
        }

        // clone handle：File handle 做浅克隆，Device 暂不支持。
        let new_entry = self.dup_entry_from(source, target_fd)?;

        let new_fd = match target_fd {
            Some(tgt) => {
                if (0..=2).contains(&tgt) {
                    return Err(DupError::BadFd);
                }
                self.table.insert(tgt, new_entry);
                tgt
            }
            None => {
                let fd = self.next_fd;
                self.next_fd += 1;
                self.table.insert(fd, new_entry);
                fd
            }
        };

        Ok(new_fd)
    }

    /// 从已有 entry 创建 dup 条目。
    fn dup_entry_from(
        &self,
        source: &FileDescriptorEntry,
        target_fd: Option<Fd>,
    ) -> Result<FileDescriptorEntry, DupError> {
        let fd = target_fd.unwrap_or(self.next_fd);
        match &source.handle {
            FdHandle::File(fh) => {
                Ok(FileDescriptorEntry {
                    fd,
                    kind: source.kind,
                    handle: FdHandle::File(FileHandle {
                        source: fh.source.clone(),
                        cursor: fh.cursor,
                    }),
                    flags: source.flags,
                    virtual_path: source.virtual_path.clone(),
                })
            }
            FdHandle::Device(shared_dev) => {
                // 设备句柄通过 Arc::clone 共享同一切实实例。
                // 新 fd slot 独立持有 descriptor flags，但底层读写状态是共享的。
                Ok(FileDescriptorEntry {
                    fd,
                    kind: source.kind,
                    handle: FdHandle::Device(Arc::clone(shared_dev)),
                    flags: source.flags,
                    virtual_path: source.virtual_path.clone(),
                })
            }
        }
    }
}

impl Default for FileDescriptorTable {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// FdHandle 上的统一分派方法
// ============================================================================

/// 对 fd 执行 read。
///
/// - 如果是 File handle，调用 `file_read`
/// - 如果是 Device handle，调用 `device.read()`
/// - 标准流 stdin 返回 EOF（空）
///
/// 返回的字节由调用方回写到目标侧。
pub fn read_from_fd(
    entry: &mut FileDescriptorEntry,
    len: usize,
) -> Result<Vec<u8>, FdReadWriteError> {
    match &mut entry.handle {
        FdHandle::File(fh) => {
            if matches!(entry.kind, FdKind::Stdin) {
                Ok(Vec::new())
            } else if matches!(entry.kind, FdKind::Stdout | FdKind::Stderr) {
                Ok(Vec::new())
            } else {
                file_read(fh, len).map_err(|e| match e {
                    FileReadError::HostIo(msg) => FdReadWriteError::Internal(msg),
                    FileReadError::ProviderError(msg) => FdReadWriteError::Internal(msg),
                })
            }
        }
        FdHandle::Device(dev) => {
            let mut ctx = DeviceIoContext { fd: entry.fd };
            dev.lock().unwrap().read(&mut ctx, len).map_err(|e| match e {
                DeviceError::NotSupported => FdReadWriteError::NotSupported,
                _ => FdReadWriteError::Internal(e.to_string()),
            })
        }
    }
}

/// 对 fd 执行 pread（从 `offset` 读，不移动游标）。
///
/// - File handle：调用 [`file_pread`]，不影响 cursor（`pread64` 随机访问语义）
/// - Device handle：设备 pread 退化为 [`VirtualDevice::read`]——
///   流式设备（如 `/dev/urandom`）每次产生新字节，`offset` 无意义，
///   保持与 read 相同的回写主线
/// - 标准流返回空（EOF）
///
/// 返回字节由调用方回写到目标侧，与 [`read_from_fd`] 共用同一条目标侧回写路径。
pub fn pread_from_fd(
    entry: &mut FileDescriptorEntry,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>, FdReadWriteError> {
    match &mut entry.handle {
        FdHandle::File(fh) => {
            if matches!(entry.kind, FdKind::Stdin | FdKind::Stdout | FdKind::Stderr) {
                Ok(Vec::new())
            } else {
                file_pread(fh, offset, len).map_err(|e| match e {
                    FileReadError::HostIo(msg) => FdReadWriteError::Internal(msg),
                    FileReadError::ProviderError(msg) => FdReadWriteError::Internal(msg),
                })
            }
        }
        FdHandle::Device(dev) => {
            // 流式设备 pread 退化为 read：复用回写主线，offset 忽略。
            let mut ctx = DeviceIoContext { fd: entry.fd };
            dev.lock().unwrap().read(&mut ctx, len).map_err(|e| match e {
                DeviceError::NotSupported => FdReadWriteError::NotSupported,
                _ => FdReadWriteError::Internal(e.to_string()),
            })
        }
    }
}

/// 对 fd 执行 write。
///
/// - 如果是 File handle，调用 `file_write`
/// - 如果是 Device handle，调用 `device.write()`
/// - stdout/stderr 由调用方在 syscall 层特殊处理（写入 LinuxRuntime.stdout）
pub fn write_to_fd(
    entry: &mut FileDescriptorEntry,
    data: &[u8],
) -> Result<usize, FdReadWriteError> {
    match &mut entry.handle {
        FdHandle::File(fh) => {
            if matches!(entry.kind, FdKind::Stdout | FdKind::Stderr) {
                Ok(data.len())
            } else if matches!(entry.kind, FdKind::Stdin) {
                Ok(0)
            } else {
                file_write(fh, data).map_err(|e| match e {
                    FileWriteError::HostIo(msg) => FdReadWriteError::Internal(msg),
                    FileWriteError::NotWritable => FdReadWriteError::NotSupported,
                })
            }
        }
        FdHandle::Device(dev) => {
            let mut ctx = DeviceIoContext { fd: entry.fd };
            dev.lock().unwrap().write(&mut ctx, data).map_err(|e| match e {
                DeviceError::NotSupported => FdReadWriteError::NotSupported,
                _ => FdReadWriteError::Internal(e.to_string()),
            })
        }
    }
}

/// 对 fd 执行 ioctl。
///
/// - File handle 暂不支持 ioctl（返回 ENOTTY）。
/// - Device handle 调用 `device.ioctl()`。
/// - 返回值写入目标侧 x0（i64 语义）。
pub fn ioctl_on_fd(
    entry: &FileDescriptorEntry,
    request: u64,
    argp: u64,
) -> Result<i64, FdReadWriteError> {
    match &entry.handle {
        FdHandle::File(_) => {
            let _ = (request, argp);
            Err(FdReadWriteError::NotSupported)
        }
        FdHandle::Device(dev) => {
            let mut ctx = DeviceIoctlContext { fd: entry.fd };
            dev.lock().unwrap().ioctl(&mut ctx, request, argp).map_err(|e| match e {
                DeviceError::NotSupported => FdReadWriteError::NotSupported,
                _ => FdReadWriteError::Internal(e.to_string()),
            })
        }
    }
}

/// 从 fd 合成 stat 信息。
///
/// - File handle：根据 VirtFileSource 合成文件 stat（st_mode = S_IFREG）。
/// - Device handle：调用 `device.fstat()`。
pub fn fstat_from_fd(
    entry: &FileDescriptorEntry,
) -> Result<SyntheticStat, FdReadWriteError> {
    match &entry.handle {
        FdHandle::File(fh) => {
            let (st_size, st_dev, st_ino) = match &fh.source {
                VirtFileSource::HostPath(_) => {
                    // 尝试统计宿主文件大小。
                    (0u64, 0u64, 0u64)
                }
                VirtFileSource::Bytes(bytes) => {
                    (bytes.len() as u64, 0u64, 0u64)
                }
                VirtFileSource::Dynamic(_) => {
                    (0u64, 0u64, 0u64)
                }
            };
            Ok(SyntheticStat {
                st_mode: 0x8180, // S_IFREG | 0600
                st_size,
                st_dev,
                st_ino,
            })
        }
        FdHandle::Device(dev) => {
            let ctx = rundroid_driver::context::DeviceStatContext { fd: entry.fd };
            dev.lock().unwrap().fstat(&ctx).map_err(|_| FdReadWriteError::NotSupported)
        }
    }
}

/// 从 fd 查询 mmap 区域描述。
///
/// - File handle：暂不支持（返回 Ok(None)）。
/// - Device handle：调用 `device.mmap()`，返回设备侧区域描述。
/// - runtime 负责后续的目标侧 mem_map。
pub fn mmap_from_fd(
    entry: &FileDescriptorEntry,
    length: usize,
    offset: u64,
    prot: i32,
    flags: i32,
) -> Result<Option<DeviceMappedRegion>, FdReadWriteError> {
    match &entry.handle {
        FdHandle::File(_fh) => {
            // File-backed mmap 暂未实现（Phase 2）。
            // 对常规文件执行 mmap 在 bootstrap 阶段返回 None，
            // 调用侧回退为匿名映射。
            let _ = (_fh, length, offset, prot, flags);
            Ok(None)
        }
        FdHandle::Device(dev) => {
            let mut ctx = DeviceMmapContext { fd: entry.fd };
            let req = DeviceMmapRequest {
                length,
                offset,
                prot,
                flags,
            };
            dev.lock().unwrap().mmap(&mut ctx, &req).map_err(|_| FdReadWriteError::NotSupported)
        }
    }
}
