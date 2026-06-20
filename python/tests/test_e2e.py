"""rundroid Python E2E 测试。

验证：
1. 纯导出函数调用
2. 自定义 device 被目标程序 openat → read 真实命中
3. 内建 /dev/urandom 被目标程序 openat → read
4. VirtFile.bytes 挂载被目标程序 openat → read
5. 路径冲突报错
6. getrandom 确定性
"""

import os
import pytest
from rundroid import Runtime, VirtFile, VirtualDevice, device, register

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SMOKE_SO = os.path.join(REPO_ROOT, "resources", "smoke", "build", "libsmoke.so")

SCRATCH = 0x800000


def _write_path(rt, addr, path: str):
    data = path.encode("utf-8") + b"\0"
    rt.write_guest(addr, data)


@pytest.fixture
def rt():
    """创建 Runtime 并在测试后显式关闭，避免 Unicorn 引擎在 FFI Drop 时崩溃。"""
    r = Runtime("arm64", "unicorn", 42)
    r.load("smoke", open(SMOKE_SO, "rb").read())
    yield r
    r.close()


def test_pure_export(rt):
    """纯导出调用不依赖 syscall。"""
    assert rt.call("rd_add", 1, 2) == 3


def test_custom_device_open_read(rt):
    """Python @device 注册的设备被目标程序 openat → read 真实命中。"""
    @device("/dev/pytest")
    class PyTestDev(VirtualDevice):
        def read(self, fd, size):
            return b"PYTHON"

    register(rt, [PyTestDev])

    PATH_ADDR = SCRATCH
    BUF_ADDR = SCRATCH + 0x100
    _write_path(rt, PATH_ADDR, "/dev/pytest")
    result = rt.call("rd_open_read", PATH_ADDR, BUF_ADDR, 6)
    assert result == 6, f"expected 6 bytes, got {result}"


def test_builtin_urandom_open_read(rt):
    """内置 /dev/urandom 被目标程序 openat + read 真实返回字节。"""
    PATH_ADDR = SCRATCH
    BUF_ADDR = SCRATCH + 0x100
    _write_path(rt, PATH_ADDR, "/dev/urandom")
    result = rt.call("rd_open_read", PATH_ADDR, BUF_ADDR, 16)
    assert result == 16, f"expected 16 bytes, got {result}"


def test_virt_file_open_read(rt):
    """VirtFile.bytes 挂载的文件被目标程序 openat + read 真实命中。"""
    rt.fs.map_file("/data/hello.txt", VirtFile.bytes(b"HELLO"))
    PATH_ADDR = SCRATCH
    BUF_ADDR = SCRATCH + 0x100
    _write_path(rt, PATH_ADDR, "/data/hello.txt")
    result = rt.call("rd_open_read", PATH_ADDR, BUF_ADDR, 5)
    assert result == 5, f"expected 5 bytes, got {result}"


def test_duplicate_path_error(rt):
    """同名路径重复挂载立即报错。"""
    rt.fs.map_file("/dup", VirtFile.bytes(b"first"))
    with pytest.raises(ValueError):
        rt.fs.map_file("/dup", VirtFile.bytes(b"second"))


def test_get_random(rt):
    """getrandom syscall 返回确定性结果。"""
    result = rt.call("rd_get_random", 0x800000, 16)
    assert result == 89  # seed=42 的确定性校验和


if __name__ == "__main__":
    r = Runtime("arm64", "unicorn", 42)
    r.load("smoke", open(SMOKE_SO, "rb").read())
    test_pure_export(r)
    test_custom_device_open_read(r)
    test_builtin_urandom_open_read(r)
    test_virt_file_open_read(r)
    test_duplicate_path_error(r)
    test_get_random(r)
    r.close()
    print("All tests passed!")
