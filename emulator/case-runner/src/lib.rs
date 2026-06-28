//! `rundroid-case-runner`
//!
//! 把 backend / memory / elf 三层 / os / telemetry 串成一个完整 case 执行路径：
//! 1. 读取 `case.toml` → [`manifest::CaseManifest`]
//! 2. 解析 `resource:` URI → 本地字节流
//! 3. 装配 [`runtime::GuestRuntime`]（unicorn engine + region tracker + linux rt）
//! 4. parser → loader → linker
//! 5. 调用 entry 符号
//! 6. 把执行过程落盘为 `result.json` / `backend.json` / `events.jsonl`
//!
//! 该 crate 是"装配层"，自身不引入新的执行模型，
//! 所有具体行为都委托给上述子 crate。
//!
//! 本 crate 允许少量 `unsafe`：用于绕过 linker trait 与 graph 借用检查的"重叠 mut 借用"误报，
//! 所有 unsafe 都集中在 `runtime.rs` 的 LinkCtxAdapter 中，安全论证在代码注释里写明。

pub mod artifacts;
pub mod case;
pub mod manifest;
pub mod resource;
pub mod runtime;

// JNI trampoline hook + dispatch 是跨装配层共享的原语
// （case-runner 与 Python 绑定层共同消费），位于独立的
// `rundroid-jni-trampoline` crate。这里以 `jni_hook` 命名 re-export，
// 保持 case-runner 内部既有引用路径（`crate::jni_hook::JniTrampolineHook`）不变。
pub use rundroid_jni_trampoline as jni_hook;

pub use artifacts::{backend_info_from, Artifacts, BackendInfo, CallOutcome, CaseResult, Outcome, RegionEntry};
pub use case::{run_case, CaseRunError};
pub use manifest::{CaseCall, CaseManifest};
pub use resource::{resolve_resource, ResourceError};
pub use runtime::{EventRecord, GuestRuntime, RuntimeAssemblyError};
