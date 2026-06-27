//! JNI ABI surfaces — guest 可见的 `_JavaVM` / `_JNIEnv` ABI 对象。
//!
//! 把 guest address space 中的 JNI ABI 表面收敛为一等类型：
//! - [`JNIEnvABI`]：guest 可见的 `JNIEnv*`（函数指针表 + trampoline 布局），
//!   是 [`crate::function_table::JniFunctionTable`] 的语义收敛命名。
//! - [`JavaVMABI`]：guest 可见的 `JavaVM*`（invoke table + trampoline 布局），
//!   覆盖 `GetEnv` / `AttachCurrentThread` / `DetachCurrentThread`。
//! - [`JniSlotSpec`]：每个 ABI slot 的统一元数据（name / offset / handler），
//!   数据驱动 dispatch 与 telemetry 的声明式 catalog。
//!
//! # 分层约束
//!
//! 本模块随 crate `forbid(unsafe_code)` 且**不依赖 backend**——
//! 只负责"guest 内存布局 + slot 元数据 + GetEnv/Attach/Detach 纯状态逻辑"。
//! "解 CPU 寄存器 + 调 [`crate::jnienv::JniEnvSurface`] dispatch"强依赖 backend
//! 的 `GuestCPU`，留给装配层（case-runner）按 slot 元数据分流实现。
//!
//! # slot 元数据驱动的边界
//!
//! catalog 驱动的是**分流决策与 telemetry**（已实现走 Bridge、未实现 fail-fast、
//! 每次调用能报名），而每个 Bridge slot 的"解参 + 调 dispatch"实现仍是一段代码
//! ——因为各 JNI entry 的参数布局不同，无法纯数据驱动。这契合 design 的
//! "通过 slot metadata 找到 handler"：metadata 定位 handler 归属，handler 本体在装配层。

use crate::error::JniError;
use crate::function_table::{
    align_up, JniFunctionTable, ARM64_NOP, JNI_TABLE_SIZE, TRAMPOLINE_SLOT_SIZE,
};
use crate::native_registry::validate_jni_version;

// ============================================================================
// JNI 通用返回码（invoke table 入口语义）
// ============================================================================

/// JNI_OK — 操作成功。
pub const JNI_OK: u64 = 0;
/// JNI_ERR (-1) — 通用错误。
pub const JNI_ERR: u64 = u64::MAX;
/// JNI_EDETACHED (-2) — 线程未 attach 到 JavaVM。
pub const JNI_EDETACHED: u64 = u64::MAX - 1;
/// JNI_EVERSION (-3) — 不支持的 JNI version。
pub const JNI_EVERSION: u64 = u64::MAX - 2;

// ============================================================================
// ABI slot 元数据模型
// ============================================================================

/// 一个 ABI slot 的 handler 归属。
///
/// jni crate 只持有 slot 的**声明式 catalog**（是否已实现），不持有 handler 实现
/// 本身——后者依赖 backend `GuestCPU`，由装配层提供。装配层按 [`JniSlotSpec`]
/// 的 offset/name 分流到具体桥接 handler。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JniSlotHandler {
    /// 已实现的桥接 slot：装配层按 catalog 提供解参 + dispatch。
    Bridge,
    /// 尚未实现的 slot：guest 调用时 fail-fast 返回错误，不静默通过。
    Unimplemented,
}

/// 单个 ABI slot 的统一元数据：name / offset / handler。
///
/// 对应 design 要求的 slot 元数据模型。`offset` 是 function/invoke table 内的
/// 索引；guest 侧的"拦截地址"（trampoline guest address）由 layout 后按
/// `trampoline_base + offset * TRAMPOLINE_SLOT_SIZE` 计算，不进静态 catalog
/// （依赖运行期 base 地址）。
#[derive(Debug, Clone, Copy)]
pub struct JniSlotSpec {
    /// JNI entry 名（如 `"FindClass"` / `"GetEnv"`），供 telemetry 与日志。
    pub name: &'static str,
    /// function / invoke table 内的槽位索引。
    pub offset: usize,
    /// handler 归属（已实现 / 未实现）。
    pub handler: JniSlotHandler,
}

impl JniSlotSpec {
    /// 构造一个已实现的 Bridge slot。
    pub const fn bridge(name: &'static str, offset: usize) -> Self {
        Self { name, offset, handler: JniSlotHandler::Bridge }
    }

    /// 构造一个未实现的 slot（标记覆盖计划 + 未命中时 fail-fast 报名）。
    pub const fn unimplemented(name: &'static str, offset: usize) -> Self {
        Self { name, offset, handler: JniSlotHandler::Unimplemented }
    }
}

/// 在静态 catalog 中按 offset 查 slot 元数据。
///
/// 未列入 catalog 的 slot 返回 `None`（装配层按"未实现"处理）。
fn find_slot(catalog: &'static [JniSlotSpec], offset: usize) -> Option<&'static JniSlotSpec> {
    catalog.iter().find(|s| s.offset == offset)
}

// ============================================================================
// JNIEnvABI — guest 可见的 JNIEnv ABI 对象
// ============================================================================

/// JNIEnv function table slot 的声明式 catalog。
///
/// 列出已桥接（`Bridge`）与关键未实现（`Unimplemented`）的 JNI entry，
/// 供装配层数据驱动 dispatch 与 telemetry。索引与 [`crate::function_table`]
/// 常量、unidbg DalvikVM64 对齐。
pub static JNI_ENV_SLOTS: &[JniSlotSpec] = &[
    // —— version ——
    JniSlotSpec::bridge("GetVersion", crate::function_table::JNI_GET_VERSION),
    // —— class / member lookup ——
    JniSlotSpec::bridge("FindClass", crate::function_table::JNI_FIND_CLASS),
    JniSlotSpec::bridge("GetMethodID", crate::function_table::JNI_GET_METHOD_ID),
    JniSlotSpec::bridge(
        "GetStaticMethodID",
        crate::function_table::JNI_GET_STATIC_METHOD_ID,
    ),
    JniSlotSpec::bridge("GetFieldID", crate::function_table::JNI_GET_FIELD_ID),
    JniSlotSpec::bridge(
        "GetStaticFieldID",
        crate::function_table::JNI_GET_STATIC_FIELD_ID,
    ),
    // —— object 实例化 ——
    JniSlotSpec::bridge("AllocObject", crate::function_table::JNI_ALLOC_OBJECT),
    JniSlotSpec::bridge("NewObject", crate::function_table::JNI_NEW_OBJECT),
    JniSlotSpec::bridge(
        "GetObjectClass",
        crate::function_table::JNI_GET_OBJECT_CLASS,
    ),
    // —— Call*Method (instance) ——
    JniSlotSpec::bridge(
        "CallObjectMethod",
        crate::function_table::JNI_CALL_OBJECT_METHOD,
    ),
    JniSlotSpec::bridge(
        "CallBooleanMethod",
        crate::function_table::JNI_CALL_BOOLEAN_METHOD,
    ),
    JniSlotSpec::bridge("CallByteMethod", crate::function_table::JNI_CALL_BYTE_METHOD),
    JniSlotSpec::bridge("CallCharMethod", crate::function_table::JNI_CALL_CHAR_METHOD),
    JniSlotSpec::bridge(
        "CallShortMethod",
        crate::function_table::JNI_CALL_SHORT_METHOD,
    ),
    JniSlotSpec::bridge("CallIntMethod", crate::function_table::JNI_CALL_INT_METHOD),
    JniSlotSpec::bridge("CallLongMethod", crate::function_table::JNI_CALL_LONG_METHOD),
    JniSlotSpec::bridge(
        "CallFloatMethod",
        crate::function_table::JNI_CALL_FLOAT_METHOD,
    ),
    JniSlotSpec::bridge(
        "CallDoubleMethod",
        crate::function_table::JNI_CALL_DOUBLE_METHOD,
    ),
    JniSlotSpec::bridge("CallVoidMethod", crate::function_table::JNI_CALL_VOID_METHOD),
    // —— CallStatic*Method ——
    JniSlotSpec::bridge(
        "CallStaticVoidMethod",
        crate::function_table::JNI_CALL_STATIC_VOID_METHOD,
    ),
    JniSlotSpec::bridge(
        "CallStaticIntMethod",
        crate::function_table::JNI_CALL_STATIC_INT_METHOD,
    ),
    JniSlotSpec::bridge(
        "CallStaticObjectMethod",
        crate::function_table::JNI_CALL_STATIC_OBJECT_METHOD,
    ),
    JniSlotSpec::bridge(
        "CallStaticBooleanMethod",
        crate::function_table::JNI_CALL_STATIC_BOOLEAN_METHOD,
    ),
    JniSlotSpec::bridge(
        "CallStaticLongMethod",
        crate::function_table::JNI_CALL_STATIC_LONG_METHOD,
    ),
    // —— Field get/set ——
    JniSlotSpec::bridge(
        "GetObjectField",
        crate::function_table::JNI_GET_OBJECT_FIELD,
    ),
    JniSlotSpec::bridge("GetIntField", crate::function_table::JNI_GET_INT_FIELD),
    JniSlotSpec::bridge("SetIntField", crate::function_table::JNI_SET_INT_FIELD),
    JniSlotSpec::bridge(
        "SetObjectField",
        crate::function_table::JNI_SET_OBJECT_FIELD,
    ),
    // —— String ——
    JniSlotSpec::bridge("NewStringUTF", crate::function_table::JNI_NEW_STRING_UTF),
    JniSlotSpec::unimplemented(
        "GetStringUTFChars",
        crate::function_table::JNI_GET_STRING_UTF_CHARS,
    ),
    // —— Exception ——
    JniSlotSpec::bridge(
        "ExceptionOccurred",
        crate::function_table::JNI_EXCEPTION_OCCURRED,
    ),
    JniSlotSpec::bridge(
        "ExceptionClear",
        crate::function_table::JNI_EXCEPTION_CLEAR,
    ),
    // —— Reference 管理 ——
    JniSlotSpec::bridge("NewGlobalRef", crate::function_table::JNI_NEW_GLOBAL_REF),
    JniSlotSpec::bridge(
        "DeleteGlobalRef",
        crate::function_table::JNI_DELETE_GLOBAL_REF,
    ),
    JniSlotSpec::bridge(
        "DeleteLocalRef",
        crate::function_table::JNI_DELETE_LOCAL_REF,
    ),
    JniSlotSpec::bridge("NewLocalRef", crate::function_table::JNI_NEW_LOCAL_REF),
    // —— RegisterNatives ——
    JniSlotSpec::bridge(
        "RegisterNatives",
        crate::function_table::JNI_REGISTER_NATIVES,
    ),
    // —— GetJavaVM：返回当前 JavaVM*（bootstrap 阶段尚未接 guest 出参写出）——
    JniSlotSpec::unimplemented("GetJavaVM", crate::function_table::JNI_GET_JAVA_VM),
];

/// Guest 可见的 `JNIEnv*` ABI 对象。
///
/// 持有函数指针表 + trampoline 的 guest 内存布局（[`JniFunctionTable`]），
/// 并暴露 slot 元数据 catalog（[`JNI_ENV_SLOTS`]）。是 foundation 阶段
/// `JniFunctionTable` 的语义收敛命名——layout 计算与 guest 写入逻辑完全复用，
/// 不重复实现。
#[derive(Debug, Clone)]
pub struct JNIEnvABI {
    /// guest 内存布局（env struct / function table / trampoline）。
    pub layout: JniFunctionTable,
}

impl JNIEnvABI {
    /// 按 guest base 地址计算 JNIEnv ABI 布局（不实际映射）。
    pub fn new(base: u64) -> Self {
        Self {
            layout: JniFunctionTable::layout(base),
        }
    }

    /// JNIEnv 结构体 guest 地址（即 guest 拿到的 `JNIEnv*`）。
    pub fn env_ptr(&self) -> u64 {
        self.layout.env_ptr
    }

    /// trampoline 页 guest 起始地址（code hook 覆盖起点）。
    pub fn trampoline_begin(&self) -> u64 {
        self.layout.trampoline_base
    }

    /// trampoline 页 guest 结束地址（含）。
    pub fn trampoline_end(&self) -> u64 {
        self.layout.trampoline_base + (JNI_TABLE_SIZE as u64) * TRAMPOLINE_SLOT_SIZE - 1
    }

    /// guest 内存映射总大小（env struct + function table + trampoline）。
    pub fn total_size(&self) -> usize {
        self.layout.total_size
    }

    /// JNIEnv function table slot catalog。
    pub fn slots() -> &'static [JniSlotSpec] {
        JNI_ENV_SLOTS
    }

    /// 按 table 索引查 slot 元数据（未列入 catalog 返回 `None`）。
    pub fn slot_spec(offset: usize) -> Option<&'static JniSlotSpec> {
        find_slot(JNI_ENV_SLOTS, offset)
    }

    /// 计算 slot `offset` 对应的 trampoline guest 地址（拦截地址）。
    pub fn slot_guest_address(&self, offset: usize) -> u64 {
        self.layout.trampoline_base + (offset as u64) * TRAMPOLINE_SLOT_SIZE
    }

    /// 从 trampoline 地址反算 function table 索引（let-it-failed：越界 panic）。
    pub fn function_index(&self, address: u64) -> usize {
        // TODO: maybe rename to function_index_of
        self.layout.function_index(address)
    }

    /// 把 env struct + function table + trampoline 写入 guest 内存。
    ///
    /// `mem_write(addr, bytes) -> bool` 由装配层提供（包装 backend `mem_write`）。
    pub fn write_to_guest(
        &self,
        mem_write: &mut dyn FnMut(u64, &[u8]) -> bool,
    ) -> Result<(), String> {
        self.layout.write_table_to_guest(mem_write)
    }
}

// ============================================================================
// JavaVMABI — guest 可见的 JavaVM ABI 对象（invoke table）
// ============================================================================

/// JNI invoke table 槽位索引（与 JNI 规范 `JNIInvokeInterface` 一致）。
///
/// `JNIInvokeInterface` 前 3 个槽位为 reserved NULL，随后是 5 个 invoke 入口。
/// guest 通过 `(*vm)->GetEnv(vm, &env, version)` 调用。
pub const JNI_INVOKE_RESERVED_COUNT: usize = 3;
/// DestroyJavaVM — 销毁 JavaVM（bootstrap 不支持）。
pub const JNI_INVOKE_DESTROY_JAVA_VM: usize = 3;
/// AttachCurrentThread — 把当前线程 attach 到 JavaVM。
pub const JNI_INVOKE_ATTACH_CURRENT_THREAD: usize = 4;
/// DetachCurrentThread — 把当前线程从 JavaVM detach。
pub const JNI_INVOKE_DETACH_CURRENT_THREAD: usize = 5;
/// GetEnv — 取当前线程的 JNIEnv。
pub const JNI_INVOKE_GET_ENV: usize = 6;
/// AttachCurrentThreadAsDaemon（bootstrap 不支持）。
pub const JNI_INVOKE_ATTACH_AS_DAEMON: usize = 7;
/// invoke table 总槽位（3 reserved + 5 入口）。
pub const JNI_INVOKE_TABLE_SIZE: usize = 8;

/// JavaVM invoke table slot 的声明式 catalog。
pub static JNI_INVOKE_SLOTS: &[JniSlotSpec] = &[
    JniSlotSpec::unimplemented("DestroyJavaVM", JNI_INVOKE_DESTROY_JAVA_VM),
    JniSlotSpec::bridge(
        "AttachCurrentThread",
        JNI_INVOKE_ATTACH_CURRENT_THREAD,
    ),
    JniSlotSpec::bridge(
        "DetachCurrentThread",
        JNI_INVOKE_DETACH_CURRENT_THREAD,
    ),
    JniSlotSpec::bridge("GetEnv", JNI_INVOKE_GET_ENV),
    JniSlotSpec::unimplemented(
        "AttachCurrentThreadAsDaemon",
        JNI_INVOKE_ATTACH_AS_DAEMON,
    ),
];

/// Guest 可见的 `JavaVM*` ABI 对象。
///
/// 内存布局（base 起，紧跟 [`JNIEnvABI`] 区域之后）：
/// - `_JavaVM` 结构体（首字段 = 指向 invoke table 的指针）
/// - invoke table（`JNI_INVOKE_TABLE_SIZE` × 8 字节指针，各指向 trampoline）
/// - invoke trampoline 页（每槽 4 字节 NOP，code hook 拦截）
#[derive(Debug, Clone)]
pub struct JavaVMABI {
    /// `_JavaVM` 结构体 guest 地址（guest 拿到的 `JavaVM*`）。
    pub vm_ptr: u64,
    /// invoke table guest 地址（`_JavaVM` 首字段指向此处）。
    pub invoke_table_ptr: u64,
    /// invoke trampoline 页 guest 起始地址。
    pub trampoline_base: u64,
    /// 映射总大小（字节）。
    pub total_size: usize,
}

impl JavaVMABI {
    /// 按 guest base 地址计算 JavaVM ABI 布局（不实际映射）。
    ///
    /// `base` 即 `_JavaVM` 结构体地址。布局：1 页 struct + invoke table 页 +
    /// trampoline 页，均 page 对齐。
    pub fn new(base: u64) -> Self {
        const PAGE: u64 = 0x1000;
        let vm_ptr = base;
        let invoke_table_ptr = base + PAGE;
        let invoke_table_bytes = (JNI_INVOKE_TABLE_SIZE * 8) as u64;
        let invoke_table_size = align_up(invoke_table_bytes, PAGE) as usize;
        let trampoline_base = invoke_table_ptr + invoke_table_size as u64;
        let tramp_bytes = (JNI_INVOKE_TABLE_SIZE as u64) * TRAMPOLINE_SLOT_SIZE;
        let trampoline_size = align_up(tramp_bytes, PAGE) as usize;
        let total_size = PAGE as usize + invoke_table_size + trampoline_size;

        Self {
            vm_ptr,
            invoke_table_ptr,
            trampoline_base,
            total_size,
        }
    }

    /// `_JavaVM` 结构体 guest 地址（即 guest 拿到的 `JavaVM*`）。
    pub fn vm_ptr(&self) -> u64 {
        self.vm_ptr
    }

    /// guest 内存映射总大小（struct + invoke table + trampoline）。
    pub fn total_size(&self) -> usize {
        self.total_size
    }

    /// invoke trampoline 页 guest 起始地址（code hook 覆盖起点）。
    pub fn trampoline_begin(&self) -> u64 {
        self.trampoline_base
    }

    /// invoke trampoline 页 guest 结束地址（含）。
    pub fn trampoline_end(&self) -> u64 {
        self.trampoline_base + (JNI_INVOKE_TABLE_SIZE as u64) * TRAMPOLINE_SLOT_SIZE - 1
    }

    /// JavaVM invoke table slot catalog。
    pub fn slots() -> &'static [JniSlotSpec] {
        JNI_INVOKE_SLOTS
    }

    /// 按 invoke table 索引查 slot 元数据（未列入 catalog 返回 `None`）。
    pub fn slot_spec(offset: usize) -> Option<&'static JniSlotSpec> {
        find_slot(JNI_INVOKE_SLOTS, offset)
    }

    /// 计算 invoke slot `offset` 对应的 trampoline guest 地址（拦截地址）。
    pub fn slot_guest_address(&self, offset: usize) -> u64 {
        self.trampoline_base + (offset as u64) * TRAMPOLINE_SLOT_SIZE
    }

    /// 从 invoke trampoline 地址反算 slot 索引（let-it-failed：越界 panic）。
    pub fn function_index(&self, address: u64) -> usize {
        let offset = address.saturating_sub(self.trampoline_base);
        assert!(
            offset < (JNI_INVOKE_TABLE_SIZE as u64) * TRAMPOLINE_SLOT_SIZE,
            "JavaVM invoke trampoline address 0x{address:X} out of range (base=0x{:X})",
            self.trampoline_base
        );
        (offset / TRAMPOLINE_SLOT_SIZE) as usize
    }

    /// 把 `_JavaVM` struct + invoke table + trampoline 写入 guest 内存。
    ///
    /// `mem_write(addr, bytes) -> bool` 由装配层提供。返回首个写入失败的地址描述。
    pub fn write_to_guest(
        &self,
        mem_write: &mut dyn FnMut(u64, &[u8]) -> bool,
    ) -> Result<(), String> {
        // 1. _JavaVM 首字段 = 指向 invoke table 的指针
        if !mem_write(self.vm_ptr, &self.invoke_table_ptr.to_le_bytes()) {
            return Err(format!("写入 _JavaVM header 失败 @ 0x{:X}", self.vm_ptr));
        }

        // 2. invoke table：每个 slot 指向对应 trampoline 地址
        for i in 0..JNI_INVOKE_TABLE_SIZE {
            let tramp = self.trampoline_base + (i as u64) * TRAMPOLINE_SLOT_SIZE;
            let slot = self.invoke_table_ptr + (i as u64) * 8;
            if !mem_write(slot, &tramp.to_le_bytes()) {
                return Err(format!("写入 invoke table slot {i} 失败 @ 0x{:X}", slot));
            }
        }

        // 3. invoke trampoline 页：全部 NOP
        for i in 0..JNI_INVOKE_TABLE_SIZE {
            let tramp = self.trampoline_base + (i as u64) * TRAMPOLINE_SLOT_SIZE;
            mem_write(tramp, &ARM64_NOP);
        }

        Ok(())
    }
}

// ============================================================================
// GetEnv / AttachCurrentThread / DetachCurrentThread 纯状态逻辑
// ============================================================================

/// JavaVM 当前线程 attach 状态（bootstrap 单线程模型）。
///
/// GetEnv/Attach/Detach 的纯状态机在此之上运算，**不碰 guest 内存**——
/// "把 env_ptr 写入 guest 出参 + 把返回码写回 x0"由装配层（持有 `GuestCPU`）完成。
#[derive(Debug, Clone)]
pub struct JavaVMThreadState {
    /// 当前线程是否已 attach。
    pub attached: bool,
    /// 当前 active `JNIEnv*`（guest 指针）。
    pub env_ptr: u64,
}

impl JavaVMThreadState {
    /// 创建已 attach 的主线程状态。
    ///
    /// JNI_OnLoad 之前 runtime 已 attach 主线程，故 main thread 默认 attached。
    pub fn main_thread(env_ptr: u64) -> Self {
        Self {
            attached: true,
            env_ptr,
        }
    }

    /// 当前 active env_ptr（已 attach 时有效）。
    pub fn env_ptr(&self) -> u64 {
        self.env_ptr
    }

    /// 当前线程是否已 attach。
    pub fn is_attached(&self) -> bool {
        self.attached
    }
}

/// `GetEnv(JavaVM*, void** env, jint version)` 的纯逻辑。
///
/// 校验 version，返回应写入 env 出参的 env_ptr。装配层负责把返回的 env_ptr
/// 写入 guest 的 `*env`、把 [`JNI_OK`] 写回 x0；version 非法时装配层写
/// [`JNI_EVERSION`] 到 x0。
pub fn apply_get_env(state: &JavaVMThreadState, version: u64) -> Result<u64, JniError> {
    if !validate_jni_version(version) {
        return Err(JniError::Internal(format!(
            "GetEnv: 不支持的 JNI version 0x{version:08X}"
        )));
    }
    if !state.attached {
        // JNI 规范：未 attach 时 GetEnv 行为未定义；bootstrap 显式失败便于调试。
        return Err(JniError::Internal("GetEnv: 当前线程未 attach".into()));
    }
    Ok(state.env_ptr)
}

/// `AttachCurrentThread(JavaVM*, JNIEnv** env, void*)` 的纯逻辑。
///
/// 标记当前线程已 attach，返回 env_ptr。重复 attach 幂等（JNI 语义允许）。
pub fn apply_attach_current_thread(state: &mut JavaVMThreadState) -> Result<u64, JniError> {
    state.attached = true;
    Ok(state.env_ptr)
}

/// `DetachCurrentThread(JavaVM*)` 的纯逻辑。
///
/// 清除 attach 标志。返回成功。
pub fn apply_detach_current_thread(state: &mut JavaVMThreadState) -> Result<(), JniError> {
    state.attached = false;
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::function_table::JNI_FIND_CLASS;

    // —— JNIEnvABI layout ——

    #[test]
    fn jnienv_abi_layout_addresses() {
        let base = 0x7F_C000_0000u64;
        let abi = JNIEnvABI::new(base);

        assert_eq!(abi.env_ptr(), base);
        assert_eq!(abi.trampoline_begin() % 0x1000, 0, "trampoline 应 page 对齐");
        assert!(abi.trampoline_end() > abi.trampoline_begin());
        assert!(abi.total_size() > 0);
    }

    #[test]
    fn jnienv_abi_slot_address_round_trip() {
        let abi = JNIEnvABI::new(0x7F_C000_0000);
        // FindClass offset = 6 的 trampoline 地址反算回 6
        let addr = abi.slot_guest_address(JNI_FIND_CLASS);
        assert_eq!(abi.function_index(addr), JNI_FIND_CLASS);
    }

    #[test]
    fn jnienv_slots_catalog_no_duplicate_offset() {
        let mut offsets: Vec<usize> = JNI_ENV_SLOTS.iter().map(|s| s.offset).collect();
        offsets.sort_unstable();
        let before = offsets.len();
        offsets.dedup();
        assert_eq!(offsets.len(), before, "JNI_ENV_SLOTS offset 不能重复");
    }

    #[test]
    fn jnienv_slot_spec_known_entries() {
        // 关键主线 slot 都应是 Bridge
        assert_eq!(
            JNIEnvABI::slot_spec(JNI_FIND_CLASS).unwrap().handler,
            JniSlotHandler::Bridge
        );
        assert_eq!(
            JNIEnvABI::slot_spec(crate::function_table::JNI_NEW_OBJECT)
                .unwrap()
                .handler,
            JniSlotHandler::Bridge
        );
        // GetStringUTFChars 显式标 Unimplemented
        assert_eq!(
            JNIEnvABI::slot_spec(crate::function_table::JNI_GET_STRING_UTF_CHARS)
                .unwrap()
                .handler,
            JniSlotHandler::Unimplemented
        );
    }

    #[test]
    fn jnienv_slot_spec_unknown_returns_none() {
        // offset 5 未列入 catalog
        assert!(JNIEnvABI::slot_spec(5).is_none());
    }

    // —— JavaVMABI layout ——

    #[test]
    fn javavm_abi_invoke_indices_match_jni_spec() {
        // JNIInvokeInterface：前 3 reserved，Destroy=3, Attach=4, Detach=5, GetEnv=6
        assert_eq!(JNI_INVOKE_DESTROY_JAVA_VM, 3);
        assert_eq!(JNI_INVOKE_ATTACH_CURRENT_THREAD, 4);
        assert_eq!(JNI_INVOKE_DETACH_CURRENT_THREAD, 5);
        assert_eq!(JNI_INVOKE_GET_ENV, 6);
        assert_eq!(JNI_INVOKE_TABLE_SIZE, 8);
    }

    #[test]
    fn javavm_abi_layout_addresses() {
        let base = 0x7F_C000_3000u64;
        let abi = JavaVMABI::new(base);

        assert_eq!(abi.vm_ptr(), base);
        assert!(abi.invoke_table_ptr > abi.vm_ptr);
        assert!(abi.trampoline_base > abi.invoke_table_ptr);
        assert_eq!(abi.trampoline_begin() % 0x1000, 0);
        assert!(abi.total_size() > 0);
    }

    #[test]
    fn javavm_abi_function_index_round_trip() {
        let abi = JavaVMABI::new(0x7F_C000_3000);
        let addr = abi.slot_guest_address(JNI_INVOKE_GET_ENV);
        assert_eq!(abi.function_index(addr), JNI_INVOKE_GET_ENV);
        assert_eq!(
            abi.function_index(abi.slot_guest_address(JNI_INVOKE_ATTACH_CURRENT_THREAD)),
            JNI_INVOKE_ATTACH_CURRENT_THREAD
        );
    }

    #[test]
    #[should_panic]
    fn javavm_abi_function_index_out_of_range_panics() {
        let abi = JavaVMABI::new(0x7F_C000_3000);
        abi.function_index(abi.trampoline_end() + 4);
    }

    #[test]
    fn javavm_slots_catalog_getenv_is_bridge() {
        assert_eq!(
            JavaVMABI::slot_spec(JNI_INVOKE_GET_ENV).unwrap().handler,
            JniSlotHandler::Bridge
        );
        assert_eq!(
            JavaVMABI::slot_spec(JNI_INVOKE_DESTROY_JAVA_VM)
                .unwrap()
                .handler,
            JniSlotHandler::Unimplemented
        );
    }

    // —— write_to_guest ——

    #[test]
    fn javavm_abi_write_to_guest_layout() {
        use std::cell::RefCell;
        use std::collections::HashMap;
        use std::rc::Rc;

        let abi = JavaVMABI::new(0x7F_C000_3000);
        let store: Rc<RefCell<HashMap<u64, Vec<u8>>>> = Rc::new(RefCell::new(HashMap::new()));
        let s = Rc::clone(&store);
        let mut write = move |addr: u64, bytes: &[u8]| {
            s.borrow_mut().insert(addr, bytes.to_vec());
            true
        };

        abi.write_to_guest(&mut write).unwrap();

        // _JavaVM 首字段 = invoke table 指针
        let header = u64::from_le_bytes(
            store.borrow().get(&abi.vm_ptr).unwrap().clone().try_into().unwrap(),
        );
        assert_eq!(header, abi.invoke_table_ptr);

        // invoke table slot 6 (GetEnv) 指向 GetEnv trampoline
        let slot6_addr = abi.invoke_table_ptr + (JNI_INVOKE_GET_ENV as u64) * 8;
        let slot6_val = u64::from_le_bytes(
            store.borrow().get(&slot6_addr).unwrap().clone().try_into().unwrap(),
        );
        assert_eq!(slot6_val, abi.slot_guest_address(JNI_INVOKE_GET_ENV));

        // trampoline[GetEnv] = ARM64 NOP
        let tramp = abi.slot_guest_address(JNI_INVOKE_GET_ENV);
        assert_eq!(store.borrow().get(&tramp).unwrap(), &ARM64_NOP);
    }

    #[test]
    fn javavm_abi_write_to_guest_failure_reported() {
        let abi = JavaVMABI::new(0x7F_C000_3000);
        let mut fail_all = |_addr: u64, _bytes: &[u8]| false;
        assert!(abi.write_to_guest(&mut fail_all).is_err());
    }

    // —— GetEnv / Attach / Detach 纯逻辑 ——

    #[test]
    fn javavm_thread_state_main_thread_attached() {
        let st = JavaVMThreadState::main_thread(0x7F_C000_0000);
        assert!(st.is_attached());
        assert_eq!(st.env_ptr(), 0x7F_C000_0000);
    }

    #[test]
    fn apply_get_env_accepts_supported_version() {
        let st = JavaVMThreadState::main_thread(0xAA);
        assert_eq!(apply_get_env(&st, 0x0001_0006).unwrap(), 0xAA);
        assert_eq!(apply_get_env(&st, 0x0001_0008).unwrap(), 0xAA);
    }

    #[test]
    fn apply_get_env_rejects_bad_version() {
        let st = JavaVMThreadState::main_thread(0xAA);
        assert!(apply_get_env(&st, 0x0001_0007).is_err());
        assert!(apply_get_env(&st, 0).is_err());
    }

    #[test]
    fn apply_get_env_rejects_unattached() {
        let mut st = JavaVMThreadState::main_thread(0xAA);
        st.attached = false;
        assert!(apply_get_env(&st, 0x0001_0006).is_err());
    }

    #[test]
    fn apply_attach_and_detach_state_machine() {
        let mut st = JavaVMThreadState::main_thread(0xAA);
        st.attached = false;

        // attach → 返回 env_ptr，attached=true
        assert_eq!(apply_attach_current_thread(&mut st).unwrap(), 0xAA);
        assert!(st.is_attached());

        // 重复 attach 幂等
        apply_attach_current_thread(&mut st).unwrap();
        assert!(st.is_attached());

        // detach → attached=false
        apply_detach_current_thread(&mut st).unwrap();
        assert!(!st.is_attached());

        // detach 后 GetEnv 失败
        assert!(apply_get_env(&st, 0x0001_0006).is_err());

        // 再次 attach 恢复
        apply_attach_current_thread(&mut st).unwrap();
        assert!(apply_get_env(&st, 0x0001_0006).is_ok());
    }

    // —— JNI 返回码常量 ——

    #[test]
    fn jni_return_codes_match_spec() {
        assert_eq!(JNI_OK, 0);
        assert_eq!(JNI_ERR, -1i64 as u64);
        assert_eq!(JNI_EDETACHED, -2i64 as u64);
        assert_eq!(JNI_EVERSION, -3i64 as u64);
    }
}
