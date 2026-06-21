//! JNI 函数指针表 — guest-accessible JNI function table 布局管理。
//!
//! # 架构
//!
//! Guest 代码通过 `(*env)->FindClass(env, name)` 调用 JNI 函数，
//! 这需要：
//! 1. 一个 guest 内存中的 `JNIEnv` 结构体（首字段指向函数指针表）
//! 2. 一个函数指针表（232 个槽位，每个指向 trampoline 地址）
//! 3. 一个 trampoline 页（每条目 4 字节 NOP，CodeHook 在指令执行前拦截）
//!
//! 内存布局：
//! - `JNI_ENV_BASE` (e.g. 0x7F_C000_0000): 首 8 字节 = pointer to `JNI_TABLE_BASE`
//! - `JNI_TABLE_BASE` (e.g. 0x7F_C000_1000): 232 entries × 8 bytes（pointer to trampoline[i]）
//! - `JNI_TRAMPOLINE_BASE` (e.g. 0x7F_C000_2000): 232 entries × 4 bytes（ARM64 NOP）
//!
//! 函数索引与 unidbg DalvikVM64 的 `impl.setPointer(offset, ...)` 完全对齐。
//!
//! # TODO: trampoline 只是临时缓解措施
//!
//! 当前 JNIEnv / 函数表 / trampoline 三块区域通过固定的高端地址硬编码映射，
//! 调用方的 `init_jni()` 直接用 `engine.mem_map` 抢占地址。
//! **后续应改为通过 guest 自身的 mmap/allocator 分配这些区域**，
//! 避免与 guest .so 的装载地址冲突，也避免与真实 Android 进程的地址空间布局差异过大。

// ============================================================================
// JNI 函数表索引常量（与 JNI 规范 / unidbg 一致）
// ============================================================================

/// 保留槽位（前 4 个函数指针为 NULL）。
pub const JNI_RESERVED_COUNT: usize = 4;

// —— Class / Method ——
pub const JNI_GET_VERSION: usize = 4;
pub const JNI_FIND_CLASS: usize = 6;
pub const JNI_GET_METHOD_ID: usize = 33;
pub const JNI_GET_STATIC_METHOD_ID: usize = 113;
pub const JNI_NEW_OBJECT: usize = 28;
pub const JNI_NEW_OBJECT_V: usize = 29;
pub const JNI_ALLOC_OBJECT: usize = 27;
pub const JNI_GET_OBJECT_CLASS: usize = 31;
pub const JNI_REGISTER_NATIVES: usize = 215;
pub const JNI_GET_JAVA_VM: usize = 219;

// —— Exception ——
pub const JNI_EXCEPTION_OCCURRED: usize = 15;
pub const JNI_EXCEPTION_CLEAR: usize = 17;

// —— Reference ——
pub const JNI_NEW_GLOBAL_REF: usize = 21;
pub const JNI_DELETE_GLOBAL_REF: usize = 22;
pub const JNI_DELETE_LOCAL_REF: usize = 23;
pub const JNI_NEW_LOCAL_REF: usize = 25;

// —— CallXxxMethod (instance) ——
pub const JNI_CALL_OBJECT_METHOD: usize = 34;
pub const JNI_CALL_BOOLEAN_METHOD: usize = 37;
pub const JNI_CALL_BYTE_METHOD: usize = 40;
pub const JNI_CALL_CHAR_METHOD: usize = 43;
pub const JNI_CALL_SHORT_METHOD: usize = 46;
pub const JNI_CALL_INT_METHOD: usize = 49;
pub const JNI_CALL_LONG_METHOD: usize = 52;
pub const JNI_CALL_FLOAT_METHOD: usize = 55;
pub const JNI_CALL_DOUBLE_METHOD: usize = 58;
pub const JNI_CALL_VOID_METHOD: usize = 61;

// —— CallStaticXxxMethod ——
pub const JNI_CALL_STATIC_OBJECT_METHOD: usize = 198;
pub const JNI_CALL_STATIC_BOOLEAN_METHOD: usize = 201;
pub const JNI_CALL_STATIC_BYTE_METHOD: usize = 204;
pub const JNI_CALL_STATIC_CHAR_METHOD: usize = 207;
pub const JNI_CALL_STATIC_SHORT_METHOD: usize = 210;
pub const JNI_CALL_STATIC_INT_METHOD: usize = 213;
pub const JNI_CALL_STATIC_LONG_METHOD: usize = 216;
pub const JNI_CALL_STATIC_FLOAT_METHOD: usize = 219;
pub const JNI_CALL_STATIC_DOUBLE_METHOD: usize = 222;
pub const JNI_CALL_STATIC_VOID_METHOD: usize = 225;

// —— Field ——
pub const JNI_GET_FIELD_ID: usize = 94;
pub const JNI_GET_STATIC_FIELD_ID: usize = 187;
pub const JNI_GET_OBJECT_FIELD: usize = 95;
pub const JNI_GET_BOOLEAN_FIELD: usize = 96;
pub const JNI_GET_BYTE_FIELD: usize = 97;
pub const JNI_GET_CHAR_FIELD: usize = 98;
pub const JNI_GET_SHORT_FIELD: usize = 99;
pub const JNI_GET_INT_FIELD: usize = 100;
pub const JNI_GET_LONG_FIELD: usize = 101;
pub const JNI_GET_FLOAT_FIELD: usize = 102;
pub const JNI_GET_DOUBLE_FIELD: usize = 103;
pub const JNI_SET_INT_FIELD: usize = 105;
pub const JNI_SET_OBJECT_FIELD: usize = 108;

// —— String ——
pub const JNI_NEW_STRING_UTF: usize = 167;
pub const JNI_GET_STRING_UTF_CHARS: usize = 169;

/// 函数表总条目数（Android N+ 为 232 个槽位）。
pub const JNI_TABLE_SIZE: usize = 232;

/// 每条目 trampoline 的大小（4 字节 NOP）。
pub const TRAMPOLINE_SLOT_SIZE: u64 = 4;

/// ARM64 NOP 指令的小端编码（0xD503201F）。
pub const ARM64_NOP: [u8; 4] = [0x1F, 0x20, 0x03, 0xD5];

// ============================================================================
// JniFunctionTable — guest 内存布局管理
// ============================================================================

/// 管理 guest 内存中 JNI 函数指针表和 trampoline 页的布局。
///
/// 在 guest 地址空间分配三块区域：
/// - JNIEnv 结构体（指向函数表）
/// - 函数指针表（232 个 8 字节指针）
/// - trampoline 页（232 个 4 字节 NOP）
#[derive(Debug, Clone)]
pub struct JniFunctionTable {
    /// JNIEnv 结构体在 guest 内存中的地址。
    pub env_ptr: u64,
    /// 函数指针表在 guest 内存中的地址。
    pub table_base: u64,
    /// trampoline 页在 guest 内存中的地址。
    pub trampoline_base: u64,
    /// 映射总大小（字节）。
    pub total_size: usize,
}

impl JniFunctionTable {
    /// 计算各区域地址（不实际映射，仅布局）。
    pub fn layout(base_addr: u64) -> Self {
        const PAGE: u64 = 0x1000;
        let env_size = PAGE as usize;
        let table_raw_size = (JNI_TABLE_SIZE * 8) as u64;
        let table_size = align_up(table_raw_size, PAGE) as usize;
        let table_base = base_addr + env_size as u64;

        let tramp_raw_size = (JNI_TABLE_SIZE as u64) * TRAMPOLINE_SLOT_SIZE;
        let tramp_size = align_up(tramp_raw_size, PAGE) as usize;
        let trampoline_base = table_base + table_size as u64;

        let total_size = env_size + table_size + tramp_size;

        Self {
            env_ptr: base_addr,
            table_base,
            trampoline_base,
            total_size,
        }
    }

    /// 从 trampoline 地址反算 JNI 函数索引。
    ///
    /// # Panics
    /// 如果 `address` 不在 trampoline 页范围内，直接 panic（let-it-failed）。
    pub fn function_index(&self, address: u64) -> usize {
        let offset = address.saturating_sub(self.trampoline_base);
        assert!(
            offset < (JNI_TABLE_SIZE as u64) * TRAMPOLINE_SLOT_SIZE,
            "JNI trampoline address 0x{address:X} out of range (base=0x{:X})",
            self.trampoline_base
        );
        (offset / TRAMPOLINE_SLOT_SIZE) as usize
    }

    /// 在 guest 内存中写入函数指针表和 trampoline 页。
    ///
    /// `mem_write` 是 guest 内存写入闭包（由 backend 或 GuestCPU 提供）。
    pub fn write_table_to_guest(
        &self,
        mem_write: &mut dyn FnMut(u64, &[u8]) -> bool,
    ) -> Result<(), String> {
        // 1. 写 JNIEnv header: 首 8 字节 = 指向 table_base 的指针
        let env_header = self.table_base.to_le_bytes();
        if !mem_write(self.env_ptr, &env_header) {
            return Err(format!("写入 JNIEnv header 失败 @ 0x{:X}", self.env_ptr));
        }

        // 2. 写函数指针表
        for i in 0..JNI_TABLE_SIZE {
            let tramp_addr = self.trampoline_base + (i as u64) * TRAMPOLINE_SLOT_SIZE;
            let table_slot = self.table_base + (i as u64) * 8;
            let ptr_bytes = tramp_addr.to_le_bytes();
            mem_write(table_slot, &ptr_bytes);
        }

        // 3. 写 trampoline 页：全部填充 NOP
        for i in 0..JNI_TABLE_SIZE {
            let tramp_addr = self.trampoline_base + (i as u64) * TRAMPOLINE_SLOT_SIZE;
            mem_write(tramp_addr, &ARM64_NOP);
        }

        Ok(())
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 向上对齐到 `align` 的倍数。
pub fn align_up(v: u64, align: u64) -> u64 {
    if align == 0 {
        v
    } else {
        (v + align - 1) & !(align - 1)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_addresses() {
        let base = 0x7F_C000_0000u64;
        let ft = JniFunctionTable::layout(base);

        assert_eq!(ft.env_ptr, base);
        assert_eq!(ft.table_base % 0x1000, 0);
        assert_eq!(ft.trampoline_base % 0x1000, 0);
        assert!(ft.table_base > ft.env_ptr);
        assert!(ft.trampoline_base > ft.table_base);
        assert!(ft.total_size > 0);
    }

    #[test]
    fn test_function_index_calculation() {
        let base = 0x7F_C000_0000u64;
        let ft = JniFunctionTable::layout(base);

        assert_eq!(ft.function_index(ft.trampoline_base), 0);
        assert_eq!(ft.function_index(ft.trampoline_base + 4), 1);
        assert_eq!(ft.function_index(ft.trampoline_base + 6 * 4), 6);
        assert_eq!(ft.function_index(ft.trampoline_base + 33 * 4), 33);
        assert_eq!(ft.function_index(ft.trampoline_base + 231 * 4), 231);
    }

    #[test]
    #[should_panic]
    fn test_function_index_out_of_range_panics() {
        let base = 0x7F_C000_0000u64;
        let ft = JniFunctionTable::layout(base);
        ft.function_index(ft.trampoline_base + 232 * 4);
    }

    #[test]
    fn test_constants_match_unidbg() {
        assert_eq!(JNI_FIND_CLASS, 6);
        assert_eq!(JNI_GET_METHOD_ID, 33);
        assert_eq!(JNI_GET_STATIC_METHOD_ID, 113);
        assert_eq!(JNI_NEW_OBJECT, 28);
        assert_eq!(JNI_CALL_VOID_METHOD, 61);
        assert_eq!(JNI_CALL_INT_METHOD, 49);
        assert_eq!(JNI_CALL_STATIC_VOID_METHOD, 225);
        assert_eq!(JNI_NEW_GLOBAL_REF, 21);
        assert_eq!(JNI_GET_FIELD_ID, 94);
    }
}
