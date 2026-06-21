//! AArch64 relocation 写回。
//!
//! bootstrap 最小集（design.md "Key Decisions 3"）：
//! - `R_AARCH64_RELATIVE`：*dst = load_bias + addend
//! - `R_AARCH64_GLOB_DAT` / `JUMP_SLOT`：*dst = symbol_addr + addend
//! - `R_AARCH64_ABS64`：*dst = symbol_addr + addend
//!
//! 所有写回都以"目标地址写一个 64 位值"为原子操作，
//! 因为 ARM64 这四类重定位都是 8 字节写。
//! 复杂的相对 / GOTPLT / TLS relocation 留给后续阶段。

use crate::error::ElfLinkError;
use rundroid_elf_parser::model::RelocationKind;
use rundroid_elf_loader::PendingRelocation;
use rundroid_memory::MemoryError;

/// 写回所需的"已解析符号地址"。
///
/// 对相对重定位（无 symbol_index）来说，这里直接用 None；
/// 其它类型则必须是 Some(符号的 guest 绝对地址)。
#[derive(Debug, Clone, Copy)]
pub struct RelocationPatch {
    pub target_addr: u64,
    pub value: u64,
}

/// 计算一条 relocation 的写回值。
///
/// `load_bias_of_owner`：拥有这条 relocation 的模块的 load_bias，
/// 用于 `R_AARCH64_RELATIVE` 计算 `base + addend`。
/// `resolved_symbol`：由 linker 在调用前查表得到；对 RELATIVE 来说为 None。
pub fn compute_patch(
    pending: &PendingRelocation,
    load_bias_of_owner: u64,
    resolved_symbol: Option<u64>,
) -> Result<RelocationPatch, ElfLinkError> {
    let value = match pending.kind {
        RelocationKind::Relative => {
            // RELATIVE 不查符号，直接 base + addend。
            // addend 是 i64，可能为负（罕见），转 u64 后加 base。
            load_bias_of_owner.wrapping_add(pending.addend as u64)
        }
        RelocationKind::GlobDat
        | RelocationKind::JumpSlot
        | RelocationKind::Abs64 => {
            let sym = resolved_symbol.ok_or(ElfLinkError::UnresolvedSymbol {
                name: format!("sym_idx={:?}", pending.symbol_index),
                module: pending.module_id,
            })?;
            sym.wrapping_add(pending.addend as u64)
        }
        RelocationKind::Other(_) => {
            return Err(ElfLinkError::UnsupportedRelocation(pending.kind));
        }
    };
    Ok(RelocationPatch {
        target_addr: pending.guest_addr,
        value,
    })
}

/// 把 8 字节 value 写到 guest 地址。
///
/// 这里通过 `write_u64` 闭包与 [`super::LinkContext`] 解耦，
/// 避免循环依赖（loader ← linker ← backend）。
pub fn apply_patch<F>(patch: RelocationPatch, write_u64: &mut F) -> Result<(), MemoryError>
where
    F: FnMut(u64, u64) -> Result<(), MemoryError>,
{
    write_u64(patch.target_addr, patch.value)
}
