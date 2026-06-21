//! TLS 模板提取。
//!
//! 真正的 TLS 还涉及 TCB / `TPIDR_EL0` / `__tls_get_addr` 协作，
//! bootstrap smoke 阶段只把 `.tdata`/`.tbss` 的位置暴露成 [`TlsTemplate`]，
/// 让 runtime 知道"这块静态 TLS 模板要复制到主线程 TLS 区"。

use crate::model::TlsTemplate;
use rundroid_elf_parser::{DynamicInfo, LoadSegment};

/// ARM64 TLS 程序头类型。
/// 数值来自 ELF spec：PT_TLS = 7。
pub const PT_TLS: u32 = 7;

/// 从镜像里提取 TLS 模板。
///
/// 当前 parser 没有把 PT_TLS 单独暴露（design 未要求），
/// 这里通过在 `segments` 里查找 type==PT_TLS 的方式兜底；
/// parser 将来补全 PT_TLS 字段后这里直接读字段即可。
pub fn extract_tls_template(
    segments: &[LoadSegment],
    _dynamic: &DynamicInfo,
) -> Option<TlsTemplate> {
    // 注意：parser 当前的 LoadSegment 只覆盖 PT_LOAD，不包含 PT_TLS。
    // bootstrap 阶段的 smoke case（导出函数调用 / urandom）不需要静态 TLS，
    // 因此这里返回 None 是预期行为；等 parser 暴露 PT_TLS 后再启用。
    let _ = segments;
    None
}
