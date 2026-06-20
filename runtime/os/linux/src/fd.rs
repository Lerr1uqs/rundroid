//! guest fd 表。
//!
//! guest 看到的"fd"是一个 `i32`（POSIX 语义），
//! 本层维护它到 [`FdType`] 的映射，不直接持有 host fd
//! ——因为 `/dev/urandom` 等是确定性虚拟文件，不需要真正打开 host 设备。

use std::collections::HashMap;

/// guest fd 值。0/1/2 保留给 stdin/stdout/stderr。
pub type Fd = i32;

/// 一个 guest fd 背后的"实际数据来源"。
#[derive(Debug, Clone)]
pub enum FdType {
    /// stdin（fd 0），bootstrap 不实际消费，read 返回 0。
    Stdin,
    /// stdout（fd 1）/ stderr（fd 2），write 记录到 ring buffer 供 artifact 输出。
    Stdout,
    Stderr,
    /// 来自 VFS 的虚拟文件（例如 /dev/urandom）。
    Vfs(crate::vfs::VfsSource),
}

/// fd 表。
#[derive(Debug, Default)]
pub struct FdTable {
    next: Fd,
    table: HashMap<Fd, FdType>,
}

impl FdTable {
    pub fn new() -> Self {
        let mut t = Self::default();
        t.next = 3; // 0/1/2 预留给标准流
        t.table.insert(0, FdType::Stdin);
        t.table.insert(1, FdType::Stdout);
        t.table.insert(2, FdType::Stderr);
        t
    }

    /// 分配一个新 fd。
    pub fn allocate(&mut self, ty: FdType) -> Fd {
        let id = self.next;
        self.next += 1;
        self.table.insert(id, ty);
        id
    }

    pub fn get(&self, fd: Fd) -> Option<&FdType> {
        self.table.get(&fd)
    }

    pub fn close(&mut self, fd: Fd) -> bool {
        // 标准流不允许 close（POSIX 上允许，bootstrap 简化为禁止，避免误关）。
        if (0..=2).contains(&fd) {
            return false;
        }
        self.table.remove(&fd).is_some()
    }
}
