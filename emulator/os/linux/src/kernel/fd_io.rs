//! fd IO 语义方法（kernel 域）。
//!
//! `impl LinuxRuntime` 的 fd 操作：read / read_at / write / ioctl / fstat / dup / dup3 / close。
//! 复用 [`crate::fd`] 底座函数（read_from_fd / pread_from_fd / write_to_fd / ioctl_on_fd /
//! fstat_from_fd），只产出数据／推进 fd 表状态，不接收
//! [`crate::memory_bridge::MemoryBridge`]。目标侧回写由 syscall 层负责。
//!
//! 其中 `write` 内聚处理 stdout/stderr 收集（fd=1/2 追加到 `LinuxRuntime.stdout`），
//! syscall 层不再保留独立的 stdout 分支。

use super::LinuxRuntime;
use crate::fd::{
    read_from_fd, pread_from_fd, write_to_fd, ioctl_on_fd, fstat_from_fd, DupError, Fd,
    FdReadWriteError,
};
use rundroid_driver::context::SyntheticStat;

/// kernel fd 操作错误。区分"fd 无效"与"底座 IO 错误"，
/// 供 syscall 层编码不同 errno（BadFd→EBADF，IO 错误按域映射）。
#[derive(Debug)]
pub enum FdOpError {
    /// fd 不在表中。
    BadFd,
    /// 底座 read/write/ioctl/fstat/mmap 返回的错误。
    Io(FdReadWriteError),
}

/// kernel write 结果。
///
/// 区分 stdout/stderr 收集与真实 fd 写入，让 syscall 层据此决定是否
/// emit `fd.write`（stdout 收集不产生该事件）。
#[derive(Debug)]
pub enum WriteOutcome {
    /// 写入 stdout(1)/stderr(2) 收集器。
    StdStream(usize),
    /// 写入真实 fd。
    Fd(usize),
}

impl LinuxRuntime {
    /// 从 fd 读取（移动游标）。返回读到的字节。
    ///
    /// 语义源：`read` syscall。游标移动由 [`crate::fd::read_from_fd`] 底座负责。
    pub fn read(&mut self, fd: Fd, count: usize) -> Result<Vec<u8>, FdOpError> {
        let entry = self.fds.lookup_mut(fd).ok_or(FdOpError::BadFd)?;
        read_from_fd(entry, count).map_err(FdOpError::Io)
    }

    /// pread64：从 `offset` 读取（不移动游标）。返回读到的字节。
    ///
    /// 语义源：`pread64` syscall（随机访问）。与 [`read`](Self::read) 共用底座源字节获取，
    /// 差别仅在于用传入 offset 替代游标且不写回游标。
    pub fn read_at(&mut self, fd: Fd, offset: u64, count: usize) -> Result<Vec<u8>, FdOpError> {
        let entry = self.fds.lookup_mut(fd).ok_or(FdOpError::BadFd)?;
        pread_from_fd(entry, offset, count).map_err(FdOpError::Io)
    }

    /// write 语义：fd=1/2 收集到 stdout，其余走 fd 表／设备。
    ///
    /// 返回 [`WriteOutcome`] 让 syscall 层区分 emit（stdout 不 emit `fd.write`）。
    /// stdout/stderr 特例归属 kernel 写语义，syscall 层不保留独立分支。
    pub fn write(&mut self, fd: Fd, data: &[u8]) -> Result<WriteOutcome, FdOpError> {
        if fd == 1 || fd == 2 {
            self.stdout.extend_from_slice(data);
            return Ok(WriteOutcome::StdStream(data.len()));
        }
        let entry = self.fds.lookup_mut(fd).ok_or(FdOpError::BadFd)?;
        let n = write_to_fd(entry, data).map_err(FdOpError::Io)?;
        Ok(WriteOutcome::Fd(n))
    }

    /// ioctl：仅 device 支持（file handle 返回 NotSupported）。
    ///
    /// 返回 i64 结果（写入 guest x0）。`request` 是 ioctl 号，`argp` 是目标侧指针。
    pub fn ioctl(&self, fd: Fd, request: u64, argp: u64) -> Result<i64, FdOpError> {
        let entry = self.fds.lookup(fd).ok_or(FdOpError::BadFd)?;
        ioctl_on_fd(entry, request, argp).map_err(FdOpError::Io)
    }

    /// fstat：合成 stat 信息（数据）。
    ///
    /// 返回 [`SyntheticStat`]，目标侧 struct stat64 序列化由 syscall 层完成。
    pub fn fstat(&self, fd: Fd) -> Result<SyntheticStat, FdOpError> {
        let entry = self.fds.lookup(fd).ok_or(FdOpError::BadFd)?;
        fstat_from_fd(entry).map_err(FdOpError::Io)
    }

    /// dup：复制 fd 到自动分配的新 fd。
    pub fn dup(&mut self, old_fd: Fd) -> Result<Fd, DupError> {
        self.fds.dup(old_fd, None)
    }

    /// dup3：以指定 flags 复制 fd 到指定目标 fd（ARM64 无 dup2，统一 dup3）。
    ///
    /// `new_fd` 为目标 fd，`flags` 通常为 O_CLOEXEC。
    pub fn dup3(&mut self, old_fd: Fd, new_fd: Fd, flags: i32) -> Result<Fd, DupError> {
        let dup_fd = self.fds.dup(old_fd, Some(new_fd))?;
        // 更新目标 fd 的 descriptor flags。
        if let Some(entry) = self.fds.lookup_mut(dup_fd) {
            entry.flags = flags;
        }
        Ok(dup_fd)
    }

    /// close：关闭 fd（标准流 0/1/2 不可关闭，bootstrap 简化语义）。
    pub fn close(&mut self, fd: Fd) -> bool {
        self.fds.close(fd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::VfsNode;
    use rundroid_driver::mapper::VirtFileSource;

    /// read_at 从 offset 读、不移动游标；之后 read 仍从游标 0 开始。
    /// 直接调 kernel 方法，脱离 dispatch / mock MemoryBridge。
    #[test]
    fn read_at_reads_offset_without_moving_cursor() {
        let mut rt = LinuxRuntime::new();
        rt.mount_file("/data/cursor.txt", VirtFileSource::Bytes(b"hello world".to_vec()))
            .unwrap();

        let fd = rt.open_path("/data/cursor.txt", 0).expect("open_path");

        // read_at(fd, offset=6, 5) → "world"，不动游标。
        let pread = rt.read_at(fd, 6, 5).unwrap();
        assert_eq!(&pread, b"world");

        // 随后 read(fd, 5) 应从 cursor=0 读 → "hello"。
        let read = rt.read(fd, 5).unwrap();
        assert_eq!(&read, b"hello");
    }

    /// read_at 对无效 fd 返回 BadFd。
    #[test]
    fn read_at_bad_fd_returns_bad_fd() {
        let mut rt = LinuxRuntime::new();
        assert!(matches!(rt.read_at(999, 0, 5), Err(FdOpError::BadFd)));
    }

    /// write(1/2) 追加到 stdout 收集器，返回 StdStream。
    #[test]
    fn write_to_stdout_appends_to_collector() {
        let mut rt = LinuxRuntime::new();
        match rt.write(1, b"hello\n").unwrap() {
            WriteOutcome::StdStream(n) => assert_eq!(n, 6),
            other => panic!("expected StdStream, got {other:?}"),
        }
        match rt.write(2, b"err\n").unwrap() {
            WriteOutcome::StdStream(n) => assert_eq!(n, 4),
            other => panic!("expected StdStream, got {other:?}"),
        }
        // stdout 与 stderr 统一收集到同一缓冲（bootstrap 语义）。
        assert_eq!(&rt.stdout, b"hello\nerr\n");
    }

    /// write 到真实 fd 走底座；file handle 返回 Fd 写入长度。
    #[test]
    fn write_to_real_fd_returns_fd_outcome() {
        let mut rt = LinuxRuntime::new();
        rt.mount_file("/data/w.txt", VirtFileSource::Bytes(vec![0u8; 8]))
            .unwrap();
        let fd = rt.open_path("/data/w.txt", 0).expect("open");
        match rt.write(fd, b"abc") {
            Ok(WriteOutcome::Fd(n)) => assert_eq!(n, 3),
            other => panic!("expected Fd(3), got {other:?}"),
        }
    }

    /// write 对无效 fd 返回 BadFd。
    #[test]
    fn write_bad_fd_returns_bad_fd() {
        let mut rt = LinuxRuntime::new();
        assert!(matches!(rt.write(999, b"x"), Err(FdOpError::BadFd)));
    }

    /// open_path 解析 file 节点 → VfsNode::File；mount 后可打开。
    #[test]
    fn open_path_resolves_file_node() {
        let mut rt = LinuxRuntime::new();
        rt.mount_file("/data/x", VirtFileSource::Bytes(b"hi".to_vec()))
            .unwrap();
        let fd = rt.open_path("/data/x", 0).expect("open");
        assert!(fd >= 3);
        // 确认 VFS 挂载类型未被破坏。
        assert!(matches!(
            rt.vfs.resolve("/data/x"),
            Some(VfsNode::File(_))
        ));
    }

    /// open_path 未挂载路径返回 None。
    #[test]
    fn open_path_unmapped_returns_none() {
        let mut rt = LinuxRuntime::new();
        assert!(rt.open_path("/nope", 0).is_none());
    }
}
