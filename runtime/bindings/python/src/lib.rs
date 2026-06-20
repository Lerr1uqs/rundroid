//! `rundroid-bindings-python`
//!
//! PyO3 绑定层：把 Rust 侧的 Runtime / VFS / 设备注册面暴露给 Python。
//!
//! Python 模块名为 `_rundroid`（C 扩展），上层 `rundroid` 包在此之上提供
//! decorator、VirtFile 构造器等 Pythonic API。
//!
//! # 线程安全性说明
//!
//! PyRuntime 持有 `Box<dyn Engine>` 但 Engine 不是 Sync。
//! 以下 `unsafe impl Send/Sync` 的前提是：PyRuntime 仅在 Python GIL 线程中访问，
//! 不会跨线程共享 engine 引用。所有 engine 操作都在 Python 方法调用上下文中执行。

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple, PyType};
use rundroid_backend::{Arm64Reg, Backend, Engine, MemPerms, SyscallCpu, SyscallHook};
use rundroid_backend_unicorn::UnicornBackend;
use rundroid_driver::{
    VirtualDevice,
    context::{DeviceCloseContext, DeviceIoContext, DeviceOpenContext},
    mapper::VirtFileSource,
    registry::DeviceFactory,
    device::DeviceError,
};
use rundroid_elf_loader::{DefaultLoader, ElfLoader, LoadContext, LoadRequest};
use rundroid_elf_parser::{ElfCrateParser, ElfParser, ParseInput, ParsedElf};
use rundroid_elf_linker::{
    DefaultLinker, LinkContext, ModuleGraph, RelocationPatch, ResolvedSymbol, SymbolQuery,
};
use rundroid_linux::{LinuxRuntime, SyscallResult};
use rundroid_memory::MemoryError;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// ============================================================================
// PyVirtFile
// ============================================================================

#[pyclass(name = "VirtFile")]
#[derive(Clone)]
struct PyVirtFile {
    source: VirtFileSource,
}

#[pymethods]
impl PyVirtFile {
    #[staticmethod]
    fn bytes(data: Vec<u8>) -> Self {
        Self { source: VirtFileSource::Bytes(data) }
    }

    #[staticmethod]
    fn host(path: String) -> Self {
        Self { source: VirtFileSource::HostPath(PathBuf::from(path)) }
    }
}

// ============================================================================
// PyDeviceAdapter
// ============================================================================

struct PyDeviceAdapter {
    obj: Py<PyAny>,
}

impl VirtualDevice for PyDeviceAdapter {
    fn open(&mut self, ctx: &mut DeviceOpenContext) -> Result<(), DeviceError> {
        Python::with_gil(|py| {
            self.obj.bind(py).call_method1("open", (ctx.flags, ctx.mode))
                .map_err(|e| DeviceError::Internal(e.to_string()))?;
            Ok(())
        })
    }

    fn read(&mut self, ctx: &mut DeviceIoContext, len: usize) -> Result<Vec<u8>, DeviceError> {
        Python::with_gil(|py| {
            let result = self.obj.bind(py).call_method1("read", (ctx.fd, len))
                .map_err(|e| DeviceError::Internal(e.to_string()))?;
            result.extract::<Vec<u8>>()
                .map_err(|e| DeviceError::Internal(e.to_string()))
        })
    }

    fn write(&mut self, ctx: &mut DeviceIoContext, data: &[u8]) -> Result<usize, DeviceError> {
        Python::with_gil(|py| {
            let result = self.obj.bind(py).call_method1(
                "write", (ctx.fd, PyBytes::new(py, data)),
            ).map_err(|e| DeviceError::Internal(e.to_string()))?;
            result.extract::<usize>()
                .map_err(|e| DeviceError::Internal(e.to_string()))
        })
    }

    fn close(&mut self, ctx: &mut DeviceCloseContext) -> Result<(), DeviceError> {
        Python::with_gil(|py| {
            self.obj.bind(py).call_method1("close", (ctx.fd,))
                .map_err(|e| DeviceError::Internal(e.to_string()))?;
            Ok(())
        })
    }
}

// ============================================================================
// FsProxy
// ============================================================================

#[pyclass]
#[derive(Clone)]
struct FsProxy {
    linux: Arc<Mutex<LinuxRuntime>>,
}

#[pymethods]
impl FsProxy {
    fn map_file(&self, path: String, source: &PyVirtFile) -> PyResult<()> {
        self.linux.lock().unwrap()
            .mount_file(&path, source.source.clone())
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
    }

    fn map_device(&self, path: String, cls: &Bound<'_, PyType>) -> PyResult<()> {
        let py_cls: Py<PyType> = cls.clone().unbind();
        let factory: DeviceFactory = Arc::new(move || {
            let instance: Py<PyAny> = Python::with_gil(|py| {
                py_cls.bind(py).call1(())
                    .map(|obj| obj.unbind())
                    .expect("failed to create Python device instance")
            });
            Box::new(PyDeviceAdapter { obj: instance })
        });

        // 通过 LinuxRuntime 的统一入口注册设备和挂载路径，
        // 确保 syscall openat 使用的是同一个 DeviceRegistry。
        self.linux.lock().unwrap()
            .mount_device(&path, factory)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
    }
}

// ============================================================================
// EngineHolder — 包装非 Send 的 Engine，安全边界收窄到此类型
// ============================================================================

/// 持有非 `Send` 的 [`Engine`] 实例的容器。
///
/// # 线程安全性
///
/// Unicorn 引擎内部使用 `Rc<RefCell<>>`，天然不是 `Send + Sync`。
/// `EngineHolder` 通过 `unsafe impl Send + Sync` 声明自身安全，
/// 前提是**仅在 Python GIL 线程中创建、访问和销毁**。
///
/// 此前提由以下事实保证：
/// - Python GIL 同一时刻只允许一个线程执行 Python 代码
/// - `#[pyclass]` 的方法调用始终在持有 GIL 的线程中发生
/// - `EngineHolder` 从不在线程间显式传输
///
/// # 显式关闭
///
/// Unicorn 引擎必须在宿主进程退出前显式关闭（调用 [`EngineHolder::close`]），
/// 不能依赖 Drop 自动释放。原因是 Python PyO3 FFI 对象的 Drop 时机
/// 可能在 Python 解释器 shutdown 之后，此时 Unicorn C 库的内部状态已不可用。
struct EngineHolder {
    engine: Option<Box<dyn Engine>>,
}

// SAFETY: EngineHolder 仅在 Python GIL 线程中使用，不会跨线程访问。
unsafe impl Send for EngineHolder {}
unsafe impl Sync for EngineHolder {}

impl EngineHolder {
    /// 显式关闭引擎：先 stop，再 drop。
    ///
    /// 调用后 engine 为空，后续任何 engine 操作都会 panic。
    /// 此方法幂等（重复调用无副作用）。
    fn close(&mut self) {
        if let Some(mut engine) = self.engine.take() {
            let _ = engine.emu_stop();
            // engine 在此 drop，Unicorn 内部资源在 Python 进程存活期间释放。
        }
    }
}

impl std::ops::Deref for EngineHolder {
    type Target = dyn Engine;
    fn deref(&self) -> &(dyn Engine + 'static) {
        self.engine.as_ref().expect("engine already closed").as_ref()
    }
}

impl std::ops::DerefMut for EngineHolder {
    fn deref_mut(&mut self) -> &mut (dyn Engine + 'static) {
        self.engine.as_mut().expect("engine already closed").as_mut()
    }
}

impl Drop for EngineHolder {
    fn drop(&mut self) {
        // 如果还未显式 close，在这里做最终清理。
        // 此时 Python 可能已 shutdown，emu_stop 可能失败但至少避免泄漏。
        if let Some(mut engine) = self.engine.take() {
            let _ = engine.emu_stop();
        }
    }
}

// ============================================================================
// PyRuntime
// ============================================================================

#[pyclass(name = "Runtime")]
struct PyRuntime {
    engine: EngineHolder,
    linux: Arc<Mutex<LinuxRuntime>>,
    graph: ModuleGraph,
    trampoline_mapped: bool,
}

/// 将 `BackendError` 转换为 `PyErr`。
fn backend_err(e: rundroid_backend::BackendError) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
}

#[pymethods]
impl PyRuntime {
    #[new]
    fn new(arch: &str, backend: &str, seed: u64) -> PyResult<Self> {
        if arch != "arm64" || backend != "unicorn" {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "only arch=arm64 backend=unicorn supported",
            ));
        }

        let unicorn = UnicornBackend::new();
        let mut engine = unicorn.open(rundroid_core::Arch::Arm64)
            .map_err(|e: rundroid_backend::BackendError| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
            })?;

        let mut linux = LinuxRuntime::new();
        linux.seed_rng(seed);

        let linux = Arc::new(Mutex::new(linux));

        // syscall hook
        let linux_hook = Arc::clone(&linux);
        engine.install_syscall_hook(Box::new(SyscallDispatcherPy { linux: linux_hook }))
            .map_err(|e: rundroid_backend::BackendError| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
            })?;

        // scratch buffer
        const SCRATCH_ADDR: u64 = 0x800_000;
        engine.mem_map(SCRATCH_ADDR, 0x10_0000, MemPerms::READ_WRITE)
            .map_err(|e: rundroid_backend::BackendError| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
            })?;

        Ok(Self {
            engine: EngineHolder { engine: Some(engine) },
            linux,
            graph: ModuleGraph::new(),
            trampoline_mapped: false,
        })
    }

    fn seed(&self, seed: u64) {
        self.linux.lock().unwrap().seed_rng(seed);
    }

    /// 显式关闭引擎，释放 Unicorn 内部资源。
    ///
    /// Python FFI 对象的 Drop 可能在解释器 shutdown 之后，
    /// 届时 Unicorn C 库内部状态不可用。因此必须在 Python 侧主动调用 `close()`。
    ///
    /// 调用后 Runtime 不可再使用。
    fn close(&mut self) {
        self.engine.close();
    }

    #[getter]
    fn fs(&self) -> FsProxy {
        FsProxy {
            linux: Arc::clone(&self.linux),
        }
    }

    /// 加载 ELF .so 模块。返回模块 ID（u64）。
    fn load(&mut self, name: String, bytes: Vec<u8>) -> PyResult<u64> {
        let parsed: ParsedElf = ElfCrateParser::new()
            .parse(ParseInput::new(&name, &bytes))
            .map_err(|e: rundroid_elf_parser::ElfParseError| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
            })?;

        if let Some(soname) = &parsed.dynamic.soname {
            if self.graph.by_soname.contains_key(soname) {
                return Ok(0);
            }
        }

        let module_id = rundroid_core::IdAllocator::new().module();
        let raw_id = module_id.raw();

        let mut reserve_cursor: u64 = 0x4000_0000;
        let module = {
            let engine_ref: &mut dyn Engine = &mut *self.engine;
            let mut load_ctx = LoadCtxAdapterPy {
                engine: engine_ref,
                next_reserve: &mut reserve_cursor,
            };
            DefaultLoader::new()
                .load(&mut load_ctx, &parsed, LoadRequest {
                    image_align: 0x1000,
                    bytes: &bytes,
                    module_id,
                })
                .map_err(|e: rundroid_elf_loader::ElfLoadError| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
                })?
        };

        // 收紧段权限
        for seg in &module.segments {
            let p = MemPerms::from_flags(seg.perms.read, seg.perms.write, seg.perms.execute);
            let _ = self.engine.mem_protect(seg.guest_addr, seg.size as usize, p);
        }

        let soname = parsed.dynamic.soname.clone();
        self.graph.insert(module, soname);

        // link
        {
            let engine_ref: &mut dyn Engine = &mut *self.engine;
            let graph_ptr: *mut ModuleGraph = &mut self.graph;
            let mut link_ctx = LinkCtxAdapterPy { engine: engine_ref, graph_ptr };
            DefaultLinker::new()
                .link_root(&mut link_ctx, &mut self.graph, module_id)
                .map_err(|e: rundroid_elf_linker::ElfLinkError| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
                })?;
        }

        Ok(raw_id)
    }

    /// 往 guest 内存地址写入字节。
    ///
    /// 用于在调用导出函数前设置 buffer 内容（例如把路径字符串写入 guest 内存）。
    fn write_guest(&mut self, addr: u64, data: Vec<u8>) -> PyResult<()> {
        self.engine.mem_write(addr, &data).map_err(backend_err)
    }

    /// 调用导出函数。
    #[pyo3(signature = (name, *args))]
    fn call(&mut self, name: String, args: &Bound<'_, PyTuple>) -> PyResult<u64> {
        // 从 PyTuple 提取 u64 参数（最多 8 个）。
        let mut regs = Vec::with_capacity(args.len().min(8));
        for item in args.iter().take(8) {
            let v: u64 = item.extract().map_err(|e: PyErr| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!("arg must be int: {e}"))
            })?;
            regs.push(v);
        }

        let entry_addr = self.graph.modules.values()
            .find_map(|m| m.exports.find(&name).map(|e| e.guest_addr))
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                format!("symbol `{name}` not found"),
            ))?;

        const SENTINEL_ADDR: u64 = 0x7F_FFFF_0000;
        const STACK_BASE: u64   = 0x7F_E000_0000;
        const STACK_TOP: u64    = STACK_BASE + 0x10_0000;

        if !self.trampoline_mapped {
            self.engine.mem_map(SENTINEL_ADDR, 0x1000, MemPerms::READ_EXEC)
                .map_err(backend_err)?;
            self.engine.mem_write(SENTINEL_ADDR, &[0xC0, 0x03, 0x5F, 0xD6])
                .map_err(backend_err)?;
            self.engine.mem_map(STACK_BASE, 0x10_0000, MemPerms::READ_WRITE)
                .map_err(backend_err)?;
            self.trampoline_mapped = true;
        }

        self.engine.reg_write(Arm64Reg::Sp, STACK_TOP).map_err(backend_err)?;
        self.engine.reg_write(Arm64Reg::Lr, SENTINEL_ADDR).map_err(backend_err)?;
        self.engine.reg_write(Arm64Reg::Pc, entry_addr).map_err(backend_err)?;
        for (i, v) in regs.iter().enumerate() {
            self.engine.reg_write(Arm64Reg::X(i as u8), *v).map_err(backend_err)?;
        }

        self.engine.emu_start(entry_addr, Some(SENTINEL_ADDR), None, None).map_err(backend_err)?;

        self.engine.reg_read(Arm64Reg::X(0))
            .map_err(|e| backend_err(e))
    }
}

// ============================================================================
// SyscallDispatcher
// ============================================================================

struct SyscallDispatcherPy {
    linux: Arc<Mutex<LinuxRuntime>>,
}

impl SyscallHook for SyscallDispatcherPy {
    fn on_svc(&mut self, cpu: &mut dyn SyscallCpu) {
        let nr = cpu.reg_read(Arm64Reg::X(8));
        let x0 = cpu.reg_read(Arm64Reg::X(0));
        let x1 = cpu.reg_read(Arm64Reg::X(1));
        let x2 = cpu.reg_read(Arm64Reg::X(2));
        let x3 = cpu.reg_read(Arm64Reg::X(3));
        let x4 = cpu.reg_read(Arm64Reg::X(4));
        let x5 = cpu.reg_read(Arm64Reg::X(5));

        let cpu_ptr: *mut dyn SyscallCpu = cpu as *mut dyn SyscallCpu;
        let mut read_guest = |addr: u64, len: usize| -> Option<Vec<u8>> {
            let mut buf = vec![0u8; len];
            if unsafe { (*cpu_ptr).mem_read(addr, &mut buf) } { Some(buf) } else { None }
        };
        let mut write_guest = |addr: u64, bytes: &[u8]| -> bool {
            unsafe { (*cpu_ptr).mem_write(addr, bytes) }
        };

        let result = {
            let mut linux = self.linux.lock().unwrap();
            linux.dispatch(nr, x0, x1, x2, x3, x4, x5, &mut read_guest, &mut write_guest)
        };

        match result {
            SyscallResult::Done(v) => { cpu.reg_write(Arm64Reg::X(0), v); }
            SyscallResult::Exit(_code) => { cpu.stop(); }
        }
    }
}

// ============================================================================
// LoadCtx / LinkCtx
// ============================================================================

struct LoadCtxAdapterPy<'a> {
    engine: &'a mut dyn Engine,
    next_reserve: &'a mut u64,
}

fn align_up(v: u64, a: u64) -> u64 {
    if a == 0 { v } else { (v + a - 1) & !(a - 1) }
}

impl<'a> LoadContext for LoadCtxAdapterPy<'a> {
    fn reserve_image_space(&mut self, size: u64, align: u64) -> Result<u64, MemoryError> {
        let aligned = align_up(size, 0x1000);
        let base = align_up(*self.next_reserve, align.max(0x1000));
        self.engine.mem_map(base, aligned as usize, MemPerms::ALL)
            .map_err(|_| MemoryError::InvalidSize { size: aligned, reason: "backend rejected" })?;
        *self.next_reserve = base + aligned;
        Ok(base)
    }

    fn map_segment(
        &mut self,
        spec: rundroid_elf_loader::SegmentMapSpec<'_>,
    ) -> Result<rundroid_elf_loader::MappedSegment, MemoryError> {
        Ok(rundroid_elf_loader::MappedSegment { guest_addr: spec.guest_addr, size: spec.size })
    }

    fn write_bytes(&mut self, guest_addr: u64, bytes: &[u8]) -> Result<(), MemoryError> {
        self.engine.mem_write(guest_addr, bytes)
            .map_err(|_| MemoryError::NotMapped { addr: guest_addr })
    }

    fn zero_fill(&mut self, guest_addr: u64, len: u64) -> Result<(), MemoryError> {
        let zeros = vec![0u8; len as usize];
        self.engine.mem_write(guest_addr, &zeros)
            .map_err(|_| MemoryError::NotMapped { addr: guest_addr })
    }

    fn emit(&mut self, _event: rundroid_telemetry::TelemetryEvent<'_>) {}
}

struct LinkCtxAdapterPy<'a> {
    engine: &'a mut dyn Engine,
    graph_ptr: *mut ModuleGraph,
}

impl<'a> LinkContext for LinkCtxAdapterPy<'a> {
    fn resolve(
        &self,
        query: SymbolQuery<'_>,
    ) -> Result<Option<ResolvedSymbol>, rundroid_elf_linker::ElfLinkError> {
        let graph: &ModuleGraph = unsafe { &*self.graph_ptr };
        Ok(rundroid_elf_linker::resolve(graph, query))
    }

    fn write_relocation(&mut self, patch: RelocationPatch) -> Result<(), MemoryError> {
        let bytes = patch.value.to_le_bytes();
        self.engine.mem_write(patch.target_addr, &bytes)
            .map_err(|_| MemoryError::NotMapped { addr: patch.target_addr })
    }

    fn protect_relro(&mut self, _module: rundroid_core::ModuleId) -> Result<(), MemoryError> {
        Ok(())
    }

    fn emit(&mut self, _event: rundroid_telemetry::TelemetryEvent<'_>) {}
}

// ============================================================================
// Python 模块
// ============================================================================

/// Python 侧通过 `import _rundroid` 导入此原生扩展模块。
#[pymodule]
fn _rundroid(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyRuntime>()?;
    m.add_class::<PyVirtFile>()?;
    Ok(())
}
