//! JNI 异常状态。
//!
//! [`ExceptionState`] 追踪当前线程上 pending 的 throwable 对象。
//! 遵循 JNI 规范：当有异常 pending 时，大多数 JNI 调用应被阻塞，
//! 直到异常被 `ExceptionClear()` 清除或被 `ExceptionDescribe()` 打印。
//!
//! # 当前阶段
//!
//! 仅覆盖 pending throwable 的最小语义：
//! - set / get / clear
//! - 异常发生时阻断后续 JNI 调用
//!
//! 不包含：
//! - 异常堆栈追踪
//! - 链式 cause 的完整建模
//! - `ThrowNew` 的 class constructor 调用

use crate::types::ObjectId;

// ============================================================================
// ExceptionState
// ============================================================================

/// 当前线程的异常状态。
///
/// 同一时刻最多有一个 pending 异常。
#[derive(Debug, Default)]
pub struct ExceptionState {
    /// 当前 pending 的异常信息。
    pending: Option<ExceptionRecord>,
}

/// 一条异常记录。
#[derive(Debug, Clone)]
pub struct ExceptionRecord {
    /// 异常对象的 ObjectId（在 `ObjectStore` 中注册）。
    pub object_id: ObjectId,
    /// 异常 class name（slash-separated，如 `"java/lang/NullPointerException"`）。
    pub class_name: String,
    /// 异常消息（可为空）。
    pub message: String,
    /// 引起此异常的 cause（可为 None）。
    pub cause: Option<ObjectId>,
}

impl ExceptionState {
    /// 创建空的异常状态（无 pending 异常）。
    pub fn new() -> Self {
        Self { pending: None }
    }

    /// 设置 pending 异常。
    ///
    /// 如果已有 pending 异常，新异常会覆盖旧异常（与 Android/ART 行为一致，
    /// ART 会丢弃旧异常并记录一条 warning）。
    pub fn set(&mut self, record: ExceptionRecord) {
        self.pending = Some(record);
    }

    /// 获取当前 pending 异常的不可变引用。
    pub fn pending(&self) -> Option<&ExceptionRecord> {
        self.pending.as_ref()
    }

    /// 获取当前 pending 异常的可变引用。
    pub fn pending_mut(&mut self) -> Option<&mut ExceptionRecord> {
        self.pending.as_mut()
    }

    /// 检查是否有 pending 异常。
    pub fn occurred(&self) -> bool {
        self.pending.is_some()
    }

    /// 清除当前 pending 异常。
    ///
    /// 如果当前无异常，此调用无害（no-op）。
    pub fn clear(&mut self) {
        self.pending = None;
    }

    /// 取出并清除当前 pending 异常。
    ///
    /// 返回 `Some(ExceptionRecord)` 如果有异常，否则 `None`。
    /// 用于"检查-清除"模式（如 native 方法中检查 `ExceptionCheck()`
    /// 然后 `ExceptionClear()`）。
    pub fn take(&mut self) -> Option<ExceptionRecord> {
        self.pending.take()
    }
}

// ============================================================================
// ExceptionRecord 辅助方法
// ============================================================================

impl ExceptionRecord {
    /// 创建新的异常记录。
    pub fn new(object_id: ObjectId, class_name: String, message: String) -> Self {
        Self {
            object_id,
            class_name,
            message,
            cause: None,
        }
    }

    /// 设置 cause。
    pub fn with_cause(mut self, cause: ObjectId) -> Self {
        self.cause = Some(cause);
        self
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_has_no_exception() {
        let state = ExceptionState::new();
        assert!(!state.occurred());
        assert!(state.pending().is_none());
    }

    #[test]
    fn set_and_check_exception() {
        let mut state = ExceptionState::new();
        let record = ExceptionRecord::new(
            ObjectId(1),
            "java/lang/NullPointerException".into(),
            "null pointer".into(),
        );

        state.set(record);
        assert!(state.occurred());
        assert_eq!(
            state.pending().unwrap().class_name,
            "java/lang/NullPointerException"
        );
    }

    #[test]
    fn clear_removes_exception() {
        let mut state = ExceptionState::new();
        state.set(ExceptionRecord::new(
            ObjectId(1),
            "java/lang/RuntimeException".into(),
            "error".into(),
        ));
        assert!(state.occurred());

        state.clear();
        assert!(!state.occurred());
        assert!(state.pending().is_none());
    }

    #[test]
    fn take_removes_and_returns() {
        let mut state = ExceptionState::new();
        state.set(ExceptionRecord::new(
            ObjectId(1),
            "java/lang/Exception".into(),
            "msg".into(),
        ));

        let taken = state.take().unwrap();
        assert_eq!(taken.class_name, "java/lang/Exception");
        assert!(!state.occurred());
        // take again returns None
        assert!(state.take().is_none());
    }

    #[test]
    fn set_overwrites_pending() {
        let mut state = ExceptionState::new();
        state.set(ExceptionRecord::new(
            ObjectId(1),
            "java/lang/Exception".into(),
            "first".into(),
        ));
        // 新异常覆盖旧异常
        state.set(ExceptionRecord::new(
            ObjectId(2),
            "java/lang/RuntimeException".into(),
            "second".into(),
        ));

        let pending = state.pending().unwrap();
        assert_eq!(pending.class_name, "java/lang/RuntimeException");
        assert_eq!(pending.object_id, ObjectId(2));
    }

    #[test]
    fn clear_on_empty_is_noop() {
        let mut state = ExceptionState::new();
        state.clear();
        assert!(!state.occurred());
    }

    #[test]
    fn exception_record_with_cause() {
        let cause_id = ObjectId(100);
        let record = ExceptionRecord::new(
            ObjectId(1),
            "java/lang/Exception".into(),
            "wrapped".into(),
        ).with_cause(cause_id);

        assert_eq!(record.cause, Some(ObjectId(100)));
        assert_eq!(record.message, "wrapped");
    }
}
