//! 虚拟设备操作上下文。
//!
//! 每个设备操作都带一个上下文参数，由 OS/syscall 层在调用前填充。
//! 设备不反向依赖 backend / engine，只通过上下文获取必要信息。

/// 设备 open 时传入的上下文。
///
/// open 被调用时设备尚未就绪；后续 syscall（如 read）不应依赖 open 上下文。
pub struct DeviceOpenContext {
    /// 打开标志（O_RDONLY / O_WRONLY / O_RDWR / O_NONBLOCK 等）。
    pub flags: i32,
    /// 打开模式（仅 O_CREAT 时有效，bootstrap 通常为 0）。
    pub mode: i32,
}

/// 设备 read 时传入的上下文。
pub struct DeviceIoContext {
    /// 当前 fd。
    pub fd: i32,
}

/// 设备 ioctl 时传入的上下文。
pub struct DeviceIoctlContext {
    /// 当前 fd。
    pub fd: i32,
}

/// 设备 mmap 请求描述。
///
/// 设备返回"我想映射什么"，runtime 负责"怎么映射到目标侧"。
pub struct DeviceMmapRequest {
    /// 目标程序请求的 mmap 长度。
    pub length: usize,
    /// 目标程序请求的 mmap 偏移。
    pub offset: u64,
    /// mmap prot 标志（PROT_READ / PROT_WRITE / PROT_EXEC）。
    pub prot: i32,
    /// mmap flags（MAP_SHARED / MAP_PRIVATE 等）。
    pub flags: i32,
}

/// 设备 mmap 时传入的上下文。
pub struct DeviceMmapContext {
    /// 当前 fd。
    pub fd: i32,
}

/// 设备 mmap 成功后返回的区域描述。
///
/// 设备不直接调用 backend.mem_map，只描述这段内存的期望语义。
pub struct DeviceMappedRegion {
    /// 内容字节（runtime 负责写入目标侧）。
    pub content: Vec<u8>,
    /// 建议的目标侧起始地址（0 表示由 runtime 分配）。
    pub hint_addr: u64,
    /// 期望的页权限。
    pub prot: i32,
}

/// 设备 fstat 时传入的上下文。
pub struct DeviceStatContext {
    /// 当前 fd。
    pub fd: i32,
}

/// 合成 stat 结构，供 fstat 返回。
///
/// 简化为 bootstrap 所需的最小字段；后续可扩展。
pub struct SyntheticStat {
    /// st_mode：文件类型与权限。
    pub st_mode: u32,
    /// st_size：文件大小（字节）。
    pub st_size: u64,
    /// st_dev：设备号。
    pub st_dev: u64,
    /// st_ino：inode 号。
    pub st_ino: u64,
}

/// 设备 close 时传入的上下文。
pub struct DeviceCloseContext {
    /// 当前 fd。
    pub fd: i32,
}
