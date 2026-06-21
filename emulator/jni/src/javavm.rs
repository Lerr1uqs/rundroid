//! 最小 `JavaVM` surface。
//!
//! foundation 阶段仅提供 `attach_current_thread` / `detach_current_thread`
//! 和获取当前线程 env 的能力。
//!
//! `JavaVMSurface` 持有 registry 和 ref table 的所有权，
//! 通过 `attach_current_thread` 产出 `JniEnvSurface`。

use crate::error::JniError;
use crate::jnienv::JniEnvSurface;
use crate::refs::RefTable;
use crate::registry::JniRegistry;

/// 最小 JavaVM surface。
///
/// foundation 阶段不区分多线程——只有一个"当前线程"。
/// 后续扩展线程模型时，改为维护 `ThreadId -> RefTable` 映射即可。
pub struct JavaVMSurface {
    registry: JniRegistry,
    refs: RefTable,
    attached: bool,
}

impl JavaVMSurface {
    /// 创建新的 JavaVM（持有空 registry 和空 ref table）。
    pub fn new() -> Self {
        Self {
            registry: JniRegistry::new(),
            refs: RefTable::new(),
            attached: false,
        }
    }

    /// 从已有的 registry 和 ref table 构造 JavaVM。
    ///
    /// 用于 emulator 装配层已经把 JNI 子系统组装好后一次性传入的场景。
    pub fn from_parts(registry: JniRegistry, refs: RefTable) -> Self {
        Self {
            registry,
            refs,
            attached: false,
        }
    }

    /// 获取 registry 的不可变引用。
    pub fn registry(&self) -> &JniRegistry {
        &self.registry
    }

    /// 获取 registry 的可变引用。
    pub fn registry_mut(&mut self) -> &mut JniRegistry {
        &mut self.registry
    }

    /// 将当前线程 attach 到 JavaVM。
    ///
    /// 返回该线程绑定的 `JniEnvSurface`。
    ///
    /// foundation 阶段仅支持单线程：重复 attach 返回错误。
    pub fn attach_current_thread(&mut self) -> Result<JniEnvSurface<'_>, JniError> {
        if self.attached {
            return Err(JniError::Internal("当前线程已 attach".to_string()));
        }
        self.attached = true;
        Ok(JniEnvSurface::new(&self.registry, &mut self.refs))
    }

    /// 将当前线程从 JavaVM detach。
    ///
    /// 清理当前帧的 local refs。
    pub fn detach_current_thread(&mut self) {
        self.refs.clear_frame();
        self.attached = false;
    }

    /// 检查当前线程是否已 attach。
    pub fn is_attached(&self) -> bool {
        self.attached
    }

    /// 获取当前线程的 JNIEnv（如果已 attach）。
    pub fn env(&mut self) -> Option<JniEnvSurface<'_>> {
        if self.attached {
            Some(JniEnvSurface::new(&self.registry, &mut self.refs))
        } else {
            None
        }
    }
}

impl Default for JavaVMSurface {
    fn default() -> Self {
        Self::new()
    }
}
