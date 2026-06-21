//! runtime 启动配置。
//!
//! 所有跨子系统的开关集中在这里。子系统不允许各自读取环境变量或全局状态，
//! 必须从 [`RuntimeConfig`] 取值——这样 case 之间、replay 与原始运行之间
//! 才能保证行为一致。

use crate::arch::Arch;
use crate::backend::BackendKind;
use rundroid_telemetry::TelemetryMode;

/// runtime 的总配置。
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// guest 架构。bootstrap 阶段固定为 [`Arch::Arm64`]。
    pub arch: Arch,
    /// 选用的 backend。
    pub backend: BackendKind,
    /// telemetry 开关档位。
    pub telemetry: TelemetryMode,
    /// guest 内存布局参数。
    pub memory: MemoryConfig,
}

impl RuntimeConfig {
    /// bootstrap 默认配置：ARM64 + Unicorn + telemetry 关闭 + 常规内存布局。
    ///
    /// 让"零参数启动"也能拿到一份合理配置，case 清单只需要覆盖差异字段。
    pub fn bootstrap() -> Self {
        Self {
            arch: Arch::Arm64,
            backend: BackendKind::Unicorn,
            telemetry: TelemetryMode::Disabled,
            memory: MemoryConfig::bootstrap(),
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::bootstrap()
    }
}

/// guest 内存布局参数。
///
/// 这些值影响 stack / TLS 初始放置，单独成结构方便 case 覆盖而不污染主配置。
#[derive(Debug, Clone, Copy)]
pub struct MemoryConfig {
    /// guest 栈大小（字节）。ARM64 ABI 要求 16 字节对齐，
    /// 因此这里以及上层分配都会向上对齐到 16。
    pub stack_size: u64,
    /// TLS 模板区初始预留（字节）。
    pub tls_size: u64,
    /// guest 虚拟地址空间顶端。stack 通常贴在顶端向下生长。
    pub address_space_top: u64,
}

impl MemoryConfig {
    /// bootstrap 默认布局：64KiB 栈、4KiB TLS、48 位地址空间顶。
    pub fn bootstrap() -> Self {
        Self {
            stack_size: 64 * 1024,
            tls_size: 4 * 1024,
            // 0xFFFF_FFFF_F000_0000 是 Android 用户态常见的 mmap 高端基线，
            // 留出足够低位空间给 ELF 镜像。
            address_space_top: 0xFFFF_FFFF_F000_0000,
        }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self::bootstrap()
    }
}
