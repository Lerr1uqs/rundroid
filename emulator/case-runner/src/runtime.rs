//! runtime 装配层。
//!
//! [`GuestRuntime`] 是 case-runner 内部的"总装对象"，
//! 持有 backend engine、内存账本、linux runtime、telemetry router，
//! 同时实现 [`LoadContext`](rundroid_elf_loader::LoadContext) 与
//! [`LinkContext`](rundroid_elf_linker::LinkContext)，
//! 把 elf loader/linker 的副作用落到具体 backend。

use rundroid_backend::{Arm64Reg, Backend, BackendError, Engine, MemPerms, GuestCPU, SyscallHook};
use rundroid_backend_unicorn::UnicornBackend;
use rundroid_core::{IdAllocator, ModuleId, RuntimeConfig};
use rundroid_elf_linker::{
    DefaultLinker, ElfLinkError, LinkContext as LinkCtx, LinkReport, ModuleGraph,
    RelocationPatch, ResolvedSymbol, SymbolQuery,
};
use rundroid_elf_loader::{
    DefaultLoader, ElfLoadError, ElfLoader, LoadContext as LoadCtx, LoadRequest, MappedSegment,
    SegmentMapSpec,
};
use rundroid_elf_parser::{ElfCrateParser, ElfParser, ParseInput, ParsedElf};
use rundroid_jni::{AndroidVM, JavaVMABI, JNIEnvABI, JniRegistry, ObjectStore, RefTable};
use rundroid_linux::{LinuxRuntime, MemoryBridge, SyscallResult};
use rundroid_memory::{
    AllocationRequest, DynamicArena, MemoryAddressSpace, MemoryError, MemoryMaterializer,
    MemoryPerms as AddressSpacePerms, MemoryUsage, RegionTracker, PAGE_SIZE,
};
use rundroid_telemetry::{EventSink, TelemetryEvent, TelemetryEventKind, TelemetryMode, TelemetryRouter};
use std::sync::{Arc, Mutex};
use thiserror::Error;

use crate::jni_hook::JniTrampolineHook;

#[derive(Debug, Error)]
pub enum RuntimeAssemblyError {
    #[error("backend error: {0}")]
    Backend(#[from] BackendError),
    #[error("memory layout error: {0}")]
    Memory(#[from] MemoryError),
    #[error("parse error: {0}")]
    Parse(#[from] rundroid_elf_parser::ElfParseError),
    #[error("load error: {0}")]
    Load(#[from] ElfLoadError),
    #[error("link error: {0}")]
    Link(#[from] ElfLinkError),
    #[error("entry symbol `{0}` not found in module exports")]
    EntryMissing(String),
    #[error("JNI_OnLoad failed in module `{module}`: {reason}")]
    JniOnLoadFailed { module: String, reason: String },
    #[error("JNI ABI guest write 失败: {0}")]
    JniAbiWrite(String),
}

/// 用于序列化到 `events.jsonl` 的一条事件记录。
///
/// 注意：放在 runtime.rs 是因为 CollectingSink（telemetry 落地）就在这里产生；
/// artifacts 模块通过 `crate::runtime::EventRecord` 引用以避免循环依赖。
#[derive(Debug, Clone, serde::Serialize)]
pub struct EventRecord {
    pub name: String,
    pub kind: String,
}

/// 收集型 sink：把事件攒进 `Arc<Mutex<Vec>>`，
/// case-runner 可以稍后取出落盘成 `events.jsonl`。
#[derive(Default, Clone)]
pub struct CollectingSink {
    pub events: Arc<Mutex<Vec<EventRecord>>>,
}

impl EventSink for CollectingSink {
    fn record(&mut self, event: &TelemetryEvent<'_>) {
        let kind = match event.kind {
            TelemetryEventKind::Lifecycle => "lifecycle",
            TelemetryEventKind::Memory => "memory",
            TelemetryEventKind::Elf => "elf",
            TelemetryEventKind::Execution => "execution",
            TelemetryEventKind::FileSystem => "filesystem",
            TelemetryEventKind::Jni => "jni",
        };
        self.events.lock().unwrap().push(EventRecord {
            name: event.name.to_string(),
            kind: kind.to_string(),
        });
    }
}

/// 一个完整装配好的 guest runtime。
pub struct GuestRuntime {
    pub engine: Box<dyn Engine>,
    pub regions: RegionTracker,
    pub address_space: Arc<Mutex<MemoryAddressSpace>>,
    /// LinuxRuntime 用 `Arc<Mutex<>>` 包装，因为 syscall hook 闭包也要访问它。
    /// 对外暴露的 API 通过 method 转发到内部 LinuxRuntime，调用方无感。
    pub linux_inner: Arc<Mutex<LinuxRuntime>>,
    pub allocator: IdAllocator,
    pub router: TelemetryRouter,
    pub sink: Option<CollectingSink>,
    pub config: RuntimeConfig,
    pub graph: ModuleGraph,
    pub last_link: Option<LinkReport>,
    /// JNI shim registry — class / method / field 注册表。
    pub jni_registry: JniRegistry,
    /// JNI 引用表 — handle → ObjectId 映射。
    pub jni_refs: RefTable,
    /// JNI 对象存储（共享所有权，供 hook 访问）。
    pub object_store: Arc<Mutex<ObjectStore>>,
    /// Guest 可见的 JNIEnv ABI 对象（函数表 + trampoline 布局，装配时映射）。
    pub jni_env_abi: Option<JNIEnvABI>,
    /// Guest 可见的 JavaVM ABI 对象（invoke table + trampoline 布局）。
    pub java_vm_abi: Option<JavaVMABI>,
    /// JNIEnv guest 指针（= `jni_env_abi` 的 env_ptr，便利字段）。
    pub jni_env_pointer: Option<u64>,
    /// JavaVM guest 指针（= `java_vm_abi` 的 vm_ptr，JNI_OnLoad 参数）。
    pub java_vm_pointer: Option<u64>,
    /// JNI trampoline hook 的 telemetry sink（run 后取出事件）。
    pub jni_telemetry: Option<Arc<Mutex<Vec<(String, TelemetryEventKind)>>>>,
    /// sentinel/stack 是否已经映射（复用避免重叠 mem_map）。
    trampoline_mapped: bool,
}

impl GuestRuntime {
    /// 设置 urandom PRNG 种子（让 syscall 路径产生确定性输出）。
    pub fn seed_rng(&mut self, seed: u64) {
        self.linux_inner.lock().unwrap().seed_rng(seed);
    }
}

/// LinuxRuntime 对外的"看起来像直接持有"的访问接口。
/// case.rs 等通过 `rt.linux()` 拿到 guard 使用，避免每次都写 lock。
impl GuestRuntime {
    /// 锁定 LinuxRuntime，返回 guard。短期使用，避免长时间持锁
    /// （emu_start 期间不能持锁，否则 hook 里再 lock 会死锁）。
    pub fn linux(&self) -> std::sync::MutexGuard<'_, LinuxRuntime> {
        self.linux_inner.lock().unwrap()
    }
}

/// 把 svc 指令分派到 LinuxRuntime 的 hook。
struct SyscallDispatcher {
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
        // prot (POSIX PROT_* 位掩码) → MemPerms。
        let read = (prot & 1) != 0;
        let write = (prot & 2) != 0;
        let exec = (prot & 4) != 0;
        let perms = MemPerms::from_flags(read, write, exec);
        self.cpu.mem_map(addr, len, perms).is_ok()
    }

    fn protect(&mut self, addr: u64, len: usize, prot: i32) -> bool {
        let read = (prot & 1) != 0;
        let write = (prot & 2) != 0;
        let exec = (prot & 4) != 0;
        let perms = MemPerms::from_flags(read, write, exec);
        self.cpu.mem_protect(addr, len, perms).is_ok()
    }

    fn unmap(&mut self, addr: u64, len: usize) -> bool {
        self.cpu.mem_unmap(addr, len).is_ok()
    }
}

impl SyscallHook for SyscallDispatcher {
    fn on_svc(&mut self, cpu: &mut dyn GuestCPU) {
        let nr = cpu.reg_read(Arm64Reg::X(8));
        let x0 = cpu.reg_read(Arm64Reg::X(0));
        let x1 = cpu.reg_read(Arm64Reg::X(1));
        let x2 = cpu.reg_read(Arm64Reg::X(2));
        let x3 = cpu.reg_read(Arm64Reg::X(3));
        let x4 = cpu.reg_read(Arm64Reg::X(4));
        let x5 = cpu.reg_read(Arm64Reg::X(5));

        // bridge 重借用 cpu；块结束时释放，之后 reg_write/stop 可继续用 cpu。
        // 失败的 read/write/map 让 LinuxRuntime 把"未映射缓冲"上报成 EFAULT，
        // 避免出现"写没写成功都返回 N"的假阳性。
        let result = {
            let mut bridge = CpuMemoryBridge { cpu: &mut *cpu };
            let mut linux = self.linux.lock().unwrap();
            linux.dispatch(nr, x0, x1, x2, x3, x4, x5, &mut bridge)
        };

        match result {
            SyscallResult::Done(v) => {
                cpu.reg_write(Arm64Reg::X(0), v);
            }
            SyscallResult::Exit(_code) => {
                cpu.stop();
            }
        }
    }
}

impl GuestRuntime {
    /// 按 config 装配一个空 runtime。
    pub fn assemble(config: RuntimeConfig) -> Result<Self, RuntimeAssemblyError> {
        let backend = UnicornBackend::new();
        let mut engine = backend.open(config.arch)?;
        let address_space = Arc::new(Mutex::new(MemoryAddressSpace::new()));
        let sink = CollectingSink::default();
        let router = match config.telemetry {
            TelemetryMode::Disabled => TelemetryRouter::disabled(),
            _ => TelemetryRouter::events_only(Box::new(sink.clone())),
        };

        // syscall hook 与 LinuxRuntime 之间通过 Rc<RefCell<>> 共享。
        // hook 被 install_syscall_hook 注册后，每次 svc 都会回调到这个 LinuxRuntime。
        let linux = Arc::new(Mutex::new(LinuxRuntime::with_address_space(Arc::clone(&address_space))));
        engine.install_syscall_hook(Box::new(SyscallDispatcher {
            linux: Arc::clone(&linux),
        }))?;

        // 预映射 guest scratch buffer：1 MiB @ 0x800_000，RW。
        // 让 case manifest 可以把 0x800_000..0x900_000 当作"传入 syscall 的缓冲地址"，
        // 否则 syscall hook 的 write_guest 会因地址未映射而（正确地）返回 EFAULT。
        // 这个区段是 bootstrap case manifest 的公开契约。
        const SCRATCH_ADDR: u64 = 0x800_000;
        const SCRATCH_SIZE: usize = 0x10_0000;
        let mut regions = RegionTracker::new();
        {
            let mut materializer = EngineMemoryMaterializer {
                engine: engine.as_mut(),
                regions: &mut regions,
            };
            address_space.lock().unwrap().allocate(
                AllocationRequest::reserved(
                    SCRATCH_ADDR,
                    SCRATCH_SIZE as u64,
                    PAGE_SIZE,
                    AddressSpacePerms::READ_WRITE,
                    MemoryUsage::Scratch,
                ),
                &mut materializer,
            )?;
        }

        Ok(Self {
            engine,
            regions,
            address_space,
            linux_inner: linux,
            allocator: IdAllocator::new(),
            router,
            sink: Some(sink),
            config,
            graph: ModuleGraph::new(),
            last_link: None,
            jni_registry: JniRegistry::new(),
            jni_refs: RefTable::new(),
            object_store: Arc::new(Mutex::new(ObjectStore::new())),
            jni_env_abi: None,
            java_vm_abi: None,
            jni_env_pointer: None,
            java_vm_pointer: None,
            jni_telemetry: None,
            trampoline_mapped: false,
        })
    }

    /// 初始化 JNI 环境：映射 JNIEnv + JavaVM ABI 表到 guest 内存，安装 code hook。
    ///
    /// 必须在 `assemble()` 之后、`emu_start()` 之前调用。`vm` 是共享的
    /// `AndroidVM`（registry + objects + refs + exceptions），JNI trampoline hook
    /// 持有其引用。
    ///
    /// 装配步骤：
    /// 1. 按 [`JNIEnvABI`] + [`JavaVMABI`] 计算 guest 内存布局（JavaVM 紧跟 JNIEnv 之后）
    /// 2. 一次性映射整块区域（env struct / 函数表 / jni trampoline +
    ///    JavaVM struct / invoke table / javavm trampoline）
    /// 3. 写入 env header + 函数指针表 + jni trampoline NOP +
    ///    JavaVM header + invoke table + javavm trampoline NOP
    /// 4. 安装 code hook 覆盖 [jni trampoline, javavm trampoline]，hook 内部按地址分流
    ///
    /// # JNI 地址布局
    ///
    /// 使用高端地址 0x7F_C000_0000（栈在 0x7F_E000_0000，sentinel 在 0x7F_FFFF_0000）。
    /// JavaVM 区域紧跟 JNIEnv 区域之后。
    ///
    /// # NOTE: 硬编码地址是临时缓解措施
    ///
    /// 当前直接用 `engine.mem_map` 抢固定地址。后续应改为通过 guest 自身的
    /// mmap 或 allocator 分配，避免与 guest .so 装载地址冲突。
    pub fn init_jni(&mut self, vm: Arc<Mutex<AndroidVM>>) -> Result<(), RuntimeAssemblyError> {
        const JNI_ENV_BASE: u64 = 0x7F_C000_0000;
        let env_abi = JNIEnvABI::new(JNI_ENV_BASE);
        let vm_abi = JavaVMABI::new(env_abi.env_ptr() + env_abi.total_size() as u64);

        let total_map = env_abi.total_size() + vm_abi.total_size();
        {
            let mut materializer = EngineMemoryMaterializer {
                engine: self.engine.as_mut(),
                regions: &mut self.regions,
            };
            self.address_space.lock().unwrap().allocate(
                AllocationRequest::reserved(
                    env_abi.env_ptr(),
                    total_map as u64,
                    PAGE_SIZE,
                    AddressSpacePerms::READ_EXEC,
                    MemoryUsage::JNIEnv,
                ),
                &mut materializer,
            )?;
        }

        // 2. 写入 JNIEnv 表 + JavaVM 表（含 invoke table + 各自 trampoline NOP）。
        //    mem_write 闭包借 &mut engine，写完即释放，后续可继续用 self.engine。
        {
            let engine = &mut self.engine;
            let mut mem_write = |addr: u64, bytes: &[u8]| engine.mem_write(addr, bytes).is_ok();
            env_abi
                .write_to_guest(&mut mem_write)
                .map_err(RuntimeAssemblyError::JniAbiWrite)?;
            vm_abi
                .write_to_guest(&mut mem_write)
                .map_err(RuntimeAssemblyError::JniAbiWrite)?;
        }

        // 3. 创建 JniTrampolineHook（持有双 ABI + 线程状态 + verbose 开关），安装为 code hook。
        //    范围覆盖 jni trampoline + javavm trampoline，hook 内部按地址分流 dispatch。
        //    verbose 默认关闭（case-runner 不打 JNI trace）；Python 绑定层安装时
        //    传入可被 toggle 的 Arc<AtomicBool>。
        let hook = JniTrampolineHook::new(
            env_abi.clone(),
            vm_abi.clone(),
            vm,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        let begin = env_abi.trampoline_begin();
        let end = vm_abi.trampoline_end();

        // 保存 telemetry sink 引用（run 之后读取事件）
        self.jni_telemetry = Some(hook.telemetry_sink());

        self.engine
            .install_code_hook(begin, end, Box::new(hook))
            .map_err(RuntimeAssemblyError::Backend)?;

        // 先取出 guest 指针，再 move ABI 对象到字段（避免 move 后借用）。
        let env_ptr = env_abi.env_ptr();
        let vm_ptr = vm_abi.vm_ptr();

        self.jni_env_abi = Some(env_abi);
        self.java_vm_abi = Some(vm_abi);
        self.jni_env_pointer = Some(env_ptr);
        self.java_vm_pointer = Some(vm_ptr);

        Ok(())
    }

    /// 装载并链接一个模块（root），按需递归装载 `DT_NEEDED`。
    /// 返回 root 模块的 ID。
    ///
    /// `dep_provider`：DT_NEEDED 出现时被调用，按 soname 返回对应 .so 字节流。
    /// 返回 `None` 视为依赖缺失，整个装载失败（Android linker 语义）。
    pub fn load_and_link(
        &mut self,
        module_name: &str,
        bytes: &[u8],
        dep_provider: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    ) -> Result<ModuleId, RuntimeAssemblyError> {
        let root_id = self.load_one(module_name, bytes)?;
        // 解析 root 拿到 DT_NEEDED 列表（再 parse 一次开销可接受，避免改 load_one 签名）。
        let needed: Vec<String> = {
            let parsed = ElfCrateParser::new().parse(ParseInput::new(module_name, bytes))?;
            parsed.dynamic.needed.clone()
        };
        // 递归装载 DT_NEEDED（DFS），建立 deps 边。
        // bootstrap 阶段的容错策略：dep_provider 返回 None 视为"该依赖暂未在资源 pack 中提供"
        // （典型例子：libc.so / libdl.so 由 host bridge 兜底，但 host bridge 尚未接入）。
        // 这种情况记一条 telemetry 事件并跳过，不阻塞 root 装载——
        // 真正的 Android linker 是 hard-fail，我们等 host bridge 接入后再切严格模式。
        for dep_name in &needed {
            let dep_id = match self.graph.by_soname.get(dep_name).copied() {
                Some(id) => id,
                None => match dep_provider(dep_name) {
                    Some(dep_bytes) => self.load_and_link(dep_name, &dep_bytes, dep_provider)?,
                    None => {
                        self.router.emit(&TelemetryEvent::new(
                            "module.dep_missing",
                            TelemetryEventKind::Elf,
                        ));
                        continue;
                    }
                },
            };
            self.graph.add_dep(root_id, dep_id);
        }

        // link_root 在 root 上一次性 link 整张图（schedule 会拓扑遍历 deps）。
        let report = {
            let Self {
                address_space,
                engine,
                graph,
                regions,
                router,
                ..
            } = self;
            // 见下方注释：link_root 同时需要 &mut graph 与 &mut dyn LinkContext，
            // 而 resolve 又要读 graph。运行期是顺序的，借用裸指针绕过编译期借用检查。
            let graph_ptr: *mut ModuleGraph = graph;
            let mut link_ctx = LinkCtxAdapter {
                space: address_space,
                engine: engine.as_mut(),
                graph_ptr,
                regions,
                router,
            };
            DefaultLinker::new().link_root(&mut link_ctx, graph, root_id)?
        };
        self.last_link = Some(report);

        // JNI_OnLoad lifecycle：模块装载 + 链接完成后，
        // 自动检测并调用各模块导出的 JNI_OnLoad。
        let _onload_results = self.detect_jni_onload()?;

        Ok(root_id)
    }

    /// 装载单个模块（不做 DT_NEEDED 递归、不 link）。
    /// 重复 soname 会被忽略（返回已装载的 id）。
    fn load_one(
        &mut self,
        module_name: &str,
        bytes: &[u8],
    ) -> Result<ModuleId, RuntimeAssemblyError> {
        let parsed: ParsedElf =
            ElfCrateParser::new().parse(ParseInput::new(module_name, bytes))?;

        // 若已有同 soname 的模块，直接返回已装载的 id（去重）。
        if let Some(name) = &parsed.dynamic.soname {
            if let Some(existing) = self.graph.by_soname.get(name).copied() {
                return Ok(existing);
            }
        }

        let module_id = self.allocator.module();
        {
            let Self {
                address_space,
                engine,
                regions,
                router,
                ..
            } = self;
            let mut ctx = LoadCtxAdapter {
                space: address_space,
                engine: engine.as_mut(),
                regions,
                router,
            };
            let module = DefaultLoader::new().load(
                &mut ctx,
                &parsed,
                LoadRequest {
                    image_align: 0x1000,
                    bytes,
                    module_id,
                },
            )?;
            self.graph.insert(module, parsed.dynamic.soname.clone());
        }

        Ok(module_id)
    }

    /// 尝试 Java_* mangled symbol fallback 查找。
    ///
    /// 当 method 未通过 `RegisterNatives` 注册、也未在 registry 中找到 Rust/Python handler 时，
    /// 在已装载模块中查找 `Java_*` mangled 符号名。
    ///
    /// 先尝试无重载的短格式，失败再尝试带签名后缀的长格式。
    pub fn resolve_java_native(
        &self,
        class_name: &str,
        method_name: &str,
        sig_descriptor: &str,
    ) -> Option<u64> {
        use rundroid_jni::native_registry::{mangle_java_method, mangle_java_method_overloaded};
        // 先尝试无重载的符号
        let simple = mangle_java_method(class_name, method_name);
        if let Some(addr) = self.resolve_symbol(&simple) {
            return Some(addr);
        }
        // 再尝试带签名后缀的重载符号
        let overloaded = mangle_java_method_overloaded(class_name, method_name, sig_descriptor);
        self.resolve_symbol(&overloaded)
    }

    /// 按名字在已装载模块里查找符号地址。
    pub fn resolve_symbol(&self, name: &str) -> Option<u64> {
        for m in self.graph.modules.values() {
            if let Some(e) = m.exports.find(name) {
                return Some(e.guest_addr);
            }
        }
        None
    }

    /// 调用一个已装载模块中的导出函数。
    ///
    /// 实现：
    /// - 在 guest 内存里放一段 `ret` sentinel，LR 指向它，PC 指向 entry
    /// - 参数按 AArch64 ABI 放进 x0..x7
    /// - `emu_start` 跑到 sentinel 为止
    /// - 返回 x0
    ///
    /// 注意：bootstrap 阶段不接 syscall hook（svc 指令未被拦截），
    /// 因此调用路径上遇到 svc 会作为"非法指令"失败。
    /// 纯计算函数（如 rd_add）可以直接跑通；
    /// 依赖 syscall 的函数需要等 syscall hook 接入后才能跑。
    pub fn call_export(&mut self, entry_addr: u64, args: &[u64]) -> Result<u64, BackendError> {
        use rundroid_backend::Arm64Reg;

        // sentinel + stack 只在第一次调用时映射；后续复用。
        // 否则在同一 runtime 内多次 call 会触发重叠 mem_map。
        // TODO: 固定地址未来有更好的方式处理
        const SENTINEL_ADDR: u64 = 0x7F_FFFF_0000;
        const STACK_BASE: u64 = 0x7F_E000_0000;
        const STACK_TOP: u64 = STACK_BASE + 0x10_0000;
        if !self.trampoline_mapped {
            let mut materializer = EngineMemoryMaterializer {
                engine: self.engine.as_mut(),
                regions: &mut self.regions,
            };
            self.address_space.lock().unwrap().allocate(
                AllocationRequest::reserved(
                    SENTINEL_ADDR,
                    0x1000,
                    PAGE_SIZE,
                    AddressSpacePerms::READ_EXEC,
                    MemoryUsage::Trampoline,
                ),
                &mut materializer,
            ).map_err(|e| BackendError::Emulation(Box::leak(format!("address space allocate sentinel failed: {e}").into_boxed_str())))?;
            self.address_space.lock().unwrap().allocate(
                AllocationRequest::reserved(
                    STACK_BASE,
                    0x10_0000,
                    PAGE_SIZE,
                    AddressSpacePerms::READ_WRITE,
                    MemoryUsage::Stack,
                ),
                &mut materializer,
            ).map_err(|e| BackendError::Emulation(Box::leak(format!("address space allocate stack failed: {e}").into_boxed_str())))?;
            // ARM64 `ret` = 0xD65F03C0，小端字节序。
            self.engine.mem_write(SENTINEL_ADDR, &[0xC0, 0x03, 0x5F, 0xD6])?;
            self.trampoline_mapped = true;
        }

        self.engine.reg_write(Arm64Reg::Sp, STACK_TOP)?;
        self.engine.reg_write(Arm64Reg::Lr, SENTINEL_ADDR)?;
        self.engine.reg_write(Arm64Reg::Pc, entry_addr)?;
        for (i, v) in args.iter().take(8).enumerate() {
            self.engine.reg_write(Arm64Reg::X(i as u8), *v)?;
        }

        // until = SENTINEL_ADDR：执行到那条 ret 之前停下。
        // 实际上 ARM64 emu_start 的 until 是"到达该地址时停止"，
        // 因此当被调函数 ret 跳到 sentinel 时立即停止，sentinel 本身不执行。
        self.engine.emu_start(entry_addr, Some(SENTINEL_ADDR), None, None)?;

        self.engine.reg_read(Arm64Reg::X(0))
    }

    /// 重置每次调用之间的瞬时状态（除栈/sentinel 之外）。
    /// 当前实现把 `trampoline_mapped` 标记放在 runtime 上，
    /// 多次调用共享同一个栈/sentinel 区。

    /// 将 JNI trampoline hook 中累积的 telemetry 事件写入 runtime router。
    pub fn flush_jni_telemetry(&mut self) {
        if let Some(ref sink) = self.jni_telemetry {
            if let Ok(mut events) = sink.lock() {
                for (name, kind) in events.drain(..) {
                    self.router.emit(&TelemetryEvent::new(&name, kind));
                }
            }
        }
    }

    /// 取出当前累积的 telemetry 事件（合并 JNI hook 事件）。
    pub fn take_events(&mut self) -> Vec<EventRecord> {
        self.flush_jni_telemetry();
        if let Some(sink) = self.sink.as_ref() {
            let mut g = sink.events.lock().unwrap();
            std::mem::take(&mut *g)
        } else {
            Vec::new()
        }
    }

    /// 遍历所有已加载模块，查找并调用 `JNI_OnLoad`。
    ///
    /// 对每个导出 `JNI_OnLoad` 的模块：
    /// 1. 以 `JavaVM*` 作为第一参数调用该函数
    /// 2. 校验返回值是否为合法 JNI version
    /// 3. 输出 telemetry 事件
    ///
    /// 返回每个模块的调用结果：`(module_id, soname, version)`。
    /// 非法 JNI version 或 backend 调用失败会立即返回 `Err`（spec: "SHALL 显式失败"）。
    pub fn detect_jni_onload(&mut self) -> Result<Vec<(ModuleId, String, u64)>, RuntimeAssemblyError> {
        use rundroid_jni::native_registry::validate_jni_version;

        let java_vm = self.java_vm_pointer.unwrap_or(0);
        let mut results = Vec::new();

        // 先收集所有 JNI_OnLoad 地址（避免 borrow 冲突）
        let onloads: Vec<(ModuleId, String, u64)> = self
            .graph
            .modules
            .iter()
            .filter_map(|(&id, m)| {
                m.exports
                    .find("JNI_OnLoad")
                    .map(|e| (id, m.name.clone(), e.guest_addr))
            })
            .collect();

        for (module_id, soname, addr) in onloads {
            self.router.emit(&TelemetryEvent::new(
                "jni.call",
                TelemetryEventKind::Jni,
            ));
            self.router.emit(&TelemetryEvent::new(
                "jni.jni_onload_call",
                TelemetryEventKind::Jni,
            ));

            if java_vm == 0 {
                return Err(RuntimeAssemblyError::JniOnLoadFailed {
                    module: soname,
                    reason: "JavaVM 指针未初始化（init_jni 未调用？）".into(),
                });
            }

            // 调用 JNI_OnLoad(JavaVM*, void* reserved)
            let version = self.call_export(addr, &[java_vm, 0])
                .map_err(|e| RuntimeAssemblyError::JniOnLoadFailed {
                    module: soname.clone(),
                    reason: format!("backend 调用失败: {e}"),
                })?;

            if !validate_jni_version(version) {
                return Err(RuntimeAssemblyError::JniOnLoadFailed {
                    module: soname,
                    reason: format!("非法 JNI version: {version:#010x}"),
                });
            }

            results.push((module_id, soname.clone(), version));
            self.router.emit(&TelemetryEvent::new(
                "jni.jni_onload_ok",
                TelemetryEventKind::Jni,
            ));
        }

        Ok(results)
    }
}

struct LoadCtxAdapter<'a> {
    space: &'a Arc<Mutex<MemoryAddressSpace>>,
    engine: &'a mut dyn Engine,
    regions: &'a mut RegionTracker,
    router: &'a mut TelemetryRouter,
}

impl<'a> LoadCtx for LoadCtxAdapter<'a> {
    fn reserve_image_space(&mut self, size: u64, align: u64) -> Result<u64, MemoryError> {
        let mut materializer = EngineMemoryMaterializer {
            engine: self.engine,
            regions: self.regions,
        };
        let region = self.space.lock().unwrap().allocate(
            AllocationRequest::dynamic(
                size,
                align.max(PAGE_SIZE),
                AddressSpacePerms::ALL,
                MemoryUsage::ELFImage,
                DynamicArena::new(0x4000_0000, 0x7F00_0000_0000),
                0x4000_0000,
            ),
            &mut materializer,
        )?;
        Ok(region.addr)
    }
    fn map_segment(&mut self, spec: SegmentMapSpec<'_>) -> Result<MappedSegment, MemoryError> {
        Ok(MappedSegment {
            guest_addr: spec.guest_addr,
            size: spec.size,
        })
    }
    fn protect_segment(
        &mut self,
        guest_addr: u64,
        size: u64,
        perms: AddressSpacePerms,
        _usage: MemoryUsage,
    ) -> Result<(), MemoryError> {
        let start = align_down(guest_addr);
        let end = align_up(
            guest_addr
                .checked_add(size)
                .ok_or(MemoryError::Overflow {
                    addr: guest_addr,
                    size,
                })?,
            PAGE_SIZE,
        );
        let mut materializer = EngineMemoryMaterializer {
            engine: self.engine,
            regions: self.regions,
        };
        self.space
            .lock()
            .unwrap()
            .protect(start, end - start, perms, &mut materializer)
    }
    fn write_bytes(&mut self, guest_addr: u64, bytes: &[u8]) -> Result<(), MemoryError> {
        self.engine
            .mem_write(guest_addr, bytes)
            .map_err(|_| MemoryError::NotMapped { addr: guest_addr })
    }
    fn zero_fill(&mut self, guest_addr: u64, len: u64) -> Result<(), MemoryError> {
        let zeros = vec![0u8; len as usize];
        self.engine
            .mem_write(guest_addr, &zeros)
            .map_err(|_| MemoryError::NotMapped { addr: guest_addr })
    }
    fn emit(&mut self, event: TelemetryEvent<'_>) {
        self.router.emit(&event);
    }
}

struct EngineMemoryMaterializer<'a> {
    engine: &'a mut dyn Engine,
    regions: &'a mut RegionTracker,
}

impl<'a> MemoryMaterializer for EngineMemoryMaterializer<'a> {
    fn map(
        &mut self,
        addr: u64,
        size: u64,
        perms: AddressSpacePerms,
        usage: MemoryUsage,
    ) -> Result<(), MemoryError> {
        self.engine
            .mem_map(addr, size as usize, MemPerms::from_flags(perms.readable(), perms.writable(), perms.executable()))
            .map_err(|e| MemoryError::MaterializeFailed {
                op: "map",
                addr,
                size,
                reason: e.to_string(),
            })?;
        self.regions.register(addr, size, origin_from_usage(usage))?;
        Ok(())
    }

    fn protect(&mut self, addr: u64, size: u64, perms: AddressSpacePerms) -> Result<(), MemoryError> {
        self.engine
            .mem_protect(
                addr,
                size as usize,
                MemPerms::from_flags(perms.readable(), perms.writable(), perms.executable()),
            )
            .map_err(|e| MemoryError::MaterializeFailed {
                op: "protect",
                addr,
                size,
                reason: e.to_string(),
            })
    }

    fn unmap(&mut self, addr: u64, size: u64) -> Result<(), MemoryError> {
        self.engine
            .mem_unmap(addr, size as usize)
            .map_err(|e| MemoryError::MaterializeFailed {
                op: "unmap",
                addr,
                size,
                reason: e.to_string(),
            })
    }
}

fn origin_from_usage(usage: MemoryUsage) -> rundroid_memory::RegionOrigin {
    match usage {
        MemoryUsage::ELFImage | MemoryUsage::Relro => rundroid_memory::RegionOrigin::ELFSegment,
        MemoryUsage::Stack => rundroid_memory::RegionOrigin::Stack,
        MemoryUsage::Tls => rundroid_memory::RegionOrigin::TLS,
        _ => rundroid_memory::RegionOrigin::RuntimeScratch,
    }
}

/// LinkContext 适配器。
///
/// 由于 [`DefaultLinker::link_root`] 同时持有 `&mut ModuleGraph` 和
/// `&mut dyn LinkContext`，而 resolve 又需要读 graph，
/// 这里用裸指针让两者"逻辑上"不冲突——
/// 运行期 link_root 是顺序调用，不会真的并发访问 graph。
struct LinkCtxAdapter<'a> {
    space: &'a Arc<Mutex<MemoryAddressSpace>>,
    engine: &'a mut dyn Engine,
    graph_ptr: *mut ModuleGraph,
    regions: &'a mut RegionTracker,
    router: &'a mut TelemetryRouter,
}

impl<'a> LinkCtx for LinkCtxAdapter<'a> {
    fn resolve(&self, query: SymbolQuery<'_>) -> Result<Option<ResolvedSymbol>, ElfLinkError> {
        // SAFETY: link_root 调用 resolve 时不会同时改 graph（schedule 已经完成）。
        // 这里只是为了绕过 Rust 借用检查的"重叠 mut 借用"误报。
        let graph: &ModuleGraph = unsafe { &*self.graph_ptr };
        Ok(rundroid_elf_linker::resolve(graph, query))
    }
    fn write_relocation(&mut self, patch: RelocationPatch) -> Result<(), MemoryError> {
        let bytes = patch.value.to_le_bytes();
        self.engine
            .mem_write(patch.target_addr, &bytes)
            .map_err(|_| MemoryError::NotMapped {
                addr: patch.target_addr,
            })
    }
    fn protect_relro(&mut self, module: ModuleId) -> Result<(), MemoryError> {
        // SAFETY: link_root 在 relocation 写完后再调 protect_relro，graph 不再被 link_root 可变借用。
        let graph: &ModuleGraph = unsafe { &*self.graph_ptr };
        let Some(m) = graph.get(module) else {
            return Ok(());
        };
        let Some(relro) = m.relro else {
            return Ok(());
        };
        // RELRO 切只读：写完 GOT 后禁止修改，并同步共享地址空间账本。
        let start = align_down(relro.start);
        let end = align_up(relro.end, PAGE_SIZE);
        let mut materializer = EngineMemoryMaterializer {
            engine: self.engine,
            regions: self.regions,
        };
        self.space
            .lock()
            .unwrap()
            .protect(start, end - start, AddressSpacePerms::READ, &mut materializer)?;
        self.router.emit(&TelemetryEvent::new(
            "relro.protect",
            TelemetryEventKind::Memory,
        ));
        Ok(())
    }
    fn emit(&mut self, event: TelemetryEvent<'_>) {
        self.router.emit(&event);
    }
}

fn align_up(v: u64, a: u64) -> u64 {
    if a == 0 {
        v
    } else {
        (v + a - 1) & !(a - 1)
    }
}

fn align_down(v: u64) -> u64 {
    v & !(PAGE_SIZE - 1)
}
