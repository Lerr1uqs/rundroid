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
use rundroid_jni::{JniRegistry, RefTable};
use rundroid_linux::{LinuxRuntime, SyscallResult};
use rundroid_memory::{MemoryError, RegionTracker};
use rundroid_telemetry::{EventSink, TelemetryEvent, TelemetryEventKind, TelemetryMode, TelemetryRouter};
use std::sync::{Arc, Mutex};
use thiserror::Error;

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
    /// reserve 游标：bump 分配镜像基址。
    reserve_cursor: u64,
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

impl SyscallHook for SyscallDispatcher {
    fn on_svc(&mut self, cpu: &mut dyn GuestCPU) {
        let nr = cpu.reg_read(Arm64Reg::X(8));
        let x0 = cpu.reg_read(Arm64Reg::X(0));
        let x1 = cpu.reg_read(Arm64Reg::X(1));
        let x2 = cpu.reg_read(Arm64Reg::X(2));
        let x3 = cpu.reg_read(Arm64Reg::X(3));
        let x4 = cpu.reg_read(Arm64Reg::X(4));
        let x5 = cpu.reg_read(Arm64Reg::X(5));

        // read_guest / write_guest 闭包：通过 GuestCPU 访问 guest 内存。
        // 不能直接借用 cpu（已经被 dispatch 借了），所以用裸指针 + 局部 unsafe。
        // SAFETY: cpu 在整个 on_svc 调用期间是稳定的，闭包仅在 dispatch 内同步使用。
        // 闭包返回 bool/Option 让 LinuxRuntime 能把"未映射缓冲"上报成 EFAULT，
        // 避免出现"写没写成功都返回 N"的假阳性。
        let cpu_ptr: *mut dyn GuestCPU = cpu as *mut dyn GuestCPU;
        let mut read_guest = |addr: u64, len: usize| -> Option<Vec<u8>> {
            let mut buf = vec![0u8; len];
            // SAFETY: 见上方 cpu_ptr 论证。
            if unsafe { (*cpu_ptr).mem_read(addr, &mut buf) } {
                Some(buf)
            } else {
                None
            }
        };
        let mut write_guest = |addr: u64, bytes: &[u8]| -> bool {
            // SAFETY: 同上。
            unsafe { (*cpu_ptr).mem_write(addr, bytes) }
        };
        let mut map_guest = |addr: u64, len: usize, prot: i32| -> bool {
            // 把 prot (POSIX PROT_* 位掩码) 转换为 MemPerms 并映射。
            let read = (prot & 1) != 0;
            let write = (prot & 2) != 0;
            let exec = (prot & 4) != 0;
            let perms = rundroid_backend::MemPerms::from_flags(read, write, exec);
            // SAFETY: 同上 cpu_ptr 论证。mem_map 可能因地址已占用而失败。
            unsafe { (*cpu_ptr).mem_map(addr, len, perms).is_ok() }
        };

        let result = {
            let mut linux = self.linux.lock().unwrap();
            linux.dispatch(
                nr, x0, x1, x2, x3, x4, x5, &mut read_guest, &mut write_guest, &mut map_guest,
            )
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
        let sink = CollectingSink::default();
        let router = match config.telemetry {
            TelemetryMode::Disabled => TelemetryRouter::disabled(),
            _ => TelemetryRouter::events_only(Box::new(sink.clone())),
        };

        // syscall hook 与 LinuxRuntime 之间通过 Rc<RefCell<>> 共享。
        // hook 被 install_syscall_hook 注册后，每次 svc 都会回调到这个 LinuxRuntime。
        let linux = Arc::new(Mutex::new(LinuxRuntime::new()));
        engine.install_syscall_hook(Box::new(SyscallDispatcher {
            linux: Arc::clone(&linux),
        }))?;

        // 预映射 guest scratch buffer：1 MiB @ 0x800_000，RW。
        // 让 case manifest 可以把 0x800_000..0x900_000 当作"传入 syscall 的缓冲地址"，
        // 否则 syscall hook 的 write_guest 会因地址未映射而（正确地）返回 EFAULT。
        // 这个区段是 bootstrap case manifest 的公开契约。
        const SCRATCH_ADDR: u64 = 0x800_000;
        const SCRATCH_SIZE: usize = 0x10_0000;
        engine.mem_map(SCRATCH_ADDR, SCRATCH_SIZE, MemPerms::READ_WRITE)?;

        let mut regions = RegionTracker::new();
        regions.register(
            SCRATCH_ADDR,
            SCRATCH_SIZE as u64,
            rundroid_memory::RegionOrigin::RuntimeScratch,
        )?;

        Ok(Self {
            engine,
            regions,
            linux_inner: linux,
            allocator: IdAllocator::new(),
            router,
            sink: Some(sink),
            config,
            graph: ModuleGraph::new(),
            last_link: None,
            jni_registry: JniRegistry::new(),
            jni_refs: RefTable::new(),
            // 镜像装载起点：1 GiB 处，远离 stack/TLS 高端。
            reserve_cursor: 0x4000_0000,
            trampoline_mapped: false,
        })
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
                engine,
                graph,
                router,
                ..
            } = self;
            // 见下方注释：link_root 同时需要 &mut graph 与 &mut dyn LinkContext，
            // 而 resolve 又要读 graph。运行期是顺序的，借用裸指针绕过编译期借用检查。
            let graph_ptr: *mut ModuleGraph = graph;
            let mut link_ctx = LinkCtxAdapter {
                engine: engine.as_mut(),
                graph_ptr,
                router,
            };
            DefaultLinker::new().link_root(&mut link_ctx, graph, root_id)?
        };
        self.last_link = Some(report);
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
        let module_segments: Vec<rundroid_elf_loader::MappedSegmentInfo>;
        {
            let Self {
                engine,
                regions,
                router,
                reserve_cursor,
                ..
            } = self;
            let mut ctx = LoadCtxAdapter {
                engine: engine.as_mut(),
                regions,
                router,
                next_reserve: reserve_cursor,
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
            module_segments = module.segments.clone();
            self.graph.insert(module, parsed.dynamic.soname.clone());
        }

        // review finding 3：footprint 装载时整块按 RWX 映射，
        // 这里按 PT_LOAD 的精确 p_flags 收紧权限，避免 RWX 掩盖错误。
        // 失败只记 telemetry 不阻塞装载——某些 backend 可能不支持 mem_protect。
        for seg in &module_segments {
            let p = MemPerms::from_flags(seg.perms.read, seg.perms.write, seg.perms.execute);
            if let Err(_e) = self.engine.mem_protect(seg.guest_addr, seg.size as usize, p) {
                self.router.emit(&TelemetryEvent::new(
                    "mem.protect_failed",
                    TelemetryEventKind::Memory,
                ));
            }
        }

        Ok(module_id)
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
        use rundroid_backend::{Arm64Reg, MemPerms};

        // sentinel + stack 只在第一次调用时映射；后续复用。
        // 否则在同一 runtime 内多次 call 会触发重叠 mem_map。
        const SENTINEL_ADDR: u64 = 0x7F_FFFF_0000;
        const STACK_BASE: u64 = 0x7F_E000_0000;
        const STACK_TOP: u64 = STACK_BASE + 0x10_0000;
        if !self.trampoline_mapped {
            self.engine.mem_map(SENTINEL_ADDR, 0x1000, MemPerms::READ_EXEC)?;
            self.engine.mem_map(STACK_BASE, 0x10_0000, MemPerms::READ_WRITE)?;
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

    /// 取出当前累积的 telemetry 事件。
    pub fn take_events(&mut self) -> Vec<EventRecord> {
        if let Some(sink) = self.sink.as_ref() {
            let mut g = sink.events.lock().unwrap();
            std::mem::take(&mut *g)
        } else {
            Vec::new()
        }
    }

    /// 遍历所有已加载模块，查找并调用 `JNI_OnLoad`。
    ///
    /// 对于每个导出 `JNI_OnLoad` 符号的模块，
    /// 记录 telemetry 事件并尝试调用。
    ///
    /// # foundation 阶段行为
    ///
    /// - 检测到 `JNI_OnLoad` 符号后发出 `jni.call` telemetry 事件
    /// - 不实际执行 `JNI_OnLoad`（因为需要完整的 `JavaVM*` guest 函数表）
    /// - 返回找到的 `JNI_OnLoad` 符号所在的模块列表
    pub fn detect_jni_onload(&mut self) -> Vec<(ModuleId, String, u64)> {
        let mut found = Vec::new();

        for (&module_id, module) in &self.graph.modules {
            if let Some(entry) = module.exports.find("JNI_OnLoad") {
                let soname = module.name.clone();
                found.push((module_id, soname.clone(), entry.guest_addr));

                self.router.emit(&TelemetryEvent::new(
                    "jni.call",
                    TelemetryEventKind::Jni,
                ));
                self.router.emit(&TelemetryEvent::new(
                    "jni.jni_onload_detected",
                    TelemetryEventKind::Jni,
                ));
            }
        }

        found
    }
}

struct LoadCtxAdapter<'a> {
    engine: &'a mut dyn Engine,
    regions: &'a mut RegionTracker,
    router: &'a mut TelemetryRouter,
    next_reserve: &'a mut u64,
}

impl<'a> LoadCtx for LoadCtxAdapter<'a> {
    fn reserve_image_space(&mut self, size: u64, align: u64) -> Result<u64, MemoryError> {
        // 一次性映射整个镜像 footprint。
        // 这样做有两个原因：
        // 1. Unicorn 要求 mem_map 的 addr/size 都 page 对齐；
        //    ELF 段是 footprint 内的子区间，逐段映射容易触发对齐失败或段间 page 重叠。
        // 2. ELF 段经常共享同一 page（例如 RX 段末尾 + RW 段开头的边界 page），
        //    Unicorn 拒绝重叠 mem_map，因此必须把整个 footprint 作为一整块映射。
        //
        // 权限：用 RWX 全开。精确的段级权限切换在 RELRO 写回之后进行。
        let aligned = align_up(size, 0x1000);
        let base = align_up(*self.next_reserve, align.max(0x1000));
        self.engine
            .mem_map(base, aligned as usize, MemPerms::ALL)
            .map_err(|_| MemoryError::InvalidSize {
                size: aligned,
                reason: "backend mem_map rejected footprint",
            })?;
        self.regions.register(
            base,
            aligned,
            rundroid_memory::RegionOrigin::ELFSegment,
        )?;
        *self.next_reserve = base + aligned;
        Ok(base)
    }
    fn map_segment(&mut self, spec: SegmentMapSpec<'_>) -> Result<MappedSegment, MemoryError> {
        // footprint 已经在 reserve_image_space 里整块映射 + 记账，
        // 这里不再调用 backend.mem_map，也不重复注册 region（会触发 Overlap）。
        Ok(MappedSegment {
            guest_addr: spec.guest_addr,
            size: spec.size,
        })
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

/// LinkContext 适配器。
///
/// 由于 [`DefaultLinker::link_root`] 同时持有 `&mut ModuleGraph` 和
/// `&mut dyn LinkContext`，而 resolve 又需要读 graph，
/// 这里用裸指针让两者"逻辑上"不冲突——
/// 运行期 link_root 是顺序调用，不会真的并发访问 graph。
struct LinkCtxAdapter<'a> {
    engine: &'a mut dyn Engine,
    graph_ptr: *mut ModuleGraph,
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
        // RELRO 切只读：写完 GOT 后禁止修改。
        let range = relro.end - relro.start;
        let perms = MemPerms::READ;
        self.engine
            .mem_protect(relro.start, range as usize, perms)
            .map_err(|_| MemoryError::NotMapped { addr: relro.start })?;
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
