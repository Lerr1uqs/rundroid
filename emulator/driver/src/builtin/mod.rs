//! rundroid 内建设备。
//!
//! 当前阶段提供：
//! - `/dev/urandom` / `/dev/random`：确定性伪随机设备
//! - `/dev/null`：写丢弃、读 EOF
//! - `/dev/zero`：读零字节、写丢弃
//!
//! 每个 builtin 以工厂函数形式导出，供 runtime 在启动时注册。

pub mod null;
pub mod urandom;
pub mod zero;

pub use null::null_factory;
pub use urandom::urandom_factory;
pub use zero::zero_factory;
