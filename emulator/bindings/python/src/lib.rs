//! `rundroid-bindings-python`
//!
//! PyO3 绑定层：把 Rust 侧的 Emulator / VFS / JNI shim 暴露给 Python。
//!
//! Python 模块名为 `_rundroid`（C 扩展），上层 `rundroid` 包在此之上提供
//! decorator、VirtFile 构造器等 Pythonic API。
//!
//! # 线程安全性说明
//!
//! PyEmulator 持有 `Box<dyn Engine>` 但 Engine 不是 Sync。
//! 以下 `unsafe impl Send/Sync` 的前提是：PyEmulator 仅在 Python GIL 线程中访问，
//! 不会跨线程共享 engine 引用。所有 engine 操作都在 Python 方法调用上下文中执行。

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple, PyType};
use pyo3::IntoPyObjectExt;
use rundroid_backend::{Arm64Reg, Backend, Engine, MemPerms, GuestCPU, SyscallHook};
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
use rundroid_jni::{
    FieldAccess,
    FieldSig,
    JniArgs,
    JniError,
    JniRegistry,
    JValue,
    MethodImpl,
    MethodSig,
    RefTable,
    SharedField,
};
use rundroid_linux::{LinuxRuntime, SyscallResult};
use rundroid_memory::MemoryError;
use std::collections::HashMap;
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
struct EngineHolder {
    engine: Option<Box<dyn Engine>>,
}

unsafe impl Send for EngineHolder {}
unsafe impl Sync for EngineHolder {}

impl EngineHolder {
    fn close(&mut self) {
        if let Some(mut engine) = self.engine.take() {
            let _ = engine.emu_stop();
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
        if let Some(mut engine) = self.engine.take() {
            let _ = engine.emu_stop();
        }
    }
}

// ============================================================================
// PyEmulator
// ============================================================================

/// Emulator 是 rundroid 的 Python 侧主入口对象。
///
/// 它装配 Unicorn 引擎、Linux syscall runtime、ELF 模块图和 JNI shim registry，
/// 为 Python 脚本层提供完整的 Android Native 执行环境。
#[pyclass(name = "Emulator")]
struct PyEmulator {
    engine: EngineHolder,
    linux: Arc<Mutex<LinuxRuntime>>,
    graph: ModuleGraph,
    trampoline_mapped: bool,
    /// JNI shim registry — class / method / field 注册表。
    jni_registry: JniRegistry,
    /// JNI 引用表 — handle → ObjectId。
    jni_refs: RefTable,
    /// Java 实例表：handle → Python 对象。
    /// 当 Python 侧 new_java_instance() 时创建，release_java_instance() 时清除。
    java_instances: HashMap<u32, Py<PyAny>>,
    /// 已注册的 Python class 类型：class_name → PyType，供 new_java_instance 实例化。
    class_types: HashMap<String, Py<PyType>>,
    /// 方法名映射：(class_name, java_method_name) → python_method_name。
    /// 因为 descriptor 中的 Java method 名（如 "Signature"）可能与
    /// Python 方法名（如 "signature_init"）不同。
    method_names: HashMap<(String, String), String>,
    /// 下一个实例 handle。
    next_instance_handle: u32,
}

fn backend_err(e: rundroid_backend::BackendError) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
}

#[pymethods]
impl PyEmulator {
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

        let linux_hook = Arc::clone(&linux);
        engine.install_syscall_hook(Box::new(SyscallDispatcherPy { linux: linux_hook }))
            .map_err(|e: rundroid_backend::BackendError| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
            })?;

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
            jni_registry: JniRegistry::new(),
            jni_refs: RefTable::new(),
            java_instances: HashMap::new(),
            class_types: HashMap::new(),
            method_names: HashMap::new(),
            next_instance_handle: 1,
        })
    }

    fn seed(&self, seed: u64) {
        self.linux.lock().unwrap().seed_rng(seed);
    }

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

        for seg in &module.segments {
            let p = MemPerms::from_flags(seg.perms.read, seg.perms.write, seg.perms.execute);
            let _ = self.engine.mem_protect(seg.guest_addr, seg.size as usize, p);
        }

        let soname = parsed.dynamic.soname.clone();
        self.graph.insert(module, soname);

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

    fn write_guest(&mut self, addr: u64, data: Vec<u8>) -> PyResult<()> {
        self.engine.mem_write(addr, &data).map_err(backend_err)
    }

    #[pyo3(signature = (name, *args))]
    fn call(&mut self, name: String, args: &Bound<'_, PyTuple>) -> PyResult<u64> {
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

    // ========================================================================
    // JNI shim — 注册 / 实例化 / 调用
    // ========================================================================

    /// 注册 Java shim class 的 method 和 field。
    ///
    /// 读取 class 上的 metadata 属性（`__java_class_name__`、`__java_methods__`、
    /// `__java_static_fields__`），解析 descriptor 并注册到 JNI registry。
    /// 同时保存 Python class 引用，供后续 `new_java_instance` 实例化。
    fn register_java_class(&mut self, cls: &Bound<'_, PyType>) -> PyResult<()> {
        let class_name: String = cls.getattr("__java_class_name__")
            .map_err(|_| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "class 缺少 @java_class decorator 注解"
            ))?
            .extract()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                format!("__java_class_name__ 不是有效的字符串: {e}")
            ))?;

        // 保存 Python class 类型引用
        self.class_types.insert(class_name.clone(), cls.clone().unbind());

        let mut class_def = rundroid_jni::JClassDef::new(rundroid_jni::ClassId(0), class_name.clone());

        // —— 注册 methods ——
        let methods = cls.getattr("__java_methods__")
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("读取 __java_methods__ 失败: {e}")
            ))?;
        let len = methods.len()? as usize;
        for i in 0..len {
            let item = methods.get_item(i)?;
            // Python tuple: (name, desc, func, is_static)
            let method_name: String = item.get_item(0)?.extract()
                .map_err(|e: PyErr| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    format!("无法提取 method name: {e}")
                ))?;
            let desc: String = item.get_item(1)?.extract()
                .map_err(|e: PyErr| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    format!("无法提取 method descriptor: {e}")
                ))?;
            let _py_fn_obj = item.get_item(2)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    format!("无法提取 method 函数: {e}")
                ))?;
            let is_static: bool = item.get_item(3)?
                .extract()
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    format!("无法提取 is_static: {e}")
                ))?;

            let mut sig = MethodSig::parse(&desc)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("method descriptor 解析失败: {e}")
                ))?;
            if sig.class.is_empty() {
                sig.class = class_name.clone();
            }

            // 保存 Java method 名 → Python method 名的映射
            self.method_names.insert(
                (class_name.clone(), sig.name.clone()),
                method_name.clone(),
            );

            // 实际的 method 分派通过 call_java_method() 直接 Python→Python，
            // Rust registry 只做签名记录占位。此处注册一个空 handler。
            let real_handler: Arc<dyn Fn(&JniArgs) -> Result<JValue, JniError> + Send + Sync> =
                Arc::new(move |_args: &JniArgs| -> Result<JValue, JniError> {
                    Ok(JValue::Void)
                });

            class_def.add_method(sig, is_static, MethodImpl::RustNative(real_handler))
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("method 注册失败: {e}")
                ))?;
        }

        // —— 注册 static fields ——
        let fields_val = cls.getattr("__java_static_fields__")
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("读取 __java_static_fields__ 失败: {e}")
            ))?;
        let flen = fields_val.len()? as usize;
        for i in 0..flen {
            let item = fields_val.get_item(i)?;
            let desc: String = item.get_item(1)?.extract()
                .map_err(|e: PyErr| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    format!("无法提取 field descriptor: {e}")
                ))?;
            let is_static: bool = item.get_item(2)?.extract()
                .map_err(|e: PyErr| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    format!("无法提取 is_static: {e}")
                ))?;
            let py_val: Py<PyAny> = item.get_item(3)?.into();
            let mut sig = FieldSig::parse(&desc)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("field descriptor 解析失败: {e}")
                ))?;
            if sig.class.is_empty() {
                sig.class = class_name.clone();
            }

            let jval = Python::with_gil(|py| -> Result<JValue, JniError> {
                let val = py_val.bind(py);
                if val.is_none() { Ok(JValue::Null) }
                else if let Ok(i) = val.extract::<i64>() { Ok(JValue::Long(i)) }
                else if let Ok(i) = val.extract::<i32>() { Ok(JValue::Int(i)) }
                else if let Ok(b) = val.extract::<bool>() { Ok(JValue::Boolean(b)) }
                else if let Ok(f) = val.extract::<f64>() { Ok(JValue::Double(f)) }
                else { Err(JniError::Internal("不支持的 field 初始值类型".to_string())) }
            }).map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                format!("field 初始值转换失败: {e}")
            ))?;

            let access = FieldAccess::RustNative(Arc::new(SharedField::new(jval)));
            class_def.add_field(sig, is_static, access)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("field 注册失败: {e}")
                ))?;
        }

        self.jni_registry.register_class(class_def)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("class 注册失败: {e}")
            ))?;

        Ok(())
    }

    /// 创建 Java 对象实例。
    ///
    /// 根据已注册的 class name，在 Python 侧实例化对应的 shim class
    /// （调用 `__init__`），存储到实例表并返回 handle。
    ///
    /// handle 是一个 u32 整数，后续 `call_java_method` 通过它定位实例。
    fn new_java_instance(&mut self, class_name: &str) -> PyResult<u32> {
        let py_cls = self.class_types.get(class_name)
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("class `{class_name}` 尚未注册")
            ))?;

        let instance = Python::with_gil(|py| -> PyResult<Py<PyAny>> {
            let obj = py_cls.bind(py).call1(())
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("实例化 `{class_name}` 失败: {e}")
                ))?;
            Ok(obj.unbind())
        })?;

        let handle = self.next_instance_handle;
        self.next_instance_handle += 1;
        self.java_instances.insert(handle, instance);

        Ok(handle)
    }

    /// 获取 Python 实例对象（供 Python 侧直接操作）。
    fn java_instance(&self, handle: u32) -> PyResult<PyObject> {
        let obj = self.java_instances.get(&handle)
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("实例 handle {handle} 不存在")
            ))?;
        Python::with_gil(|py| Ok(obj.clone_ref(py).into()))
    }

    /// 释放 Java 实例——当 native/guest 代码通过 JNI 释放对象引用时调用。
    ///
    /// 调用后 handle 失效，后续通过该 handle 的操作返回错误。
    fn release_java_instance(&mut self, handle: u32) -> PyResult<()> {
        self.java_instances.remove(&handle)
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("实例 handle {handle} 不存在")
            ))?;
        Ok(())
    }

    /// 调用已注册 Java 实例的方法。
    ///
    /// 参数：
    /// - `handle`: `new_java_instance` 返回的实例句柄（0 表示 static method）
    /// - `method_desc`: method descriptor（如 `"hashCode()I"` 或 `"Signature([B)V"`）
    /// - `args`: Python tuple 参数列表
    ///
    /// 方法通过 Python 直接分派到实例上，绕过 Rust registry dispatch。
    #[pyo3(signature = (handle, method_desc, args))]
    fn call_java_method(
        &self,
        handle: u32,
        method_desc: &str,
        args: &Bound<'_, PyTuple>,
    ) -> PyResult<PyObject> {
        let sig = MethodSig::parse(method_desc)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("method descriptor 解析失败: {e}")
            ))?;
        let java_name = sig.name;

        // 查找实例所属的 class（从对象的 class name 推导）
        // 简单实现：遍历 class_types 找到第一个 class
        // 实际上应该从实例本身获取 class name，这里用 descriptor 中的 class
        let class_name = if !sig.class.is_empty() {
            sig.class.clone()
        } else {
            // 从 method_names 反查
            self.method_names.iter()
                .find(|((_cn, jn), _)| jn == &java_name)
                .map(|((cn, _), _)| cn.clone())
                .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("无法找到 Java method `{java_name}` 的注册 class")
                ))?
        };
        let lookup_key = (class_name.clone(), java_name.clone());
        let method_name = self.method_names.get(&lookup_key)
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("method `{java_name}` 在 class `{class_name}` 中未注册")
            ))?;

        // 查找实例
        let instance = self.java_instances.get(&handle)
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("实例 handle {handle} 不存在")
            ))?;

        // 将 PyTuple args 转换为 Python 参数格式
        Python::with_gil(|py| {
            // 构造调用参数：(self,) + args
            let bound = instance.bind(py);
            match args.len() {
                0 => {
                    bound.call_method0(&method_name)
                        .map(|r| r.into())
                        .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("method `{method_name}` 调用失败: {e}")
                        ))
                }
                1 => {
                    let a0 = args.get_item(0)?;
                    bound.call_method1(&method_name, (a0,))
                        .map(|r| r.into())
                        .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("method `{method_name}` 调用失败: {e}")
                        ))
                }
                _ => {
                    bound.call_method(&method_name, args, None)
                        .map(|r| r.into())
                        .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("method `{method_name}` 调用失败: {e}")
                        ))
                }
            }
        })
    }

    /// 读取已注册的 static field 值。
    #[pyo3(signature = (class_name, field_desc))]
    fn read_java_field(&self, class_name: &str, field_desc: &str) -> PyResult<PyObject> {
        let mut sig = FieldSig::parse(field_desc)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("field descriptor 解析失败: {e}")
            ))?;
        if sig.class.is_empty() {
            sig.class = class_name.to_string();
        }

        let result = self.jni_registry.dispatch_static_field_get(&sig)
            .or_else(|_| self.jni_registry.dispatch_field_get(&sig));

        jvalue_to_pyobject(result)
    }

    /// 获取实例的 Python 属性（field 值）。
    fn read_instance_field(&self, handle: u32, field_name: &str) -> PyResult<PyObject> {
        let instance = self.java_instances.get(&handle)
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("实例 handle {handle} 不存在")
            ))?;

        Python::with_gil(|py| {
            instance.bind(py).getattr(field_name)
                .map(|r| r.into())
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("读取实例 field `{field_name}` 失败: {e}")
                ))
        })
    }
}

/// 将 JValue Result 转为 Python 对象。
fn jvalue_to_pyobject(result: Result<JValue, JniError>) -> PyResult<PyObject> {
    match result {
        Ok(val) => Python::with_gil(|py| {
            let obj: PyObject = match val {
                JValue::Int(i) => i.into_py_any(py).unwrap(),
                JValue::Long(l) => l.into_py_any(py).unwrap(),
                JValue::Float(f) => f.into_py_any(py).unwrap(),
                JValue::Double(d) => d.into_py_any(py).unwrap(),
                JValue::Boolean(b) => b.into_py_any(py).unwrap(),
                JValue::Void | JValue::Null => py.None(),
                JValue::Object(id) => id.0.into_py_any(py).unwrap(),
                _ => py.None(),
            };
            Ok(obj)
        }),
        Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
            format!("JNI 调用失败: {e}")
        )),
    }
}

// ============================================================================
// SyscallDispatcher
// ============================================================================

struct SyscallDispatcherPy {
    linux: Arc<Mutex<LinuxRuntime>>,
}

impl SyscallHook for SyscallDispatcherPy {
    fn on_svc(&mut self, cpu: &mut dyn GuestCPU) {
        let nr = cpu.reg_read(Arm64Reg::X(8));
        let x0 = cpu.reg_read(Arm64Reg::X(0));
        let x1 = cpu.reg_read(Arm64Reg::X(1));
        let x2 = cpu.reg_read(Arm64Reg::X(2));
        let x3 = cpu.reg_read(Arm64Reg::X(3));
        let x4 = cpu.reg_read(Arm64Reg::X(4));
        let x5 = cpu.reg_read(Arm64Reg::X(5));

        let cpu_ptr: *mut dyn GuestCPU = cpu as *mut dyn GuestCPU;
        let mut read_guest = |addr: u64, len: usize| -> Option<Vec<u8>> {
            let mut buf = vec![0u8; len];
            if unsafe { (*cpu_ptr).mem_read(addr, &mut buf) } { Some(buf) } else { None }
        };
        let mut write_guest = |addr: u64, bytes: &[u8]| -> bool {
            unsafe { (*cpu_ptr).mem_write(addr, bytes) }
        };
        let mut map_guest = |addr: u64, len: usize, prot: i32| -> bool {
            use rundroid_backend::MemPerms;
            let read = (prot & 1) != 0;
            let write = (prot & 2) != 0;
            let exec = (prot & 4) != 0;
            let perms = MemPerms::from_flags(read, write, exec);
            unsafe { (*cpu_ptr).mem_map(addr, len, perms).is_ok() }
        };

        let result = {
            let mut linux = self.linux.lock().unwrap();
            linux.dispatch(
                nr, x0, x1, x2, x3, x4, x5, &mut read_guest, &mut write_guest, &mut map_guest,
            )
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

#[pymodule]
fn _rundroid(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyEmulator>()?;
    m.add_class::<PyVirtFile>()?;
    Ok(())
}
