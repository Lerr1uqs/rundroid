//! `rundroid-memory`
//!
//! guest 地址空间抽象。bootstrap 阶段提供：
//! - [`region::RegionTracker`]：跟踪已映射区间，检测重叠
//! - [`layout`]：stack / TLS 等固定布局的地址计算
//! - [`error::MemoryError`]：内存层错误模型
//!
//! 本 crate 故意不依赖 backend，只做"布局规划与记账"，
//! 实际的字节写入 / 权限切换由 backend（Unicorn）执行，
//! loader 在中间协调两者。这样单元测试可以在没有 emulator 的情况下验证布局。

#![forbid(unsafe_code)]

pub mod error;
pub mod layout;
pub mod region;

pub use error::MemoryError;
pub use layout::{StackLayout, TlsLayout};
pub use region::{MemoryRegion, RegionOrigin, RegionTracker};
