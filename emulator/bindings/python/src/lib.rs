//! `rundroid-bindings-python`
//!
//! PyO3 绑定层：把 Rust 侧的 Emulator / VFS / JNI shim 暴露给 Python。
//!
//! Python 模块名为 `_rundroid`（C 扩展），上层 `rundroid` 包在此之上提供
//! decorator、VirtFile 构造器等 Pythonic API。
//!
//! # 线程安全性说明
//!
//! PyEmulatorBridge 持有 `Box<dyn Engine>` 但 Engine 不是 Sync。
//! 以下 `unsafe impl Send/Sync` 的前提是：PyEmulatorBridge 仅在 Python GIL 线程中访问，
//! 不会跨线程共享 engine 引用。所有 engine 操作都在 Python 方法调用上下文中执行。

mod javashim;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple, PyType};
use rundroid_backend::{Arm64Reg, Backend, Engine, MemPerms, GuestCPU, SyscallHook};
use rundroid_backend_unicorn::UnicornBackend;
use rundroid_core::IdAllocator as ModuleIdAllocator;
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
    AndroidVM,
    ClassId,
    FieldAccess,
    FieldSig,
    IdAllocator,
    JMethodDef,
    JNIEnvABI,
    JavaVMABI,
    JniArgs,
    JniError,
    JType,
    JValue,
    MethodImpl,
    MethodSig,
    ObjectId,
    ObjectStorage,
    ObjectStore,
    PythonCallableAnnotations,
    SharedField,
    validate_jni_version,
};
use rundroid_jni_trampoline::JniTrampolineHook;
use rundroid_linux::{LinuxRuntime, MemoryBridge, SyscallResult};
use rundroid_memory::MemoryError;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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
// PythonShimAdapter — Python shim 到 Rust runtime 的 adapter
// ============================================================================

/// Python shim 到 Rust runtime 的 adapter。
///
/// 持有 Python 侧分派所需的缓存映射，
/// 但不是 class/method/object 的 authority。
///
/// # 线程安全性
///
/// `method_names` 中的字符串仅在 Python GIL 线程中访问。
struct PythonShimAdapter {
    /// (class_name, java_method_name, argc) → python_method_name，
    /// 供 `call_java_method` 判定是否存在 Python instance-method override。
    ///
    /// 键含 argc（descriptor 的参数个数）：同名不同签名的重载（如 `foo()I` vs `foo(I)I`）
    /// 互不干扰——只有签名匹配（这里以 argc 区分，与 Python 侧 argc 分派一致）的 override
    /// 才命中 Python 路径，否则回落 framework stub。静态方法不进此表（静态方法不在
    /// `__java_dispatch__`，走 dispatch_call → wrap_python_method 的 static 分支）。
    method_names: HashMap<(String, String, usize), String>,
    /// 共享 ObjectStore 引用，供 `wrap_python_method` 闭包通过 ObjectId 查找 Python 实例。
    objects: Arc<Mutex<ObjectStore>>,
}

impl PythonShimAdapter {
    /// 创建 adapter。
    fn new(objects: Arc<Mutex<ObjectStore>>) -> Self {
        Self {
            method_names: HashMap::new(),
            objects,
        }
    }

    /// 插入方法名映射（instance method 用，argc = descriptor 参数个数）。
    fn insert_method_name(
        &mut self,
        class_name: String,
        java_method_name: String,
        argc: usize,
        python_method_name: String,
    ) {
        self.method_names.insert(
            (class_name, java_method_name, argc),
            python_method_name,
        );
    }

    /// 查找是否存在签名匹配的 Python override（仅作存在性判定）。
    fn resolve_method_name(
        &self,
        class_name: &str,
        java_method_name: &str,
        argc: usize,
    ) -> Option<&str> {
        self.method_names
            .get(&(class_name.to_string(), java_method_name.to_string(), argc))
            .map(|s| s.as_str())
    }
}

// ============================================================================
// PyEmulatorBridge
// ============================================================================

/// Emulator 是 rundroid 的 Python 侧主入口对象。
///
/// 它装配 Unicorn 引擎、Linux syscall runtime、ELF 模块图和 Android VM，
/// 为 Python 脚本层提供完整的 Android Native 执行环境（含 JNI guest 执行）。
///
/// # JNI 权威归属
///
/// class / method / field / object 的最终权威存储在 `vm: Arc<Mutex<AndroidVM>>` 中。
/// Python `register()` 和 Rust builtin（framework stub）都进入同一套 [`AndroidVM`]。
/// JNI trampoline hook 持有同一 VM 的 `Arc::clone`——绑定层与 hook 共享一个 VM。
///
/// 以下字段仅为 **binding adapter cache**，不是 authority：
/// - `shim.method_names` — 仅用于 Python override 优先分派路径（`call_java_method`）
///
/// # 重入约束（单线程仿真的内在限制）
///
/// guest JNI dispatch 在 `emu_start` 期间触发 trampoline hook，hook 持守 VM Mutex。
/// 触发到 Python `@java_method` override 时，该 override **不得**再入 VM
/// （`avm.new_object` / `emulator.call`）——否则与 hook 持守的 Mutex 自锁死锁。
/// Python override 必须是纯计算（读字段、算返回值）。详见各 change 的 spec。
#[pyclass(name = "Emulator")]
struct PyEmulatorBridge {
    engine: EngineHolder,
    linux: Arc<Mutex<LinuxRuntime>>,
    /// 共享模块 ID 分配器。
    ///
    /// 同一个 Python Emulator 允许连续 `load()` 多个不同模块，
    /// 因此 ModuleId 必须来自同一个单调递增编号空间。
    /// 若每次 `load()` 都新建分配器，会重复生成 `ModuleId(1)`，
    /// 使 `ModuleGraph` 中后插入模块覆盖先前模块。
    module_id_alloc: ModuleIdAllocator,
    /// ELF 镜像保留区游标。
    ///
    /// Python 绑定层允许同一个 Emulator 连续 `load()` 多个 so，
    /// 因此镜像基址必须来自同一条 bump 分配游标。
    /// 若每次 `load()` 都从固定地址重新开始，第二个模块会与第一个模块重叠映射。
    reserve_cursor: u64,
    graph: ModuleGraph,
    trampoline_mapped: bool,
    /// Android VM — class / method / field / object 的 canonical authority。
    ///
    /// `Arc<Mutex<AndroidVM>>`：JNI trampoline hook 钳死要 `Arc<Mutex<AndroidVM>>`
    /// （hook 是 `Box<dyn CodeHook>` 存在 engine 里，在 `emu_start` 期间触发，
    /// 必须捕获能跨任何 `&self` 借用存活的 VM 句柄）。绑定层与 hook 经 `Arc::clone`
    /// 共享同一个 VM，注册的 class 对 guest JNI dispatch 可见（同一 registry）。
    ///
    /// host 侧方法（`call_java_method` 等）访问 VM 时**调用 Python 前必须释放 guard**，
    /// 否则方法体内 `new_object` 重入会与持守的 Mutex 自锁（同 unidbg 单线程语义）。
    vm: Arc<Mutex<AndroidVM>>,
    /// 共享 ObjectId 分配器——与 `vm.object_id_alloc` 同源。
    ///
    /// marshalling 闭包（`wrap_python_method`）与 `register_java_builtin` /
    /// `call_java_method_typed` 等路径捕获此 Arc，为 `str`/`bytes` 自动 coercion
    /// 及显式 wrapper 构造分配 `ObjectId`，确保与 VM 分配的对象 ID 同空间。
    id_alloc: Arc<Mutex<IdAllocator>>,
    /// Python shim adapter — 持有 Python 侧实例化、分派所需的缓存映射，
    /// 但不是 class/method/object 的 authority。
    shim: PythonShimAdapter,
    /// JNI verbose trace 共享开关——trampoline hook 安装后仍可经 `set_jni_verbose` toggle。
    jni_verbose: Arc<AtomicBool>,
    /// JNIEnv guest 指针缓存（`init_jni` 后填充；`jni_env_pointer` 返回此值）。
    jni_env_ptr: Option<u64>,
    /// JavaVM guest 指针缓存（`init_jni` 后填充；`jni_onload` / `java_vm_pointer` 用）。
    jni_vm_ptr: Option<u64>,
}

fn backend_err(e: rundroid_backend::BackendError) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
}

#[pymethods]
impl PyEmulatorBridge {
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

        // VM 用 Arc<Mutex<AndroidVM>> 包裹：JNI trampoline hook 与绑定层共享同一 VM。
        // 取出内部独立 Arc<Mutex>（objects / id_alloc）clone 给 shim / id_alloc 字段，
        // 它们与 VM 内的字段同源（指向同一份 ObjectStore / IdAllocator），
        // 供 marshalling 闭包捕获、不持 VM 锁即可分配 ObjectId。
        let vm = Arc::new(Mutex::new(AndroidVM::new()));
        let (objects, id_alloc) = {
            let vm_guard = vm.lock().unwrap();
            (Arc::clone(&vm_guard.objects), Arc::clone(&vm_guard.object_id_alloc))
        };
        Ok(Self {
            engine: EngineHolder { engine: Some(engine) },
            linux,
            module_id_alloc: ModuleIdAllocator::new(),
            reserve_cursor: 0x4000_0000,
            graph: ModuleGraph::new(),
            trampoline_mapped: false,
            vm,
            id_alloc,
            shim: PythonShimAdapter::new(objects),
            jni_verbose: Arc::new(AtomicBool::new(false)),
            jni_env_ptr: None,
            jni_vm_ptr: None,
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

        let module_id = self.module_id_alloc.module();
        let raw_id = module_id.raw();

        let module = {
            let engine_ref: &mut dyn Engine = &mut *self.engine;
            let mut load_ctx = LoadCtxAdapterPy {
                engine: engine_ref,
                next_reserve: &mut self.reserve_cursor,
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

        self.call_guest(entry_addr, &regs)
    }

    // ========================================================================
    // JNI shim — 注册 / 实例化 / 调用
    // ========================================================================

    /// 注册 Java shim class 的 method 和 field。
    ///
    /// 读取 class 上的 metadata 属性（`__java_class_name__`、`__java_methods__`、
    /// `__java_static_fields__`），解析 descriptor，提取 Python 类型注解，
    /// 做 strict verify，然后将 method/field 注册到 AndroidVM。
    ///
    /// # 注册流程
    ///
    /// 1. 读取 `__java_class_name__`
    /// 2. 遍历 `__java_methods__`，对每个 method：
    ///    a. 解析 descriptor → MethodSig
    ///    b. 从 Python 函数提取 type annotations → PythonCallableAnnotations
    ///    c. verify annotations vs descriptor（不匹配则 fail-fast）
    ///    d. 用 `javashim::wrap_python_method` 创建真实 handler
    ///    e. 注册到 class_def
    /// 3. 遍历 `__java_static_fields__`，解析并注册 field
    /// 4. 通过 `register_or_merge_class` 注册到 AndroidVM
    ///    （若 class 已存在则 merge：Python override 替换已有，未覆盖部分保留）
    fn register_java_class(&mut self, cls: &Bound<'_, PyType>) -> PyResult<()> {
        let class_name: String = cls.getattr("__java_class_name__")
            .map_err(|_| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "class 缺少 @java_class decorator 注解"
            ))?
            .extract()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                format!("__java_class_name__ 不是有效的字符串: {e}")
            ))?;

        let mut class_def = rundroid_jni::JClassDef::new(ClassId(0), class_name.clone());

        // —— 可选 superclass ——
        // Python @java_class(name, superclass="...") 声明的继承关系落到 JClassDef.superclass，
        // 使 registry 的继承链解析（class_chain / resolve_inherited_method /
        // resolve_method_by_id）能沿父类回退——guest GetMethodID/Call*Method 据此解析
        // 子类继承自父类的方法。无 __java_superclass__ 或空串 → 不设置（默认 java/lang/Object）。
        let superclass: Option<String> = cls.getattr("__java_superclass__").ok()
            .and_then(|v| v.extract().ok());
        if let Some(sup) = superclass.filter(|s| !s.is_empty()) {
            class_def.superclass = Some(sup);
        }

        // —— 注册 methods ——
        let methods = cls.getattr("__java_methods__")
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("读取 __java_methods__ 失败: {e}")
            ))?;
        let len = methods.len()? as usize;
        for i in 0..len {
            let item = methods.get_item(i)?;
            // Python tuple: (python_name, descriptor, py_fn, is_static)
            let method_name: String = item.get_item(0)?.extract()
                .map_err(|e: PyErr| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    format!("无法提取 method name: {e}")
                ))?;
            let desc: String = item.get_item(1)?.extract()
                .map_err(|e: PyErr| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    format!("无法提取 method descriptor: {e}")
                ))?;
            let py_fn_obj: Py<PyAny> = item.get_item(2)?.into();
            let is_static: bool = item.get_item(3)?
                .extract()
                .map_err(|e: PyErr| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    format!("无法提取 is_static: {e}")
                ))?;

            let mut sig = MethodSig::parse(&desc)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("method descriptor 解析失败: {e}")
                ))?;
            if sig.class.is_empty() {
                sig.class = class_name.clone();
            }

            // —— 严格校验：Python 注解 vs Java descriptor ——
            // 从 Python 函数提取 return type 和 param type 的 JNI descriptor
            Python::with_gil(|py| {
                let verify_module = PyModule::import(py, "rundroid.javashim.verify")
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyImportError, _>(
                        format!("无法导入 verify 模块: {e}")
                    ))?;

                let ret_jni_raw: Option<String> = verify_module
                    .call_method1("get_return_type_jni", (py_fn_obj.clone_ref(py),))
                    .ok()
                    .and_then(|r| r.extract::<Option<String>>().ok().flatten());

                let param_jni_raw: Vec<String> = verify_module
                    .call_method1("get_param_types_jni", (py_fn_obj.clone_ref(py),))
                    .ok()
                    .and_then(|r| r.extract::<Vec<String>>().ok())
                    .unwrap_or_default();

                // 如果 Python 函数有 type hint，做 strict verify
                if let Some(ret_str) = ret_jni_raw {
                    if !ret_str.is_empty() {
                        // 解析 JNI descriptor → JType
                        let ret_type = parse_jtype_from_descriptor(&ret_str)
                            .unwrap_or_else(|| sig.ret.clone());
                        let param_types: Vec<rundroid_jni::JType> = param_jni_raw.iter()
                            .filter_map(|s| parse_jtype_from_descriptor(s))
                            .collect();

                        let annotations = PythonCallableAnnotations::new(ret_type, param_types);
                        annotations.verify(&sig)
                            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                                format!("注解校验失败: class=`{}`, member=`{}`, {e}",
                                    class_name, sig.name)
                            ))?;
                    }
                }

                Ok::<_, PyErr>(())
            })?;

            // 记录 instance-method override（adapter cache，非 authority）：
            // 只缓存 instance method（静态方法不在 __java_dispatch__，走 dispatch_call），
            // 键含 argc 以区分同名不同签名的重载。
            if !is_static {
                self.shim.insert_method_name(
                    class_name.clone(),
                    sig.name.clone(),
                    sig.args.len(),
                    method_name.clone(),
                );
            }

            // 用 javashim bridge 创建真实 handler（带运行时返回值校验）
            let sig_ret = sig.ret.clone();
            let objects = Arc::clone(&self.shim.objects);
            let id_alloc = Arc::clone(&self.id_alloc);
            let real_handler = javashim::wrap_python_method(
                py_fn_obj, sig.name.clone(), sig_ret, objects, id_alloc,
            );

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

        // 注册到 AndroidVM（若 class 已存在则 merge）——与 framework stub、guest JNI
        // dispatch 共用同一 registry（VM.classes），register 后即对 guest JNI 可见。
        self.vm.lock().unwrap().classes.register_or_merge_class(class_def)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("class 注册失败: {e}")
            ))?;

        Ok(())
    }

    /// 注册一个**已创建**的 Python 对象（JavaObject）到 Rust VM。
    ///
    /// 这是对象 → VM 注册的**唯一**入口（构造由 Python 侧 `avm.new_object` 驱动，
    /// 不再由 binding 按 class_name 内部实例化）。
    ///
    /// 流程：
    /// 1. ObjectId 由共享 `IdAllocator` 分配（与 VM 内 `object_id_alloc` 同源），
    ///    而非 binding 自有的计数器——ObjectId 归 AVM 层，与 marshalling 产物统一空间。
    /// 2. 存入 `ObjectStore`（`HostValue` 持有 Python 对象引用 `Py<PyAny>`）。
    /// 3. 经 `RefTable::new_global` 分配全局 handle（JNI `jobject` 等价物）。
    ///
    /// # 生命周期
    ///
    /// Python 对象引用存储在 `ObjectStore` 的 `HostValue` 中。
    /// 调用 `release_java_instance` 时从 `ObjectStore` 移除并 drop Python 引用。
    /// handle 是 u32 整数，后续 `call_java_method` 通过它定位实例。
    ///
    /// 取 `&self`（而非 `&mut self`）：本方法会被 Python 方法体经
    /// `self._avm.new_object(...)` 递归回调——此时外层 `call_java_method`（`&self`，
    /// pyo3 `PyRef`）仍在栈上，若本方法也是 `&self` 则两个 `PyRef` 可共存。
    ///
    /// 三步分别持各自独立的锁（`id_alloc` / `shim.objects` 是内层 `Arc<Mutex>`，
    /// 与 VM Mutex 无嵌套），VM Mutex 只在分配 global ref handle（`vm.refs`）时短暂
    /// 持有；本方法不调 Python，故持锁安全。
    ///
    /// 返回 `(global_handle, object_id)`：handle 供 guest JNI 引用；
    /// object_id 回填到 JavaObject 的 `_rundroid_oid`，使 marshalling 能识别已注册对象
    /// （见 `javashim::py_to_jvalue` 的 `_rundroid_oid` 分支），从而让 JavaObject 可跨
    /// Python↔Rust 编组边界（作参数/返回值），与 `JavaString`/`JavaByteArray` 一致。
    fn register_java_object(&self, class_name: &str, py_obj: Py<PyAny>) -> PyResult<(u32, u64)> {
        // 1. 由共享 IdAllocator 分配 ObjectId（独立 Arc<Mutex>，不持 VM 锁）
        let object_id = self.id_alloc.lock().unwrap().object();

        // 2. 存入 ObjectStore（独立 Arc<Mutex>，不持 VM 锁）
        self.shim.objects.lock().unwrap().insert(
            object_id,
            class_name.to_string(),
            ObjectStorage::HostValue { data: Box::new(py_obj) },
        ).map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
            format!("ObjectStore 插入失败: {e}")
        ))?;

        // 3. 分配 global ref handle（refs 是 VM 直接字段，短暂持 VM 锁，不调 Python）
        let handle = self.vm.lock().unwrap().refs.new_global(object_id);

        Ok((handle, object_id.0))
    }

    /// 注册一个 `java/lang/String` 内置对象（用于显式 `JavaString` wrapper）。
    ///
    /// 返回 `(global_handle, object_id)`：handle 供 guest JNI 引用，
    /// object_id 供 Python 侧 marshalling 复用身份（存入 wrapper 的 `_rundroid_oid`）。
    fn register_java_string(&self, value: String) -> PyResult<(u32, u64)> {
        let (handle, oid) = self.register_builtin(
            "java/lang/String",
            ObjectStorage::String(value),
        )?;
        Ok((handle, oid.0))
    }

    /// 注册一个 `byte[]` 内置对象（用于显式 `JavaByteArray` wrapper）。
    ///
    /// 返回 `(global_handle, object_id)`，语义同 [`Self::register_java_string`]。
    fn register_java_bytes(&self, value: Vec<u8>) -> PyResult<(u32, u64)> {
        let elements = value.into_iter().map(|b| JValue::Byte(b as i8)).collect();
        let (handle, oid) = self.register_builtin(
            "[B",
            ObjectStorage::PrimitiveArray { jtype: JType::Byte, elements },
        )?;
        Ok((handle, oid.0))
    }

    /// 获取 Python 实例对象（供 Python 侧直接操作）。
    ///
    /// 通过 `RefTable::resolve` → `ObjectStore::storage` 查找，
    /// 从 `HostValue` 中取出 Python 对象引用。
    fn java_instance(&self, handle: u32) -> PyResult<PyObject> {
        let vm = self.vm.lock().unwrap();
        let object_id = vm.refs.resolve(handle)
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("实例 handle {handle} 不存在")
            ))?;

        match vm.objects.lock().unwrap().storage(object_id) {
            Some(ObjectStorage::HostValue { data }) => {
                let py_obj: &Py<PyAny> = data.downcast_ref::<Py<PyAny>>()
                    .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                        "HostValue 中的数据类型不是 Py<PyAny>"
                    ))?;
                // clone_ref 仅 refcount，不回调 engine，持守 read guard 安全
                Python::with_gil(|py| Ok(py_obj.clone_ref(py).into()))
            }
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("handle {handle} 对应的对象不是 Python 实例")
            )),
        }
    }

    /// 释放 Java 实例——同时清理 ObjectStore 和 RefTable。
    ///
    /// 1. 从 `RefTable` 解析 handle → `ObjectId`
    /// 2. 从 `ObjectStore` 移除对象（从而 drop Python 对象引用）
    /// 3. 从 `RefTable` 删除 global ref
    ///
    /// 调用后 handle 失效，后续通过该 handle 的操作返回错误。
    fn release_java_instance(&mut self, handle: u32) -> PyResult<()> {
        let mut vm = self.vm.lock().unwrap();
        let object_id = vm.refs.resolve(handle)
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("实例 handle {handle} 不存在于 RefTable")
            ))?;

        // 从 ObjectStore 移除（drop HostValue → Python 对象引用失效）
        vm.objects.lock().unwrap().remove(object_id)
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("ObjectId {object_id} 不在 ObjectStore 中")
            ))?;

        // 从 RefTable 删除 global ref
        vm.refs.delete_global(handle)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("删除 ref handle 失败: {e}")
            ))?;

        Ok(())
    }

    /// 调用已注册 Java 实例的方法。
    ///
    /// # 统一 dispatch 路径
    ///
    /// 1. `RefTable::resolve(handle)` → `ObjectId`
    /// 2. `ObjectStore::storage(ObjectId)` → 取出对象数据
    /// 3. 根据 storage 变体分派：
    ///    - **HostValue**：取出 `Py<PyAny>` → 查 `PythonShimAdapter::resolve_method_name`
    ///      → `Python::with_gil` 直接调用 Python 方法（Python override 路径）
    ///    - **StubInstance**：走 `runtime.classes().dispatch_call()`
    ///      → Rust registry dispatch（framework stub 回落路径）
    ///
    /// 参数：
    /// - `handle`: `register_java_object`（经 `avm.new_object`）返回的实例句柄
    /// - `method_desc`: method descriptor（如 `"hashCode()I"`）
    /// - `args`: Python tuple 参数列表
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
        let java_name = sig.name.clone();
        let argc = sig.args.len();

        // 1. 解析 handle → ObjectId（短暂持 VM 锁，解析后即释放）
        let object_id = self.vm.lock().unwrap().refs.resolve(handle)
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("实例 handle {handle} 不存在于 RefTable")
            ))?;

        // 2. 锁内：只取出 dispatch 判定所需的 owned 数据，**立即释放锁**。
        //    关键：必须在任何 Python 调用前释放 objects 锁——方法体内可能
        //    `self._avm.new_object(...)` → `register_java_object` 再次锁 objects，
        //    若持锁进 Python 会自锁死锁。故这里只 clone 出 Py 引用（refcount，廉价）
        //    与 class_name，锁随 block 结束释放。
        //
        // 分派目标（owned，可在锁外使用）。
        enum CallTarget {
            HostValue { py_obj: Py<PyAny>, class_name: String },
            StubInstance(String),
            Unsupported(&'static str),
        }
        // objects 是内层独立 Arc<Mutex>（与 VM 内字段同源），不持 VM 锁即可访问。
        let objects = Arc::clone(&self.shim.objects);
        let target = {
            let store = objects.lock().unwrap();
            let storage = store.storage(object_id)
                .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("ObjectId {object_id} 不在 ObjectStore 中")
                ))?;
            match storage {
                ObjectStorage::HostValue { data } => {
                    let py_ref: &Py<PyAny> = data.downcast_ref::<Py<PyAny>>()
                        .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            "HostValue 中的数据类型不是 Py<PyAny>"
                        ))?;
                    let class_name = store.class_name(object_id)
                        .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                            "无法获取对象的 class name"
                        ))?
                        .to_string();
                    // clone Py 引用（refcount，不触发 Python 方法调用，持锁安全）
                    let py_obj = Python::with_gil(|py| py_ref.clone_ref(py));
                    CallTarget::HostValue { py_obj, class_name }
                }
                ObjectStorage::StubInstance { .. } => {
                    let cn = store.class_name(object_id).unwrap_or("<unknown>").to_string();
                    CallTarget::StubInstance(cn)
                }
                other => CallTarget::Unsupported(other.kind_label()),
            }
        }; // ← store 锁在此释放

        // 3. 锁外分派（Python 调用 / framework 回落都可安全重入 objects 锁）
        match target {
            // —— Python override（HostValue）——
            CallTarget::HostValue { py_obj, class_name } => {
                // 存在性判定：签名匹配（argc）的 Python instance-method override？
                // 同名不同签名（如 foo()I vs foo(I)I）不互相命中——argc 不匹配则回落 framework。
                if self.shim.resolve_method_name(&class_name, &java_name, argc).is_some() {
                    // —— Python override 路径 ——
                    // 按 Java 名调用 instance.<java_name>(args)，经 JavaObject.__getattr__
                    // 复用蓝图 __java_dispatch__（与方向 B 共用，零分歧）。
                    let sig_ret = sig.ret.clone();
                    let objects = Arc::clone(&self.shim.objects);
                    let id_alloc = Arc::clone(&self.id_alloc);
                    let result = Python::with_gil(|py| {
                        let bound = py_obj.bind(py);
                        let call_result = match args.len() {
                            0 => bound.call_method0(&java_name),
                            1 => {
                                let a0 = args.get_item(0)?;
                                bound.call_method1(&java_name, (a0,))
                            }
                            _ => bound.call_method(&java_name, args, None),
                        };

                        let ret_obj = call_result
                            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                                format!("method `{java_name}` 调用失败: {e}")
                            ))?;

                        // 运行时返回值校验（在 GIL 内完成）。
                        // py_to_jvalue：str/bytes 自动落身份成 Java 对象，不再吞成 Null。
                        let jval = javashim::py_to_jvalue(py, &ret_obj, &objects, &id_alloc)
                            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e))?;
                        if let Err(e) = javashim::validate_return_value(&jval, &sig_ret) {
                            return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                                format!("返回值类型校验失败: {e}")
                            ));
                        }

                        Ok::<PyObject, PyErr>(ret_obj.into())
                    })?;
                    Ok(result)
                } else {
                    // —— framework stub 回落路径（HostValue 中无签名匹配的 override）——
                    let resolved_class = if !sig.class.is_empty() {
                        sig.class.clone()
                    } else {
                        class_name
                    };

                    let mut dispatch_sig = sig.clone();
                    if dispatch_sig.class.is_empty() {
                        dispatch_sig.class = resolved_class;
                    }

                    // 入参编组：str/bytes 等自动 coercion（落身份成 Java 对象）。
                    let py = args.py();
                    let jni_args = javashim::pyargs_to_jniargs(py, args, &self.shim.objects, &self.id_alloc)
                        .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e))?;
                    // 创建临时 RefTable（dispatch 不需要真实 ref 表，仅满足 API 签名）
                    let mut temp_refs = rundroid_jni::RefTable::new();
                    // dispatch_call 仅走 framework stub 的 RustNative handler（无 engine 回调），
                    // 持守 VM Mutex 安全；此分支为非 override 回落，不触发 Python 方法体重入。
                    let result = self.vm.lock().unwrap().classes.dispatch_call(
                        &dispatch_sig, &jni_args, &mut temp_refs,
                    ).map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                        format!("method `{java_name}` 在 class `{}` 中分派失败: {e}", dispatch_sig.class)
                    ))?;

                    // 返回值编组：JValue::Object 按 storage 还原（String→str、byte[]→bytes）。
                    javashim::jvalue_to_py(py, &result, &self.shim.objects)
                }
            }

            // —— 非 Python 实例（StubInstance）——
            // call_java_method 是 Python 侧 API，只能操作 Python 实例（HostValue）。
            // 非 HostValue 的对象不是经 avm.new_object / register_java_object 创建的，
            // 没有 Python backing object，无法在此路径调用。
            CallTarget::StubInstance(cn) => Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                format!(
                    "handle {handle} 对应的是 Rust StubInstance（class={cn}），\
                     不是 Python 实例，不能通过 call_java_method 调用。\
                     StubInstance 的方法应由 guest 侧 JNI 调用触发。"
                )
            )),

            // —— 其他 storage 变体（不支持直接 Python 调用）——
            CallTarget::Unsupported(kind) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                format!(
                    "对象 {object_id} 的 storage 变体（{kind}）不支持 call_java_method"
                )
            )),
        }
    }

    /// 经类型化 JNI dispatch 调用已注册 Java 实例方法（参数/返回值都过 marshalling）。
    ///
    /// 与 [`Self::call_java_method`] 的区别：本方法**始终**走 marshalling——
    /// Python 参数先编组成 `JniArgs`（`str`/`bytes` 落身份成 Java 对象），
    /// 再经 registry dispatch 调到注册的 handler（Python `@java_method` 或 framework stub），
    /// 返回值再按 storage 还原成 Python 值。
    ///
    /// 这是端到端验证值编组的入口：等价于 guest 经 JNI 调用一个方法——
    /// 让 Python 侧 `@java_method` 收到的 `String`/`byte[]` 参数被还原成 `str`/`bytes`，
    /// 且返回的 `str`/`bytes` 不被吞成 `None`。
    ///
    /// # 死锁规避
    ///
    /// handler（Python 方法体）可能在方法体内 `self._avm.new_object(...)` →
    /// `register_java_object` 重入 runtime 的 write guard。故本方法在调用 handler **前**
    /// 释放 runtime read guard——先在短暂 read guard 内 clone 出 handler 的 `Arc`，
    /// guard 释放后再无锁调用。
    #[pyo3(signature = (handle, method_desc, args))]
    fn call_java_method_typed(
        &self,
        handle: u32,
        method_desc: &str,
        args: &Bound<'_, PyTuple>,
    ) -> PyResult<PyObject> {
        let py = args.py();
        let sig = MethodSig::parse(method_desc)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("method descriptor 解析失败: {e}")
            ))?;

        // 1. 短暂持 VM 锁：解析 handle → ObjectId + class_name，取出共享 objects/id_alloc。
        let (object_id, class_name, objects, id_alloc) = {
            let vm = self.vm.lock().unwrap();
            let oid = vm.refs.resolve(handle)
                .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("实例 handle {handle} 不存在于 RefTable")
                ))?;
            let class_name = vm.objects.lock().unwrap().class_name(oid)
                .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("ObjectId {oid} 不在 ObjectStore 中")
                ))?
                .to_string();
            (oid, class_name, Arc::clone(&self.shim.objects), Arc::clone(&self.id_alloc))
        };

        // 2. 解析 dispatch 用的 class（descriptor 带 class 用之，否则用对象 class_name）。
        let resolved_class = if !sig.class.is_empty() { sig.class.clone() } else { class_name };
        let mut dispatch_sig = sig.clone();
        if dispatch_sig.class.is_empty() {
            dispatch_sig.class = resolved_class.clone();
        }

        // 3. 短暂持 VM 锁：clone 出 handler Arc，立即释放锁（避免持锁调 Python 自锁）。
        let handler = {
            let vm = self.vm.lock().unwrap();
            let cls = vm.classes.find_class(&resolved_class)
                .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("class `{resolved_class}` 未注册")
                ))?;
            let method: &JMethodDef = cls.methods.get(&dispatch_sig)
                .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("method `{}` 在 class `{}` 中未注册", dispatch_sig.name, resolved_class)
                ))?;
            if method.is_static {
                return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("method `{}` 是 static，不能经实例 handle 调用", dispatch_sig.name)
                ));
            }
            match &method.imp {
                MethodImpl::RustNative(h) => Arc::clone(h),
                MethodImpl::PythonShim(_) => return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    "PythonShim 方法尚未接入"
                )),
            }
        }; // ← VM 锁在此释放

        // 4. 入参编组 + 标记 this（锁外，无 runtime guard）。
        let mut jni_args = javashim::pyargs_to_jniargs(py, args, &objects, &id_alloc)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(e))?;
        jni_args.set_this(object_id);

        // 5. 调用 handler（无 runtime guard，Python 方法体可安全重入 new_object 等）。
        //    handler 内部（wrap_python_method）已含返回值类型校验。
        let result = handler(&jni_args)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                format!("method `{}` 调用失败: {e}", dispatch_sig.name)
            ))?;

        // 6. 返回值编组（storage-aware：String→str、byte[]→bytes）。
        javashim::jvalue_to_py(py, &result, &objects)
    }

    /// 注册一个 framework stub class（纯 Rust-native handler）。
    ///
    /// 用于 test harness 模拟 Rust builtin class 注册，
    /// 后续 Python shim 可以通过 `register_or_merge_class` 覆盖其 method/field。
    ///
    /// 使用 `register_or_merge_class` 而非 `register_class`，
    /// 兼容后续 Python shim 的 override merge 语义。
    #[pyo3(signature = (class_name, methods))]
    fn register_framework_stub(
        &mut self,
        class_name: &str,
        methods: &Bound<'_, pyo3::types::PyDict>,
    ) -> PyResult<()> {
        let mut class_def = rundroid_jni::JClassDef::new(ClassId(0), class_name.to_string());

        for (key, val) in methods.iter() {
            let desc: String = key.extract()
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    format!("method key 必须是字符串: {e}")
                ))?;
            let ret_val: i32 = val.extract()
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    format!("method value 必须是 int: {e}")
                ))?;

            let mut sig = MethodSig::parse(&desc)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("descriptor 解析失败: {e}")
                ))?;
            if sig.class.is_empty() {
                sig.class = class_name.to_string();
            }

            let handler: Arc<dyn Fn(&JniArgs) -> Result<JValue, JniError> + Send + Sync> =
                Arc::new(move |_args: &JniArgs| -> Result<JValue, JniError> {
                    Ok(JValue::Int(ret_val))
                });

            class_def.add_method(sig, false, MethodImpl::RustNative(handler))
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("framework stub method 注册失败: {e}")
                ))?;
        }

        // 使用 register_or_merge_class：若 class 已存在则合并（保留已有 override），
        // 否则正常注册
        self.vm.lock().unwrap().classes.register_or_merge_class(class_def)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("framework stub class 注册失败: {e}")
            ))?;

        Ok(())
    }

    /// 读取已注册的 static field 值。
    ///
    /// 优先从 Rust registry（runtime.classes()）读取。
    /// 因为 `read_instance_field` 走 Python 实例直接读取，
    /// 本方法只在没有 Python shim 实例时使用。
    #[pyo3(signature = (class_name, field_desc))]
    fn read_java_field(&self, class_name: &str, field_desc: &str) -> PyResult<PyObject> {
        // TODO: "bar", "I" 没有考虑签名？
        let mut sig = FieldSig::parse(field_desc)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("field descriptor 解析失败: {e}")
            ))?;
        if sig.class.is_empty() {
            sig.class = class_name.to_string();
        }

        // 短暂持 VM 锁：registry 查询（field get 是纯 registry 读取，不调 Python）。
        let result = {
            let vm = self.vm.lock().unwrap();
            vm.classes.dispatch_static_field_get(&sig)
                .or_else(|_| vm.classes.dispatch_field_get(&sig))
        };
        let objects = Arc::clone(&self.shim.objects);

        Python::with_gil(|py| jvalue_result_to_py(py, result, &objects))
    }

    /// 获取实例的 Python 属性（field 值）。
    ///
    /// 通过 `RefTable::resolve` → `ObjectStore::storage` 查找 Python 实例，
    /// 然后访问其 Python 属性。
    fn read_instance_field(&self, handle: u32, field_name: &str) -> PyResult<PyObject> {
        // 锁内取出实例 Py 引用（clone），立即释放锁——getattr 会触发 JavaObject.__getattr__
        // （Python 操作），必须在无锁状态下执行。
        // 注意：refs 在 VM 上（需 VM 锁），objects 是独立 Arc<Mutex>（不需 VM 锁）。
        // 先短暂持 VM 锁解析 handle → ObjectId（释放后再锁 objects，不嵌套 VM 锁）。
        let py_obj: Py<PyAny> = {
            let object_id = self.vm.lock().unwrap().refs.resolve(handle)
                .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("实例 handle {handle} 不存在于 RefTable")
                ))?;
            let store = self.shim.objects.lock().unwrap();
            match store.storage(object_id) {
                Some(ObjectStorage::HostValue { data }) => {
                    let py_ref: &Py<PyAny> = data.downcast_ref::<Py<PyAny>>()
                        .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            "HostValue 中的数据类型不是 Py<PyAny>"
                        ))?;
                    Python::with_gil(|py| py_ref.clone_ref(py))
                }
                Some(storage) => {
                    return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                        format!(
                            "handle {handle} 对应的对象（kind={}）不支持直接读取 field",
                            storage.kind_label()
                        )
                    ));
                }
                None => {
                    return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                        format!("handle {handle} 对应的对象不在 ObjectStore 中")
                    ));
                }
            }
        };

        // 锁外：getattr（Python 操作）
        Python::with_gil(|py| {
            py_obj.bind(py).getattr(field_name)
                .map(|r| r.into())
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("读取实例 field `{field_name}` 失败: {e}")
                ))
        })
    }

    // ========================================================================
    // JNI guest 执行 — init_jni / jni_onload / 指针查询 / verbose / read_guest
    // ========================================================================

    /// 初始化 JNI guest 执行环境：映射 JNIEnv + JavaVM ABI 表到 guest 内存，
    /// 安装 trampoline code hook。
    ///
    /// 必须在 `load` 之后、`call` / `jni_onload` 之前调用一次。装配后：
    /// - guest 代码经 `(*env)->Fn` 回调会落入 trampoline，触发 hook 分派到 VM。
    /// - 返回的 `jni_env_pointer` / `java_vm_pointer` 可写入 guest 或传给 native。
    ///
    /// trampoline hook 持有 `Arc::clone(&self.vm)`——绑定层与 hook 共享同一 VM，
    /// 注册的 class 对 guest JNI dispatch 可见。verbose 开关也以 Arc 共享，
    /// 安装后仍可经 `set_jni_verbose` toggle。
    fn init_jni(&mut self) -> PyResult<()> {
        // 地址布局与 case-runner init_jni 一致：JNIEnv @ 0x7F_C000_0000，
        // JavaVM 紧跟其后；栈在 0x7F_E000_0000，sentinel 在 0x7F_FFFF_0000。
        const JNI_ENV_BASE: u64 = 0x7F_C000_0000;
        let env_abi = JNIEnvABI::new(JNI_ENV_BASE);
        let vm_abi = JavaVMABI::new(env_abi.env_ptr() + env_abi.total_size() as u64);

        // 1. 一次性映射整块 JNIEnv + JavaVM 区域（env struct / 函数表 / trampoline +
        //    JavaVM struct / invoke table / javavm trampoline）。
        let total_map = env_abi.total_size() + vm_abi.total_size();
        self.engine
            .mem_map(env_abi.env_ptr(), total_map, MemPerms::READ_EXEC)
            .map_err(backend_err)?;

        // 2. 写入 env header + 函数指针表 + trampoline NOP + JavaVM header + invoke table + trampoline NOP。
        {
            let engine: &mut EngineHolder = &mut self.engine;
            let mut mem_write = |addr: u64, bytes: &[u8]| {
                engine.mem_write(addr, bytes).is_ok()
            };
            env_abi.write_to_guest(&mut mem_write)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("JNIEnv ABI 写入 guest 失败: {e}")
                ))?;
            vm_abi.write_to_guest(&mut mem_write)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("JavaVM ABI 写入 guest 失败: {e}")
                ))?;
        }

        // 3. 安装 code hook 覆盖 [jni trampoline, javavm trampoline]，
        //    hook 内部按地址分流 dispatch（JNIEnv vs JavaVM）。
        let hook = JniTrampolineHook::new(
            env_abi.clone(),
            vm_abi.clone(),
            Arc::clone(&self.vm),
            Arc::clone(&self.jni_verbose),
        );
        let begin = env_abi.trampoline_begin();
        let end = vm_abi.trampoline_end();
        self.engine
            .install_code_hook(begin, end, Box::new(hook))
            .map_err(backend_err)?;

        // 4. 缓存 guest 指针（move ABI 前）。
        self.jni_env_ptr = Some(env_abi.env_ptr());
        self.jni_vm_ptr = Some(vm_abi.vm_ptr());

        Ok(())
    }

    /// 返回 JNIEnv guest 指针（`init_jni` 后有效；未初始化返回 None）。
    #[getter]
    fn jni_env_pointer(&self) -> Option<u64> {
        self.jni_env_ptr
    }

    /// 返回 JavaVM guest 指针（`jni_onload` 第一参数；未初始化返回 None）。
    #[getter]
    fn java_vm_pointer(&self) -> Option<u64> {
        self.jni_vm_ptr
    }

    /// 读取 guest 内存（host 视角），返回 bytes。供测试断言 guest 写出的数据。
    fn read_guest(&self, addr: u64, len: usize) -> PyResult<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.engine.mem_read(addr, &mut buf).map_err(backend_err)?;
        Ok(buf)
    }

    /// 遍历所有已加载模块，调用每个导出的 `JNI_OnLoad(JavaVM*, void*)`。
    ///
    /// 必须在 `init_jni` 之后调用（需要 JavaVM 指针）。每个模块：
    /// 1. 以 `java_vm_pointer` 作 x0、0 作 x1 调用 `JNI_OnLoad`
    /// 2. 校验返回值是合法 JNI version（`validate_jni_version`），非法则 fail-fast
    ///
    /// 返回各模块的 `(模块名, jni_version)` 列表。
    ///
    /// # 重入约束
    ///
    /// `JNI_OnLoad` 内部可能经 `(*vm)->GetEnv` → trampoline hook 分派到 VM。
    /// hook 持守 VM Mutex 期间不会回调 Python（dispatch 走 RustNative handler），
    /// 故 `call_guest`（emu_start）期间持锁安全。但若 JNI_OnLoad 注册的 native
    /// 方法后续触发 Python override，该 override 不得再入 VM（单线程仿真限制）。
    fn jni_onload(&mut self) -> PyResult<Vec<(String, u64)>> {
        let java_vm = self.jni_vm_ptr.ok_or_else(|| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
            "JavaVM 指针未初始化（init_jni 未调用？）"
        ))?;

        // 先收集所有 JNI_OnLoad 地址（避免借 self.graph 又 mut self.call_guest 冲突）。
        let onloads: Vec<(String, u64)> = self.graph.modules.values()
            .filter_map(|m| {
                m.exports.find("JNI_OnLoad").map(|e| (m.name.clone(), e.guest_addr))
            })
            .collect();

        let mut results = Vec::new();
        for (name, addr) in onloads {
            let version = self.call_guest(addr, &[java_vm, 0])?;
            if !validate_jni_version(version) {
                return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("模块 `{name}` 的 JNI_OnLoad 返回非法 JNI version: {version:#010x}")
                ));
            }
            results.push((name, version));
        }
        Ok(results)
    }

    /// 开启/关闭 JNI verbose trace（unidbg 式 `[I] JNIEnv->{slot}(...) => 0x...`）。
    ///
    /// 开启后每次 guest JNI 调用向 host stdout 打一行 trace，便于调试与测试断言
    /// （pytest `capsys` 可捕获）。trampoline hook 持有同一 `Arc<AtomicBool>`，
    /// 安装后 toggle 立即生效。
    fn set_jni_verbose(&self, on: bool) {
        self.jni_verbose.store(on, Ordering::Relaxed);
    }
}

// ============================================================================
// 非 pymethods 的内部方法（私有 helper，不暴露给 Python）
// ============================================================================

impl PyEmulatorBridge {
    /// 通用内置对象注册（Python 侧不直接调用，由 string/bytes 专方法复用）。
    ///
    /// 分配 ObjectId → 写入 ObjectStore → 分配 global ref handle。
    /// 三步分别持各自独立锁（id_alloc / objects 是内层 Arc<Mutex>），VM 锁只在
    /// 分配 global ref handle（vm.refs）时短暂持有；本方法不回调 Python，可安全持锁。
    fn register_builtin(
        &self,
        class_name: &str,
        storage: ObjectStorage,
    ) -> PyResult<(u32, ObjectId)> {
        // 1. ObjectId 由共享 IdAllocator 分配（独立 Arc<Mutex>，不持 VM 锁）
        let object_id = self.id_alloc.lock().unwrap().object();
        // 2. 写入 ObjectStore（独立 Arc<Mutex>，不持 VM 锁）
        self.shim.objects.lock().unwrap().insert(
            object_id,
            class_name.to_string(),
            storage,
        ).map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
            format!("ObjectStore 插入内置对象失败: {e}")
        ))?;
        // 3. 分配 global ref handle（refs 是 VM 直接字段，短暂持 VM 锁，不调 Python）
        let handle = self.vm.lock().unwrap().refs.new_global(object_id);
        Ok((handle, object_id))
    }

    /// 调用 guest 中地址 `entry_addr` 的函数（AArch64 ABI），返回 x0。
    ///
    /// 实现：sentinel 页放一条 `ret`，LR=sentinel，PC=entry，参数按 x0..x7 放置，
    /// `emu_start` 跑到 sentinel 自然停止，读 x0。
    ///
    /// sentinel + stack 仅首次映射（`trampoline_mapped` 复用），多次调用共享。
    /// `call`（按符号名调用）与 `jni_onload`（按 JNI_OnLoad 导出地址调用）共用此入口。
    fn call_guest(&mut self, entry_addr: u64, args: &[u64]) -> PyResult<u64> {
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
        for (i, v) in args.iter().take(8).enumerate() {
            self.engine.reg_write(Arm64Reg::X(i as u8), *v).map_err(backend_err)?;
        }

        self.engine.emu_start(entry_addr, Some(SENTINEL_ADDR), None, None).map_err(backend_err)?;

        self.engine.reg_read(Arm64Reg::X(0)).map_err(backend_err)
    }
}

/// 将 `Result<JValue, JniError>` 编组回 Python 对象（storage-aware）。
///
/// 成功时经 [`javashim::jvalue_to_py`] 还原（String→str、byte[]→bytes、primitive→标量、
/// Null→None），失败时把 `JniError` 包成 `PyRuntimeError`。
fn jvalue_result_to_py(
    py: Python<'_>,
    result: Result<JValue, JniError>,
    objects: &Arc<Mutex<ObjectStore>>,
) -> PyResult<PyObject> {
    match result {
        Ok(val) => javashim::jvalue_to_py(py, &val, objects),
        Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
            format!("JNI 调用失败: {e}")
        )),
    }
}

/// 将简单的 JNI descriptor 字符串解析为 [`JType`]。
///
/// 用于将 Python verify.py 返回的单字符 descriptor（如 `"I"`、`"V"`）
/// 或简单对象类型（如 `"Ljava/lang/String;"`）转换为 JType。
/// 只处理单层类型，不支持嵌套数组。
fn parse_jtype_from_descriptor(desc: &str) -> Option<rundroid_jni::JType> {
    if desc.is_empty() { return None; }
    match desc.chars().next().unwrap() {
        'V' => Some(rundroid_jni::JType::Void),
        'Z' => Some(rundroid_jni::JType::Boolean),
        'B' => Some(rundroid_jni::JType::Byte),
        'C' => Some(rundroid_jni::JType::Char),
        'S' => Some(rundroid_jni::JType::Short),
        'I' => Some(rundroid_jni::JType::Int),
        'J' => Some(rundroid_jni::JType::Long),
        'F' => Some(rundroid_jni::JType::Float),
        'D' => Some(rundroid_jni::JType::Double),
        'L' => {
            let semi = desc.find(';')?;
            let class_name = &desc[1..semi];
            if class_name.is_empty() { return None; }
            Some(rundroid_jni::JType::Object(class_name.to_string()))
        }
        '[' => {
            let inner = parse_jtype_from_descriptor(&desc[1..])?;
            Some(rundroid_jni::JType::Array(Box::new(inner)))
        }
        _ => None,
    }
}

// ============================================================================
// SyscallDispatcher
// ============================================================================

struct SyscallDispatcherPy {
    linux: Arc<Mutex<LinuxRuntime>>,
}

/// 把 [`GuestCPU`] 适配成 syscall 层的 [`MemoryBridge`]。
///
/// 收敛 `mem_read` / `mem_write` / `mem_map` 到 `read` / `write` / `map` 三方法，
/// 取代原来并排维护的三个 `read_guest` / `write_guest` / `map_guest` 闭包。
/// 用 `&mut *cpu` 重借用：bridge 持有期间独占 cpu，块结束时释放，
/// 之后 `reg_write` / `stop` 仍可用 cpu（不再需要裸指针 unsafe）。
struct CpuMemoryBridge<'a> {
    cpu: &'a mut dyn GuestCPU,
}

impl<'a> MemoryBridge for CpuMemoryBridge<'a> {
    fn read(&mut self, addr: u64, len: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len];
        if self.cpu.mem_read(addr, &mut buf) {
            Some(buf)
        } else {
            None
        }
    }

    fn write(&mut self, addr: u64, data: &[u8]) -> bool {
        self.cpu.mem_write(addr, data)
    }

    fn map(&mut self, addr: u64, len: usize, prot: i32) -> bool {
        use rundroid_backend::MemPerms;
        // prot (POSIX PROT_* 位掩码) → MemPerms。
        let read = (prot & 1) != 0;
        let write = (prot & 2) != 0;
        let exec = (prot & 4) != 0;
        let perms = MemPerms::from_flags(read, write, exec);
        self.cpu.mem_map(addr, len, perms).is_ok()
    }
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

        // bridge 重借用 cpu；块结束时释放，之后 reg_write/stop 可继续用 cpu。
        let result = {
            let mut bridge = CpuMemoryBridge { cpu: &mut *cpu };
            let mut linux = self.linux.lock().unwrap();
            linux.dispatch(nr, x0, x1, x2, x3, x4, x5, &mut bridge)
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
    m.add_class::<PyEmulatorBridge>()?;
    m.add_class::<PyVirtFile>()?;
    Ok(())
}
