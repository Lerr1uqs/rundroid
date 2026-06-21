//! JNI 引用表。
//!
//! 管理 handle（u32）到 ObjectId 的映射，区分 local / global / weak 生命周期。
//!
//! # 生命周期规则
//!
//! - **Local refs**：在 call frame 结束后统一清理（通过 `clear_frame()`）
//! - **Global refs**：不受局部调用结束影响，需显式 `delete_global()`
//! - **Weak global refs**：当前阶段仅标记 kind，弱引用回收策略延后实现
//!
//! guest 只看到 handle（u32 整数），不直接持有对象引用。

use crate::error::JniError;
use crate::types::ObjectId;

/// 引用种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// 局部引用——当前 call frame 结束后回收。
    Local,
    /// 全局引用——持久的对象引用。
    Global,
    /// 弱全局引用——不阻止对象回收的全局引用（暂未实现回收）。
    WeakGlobal,
}

/// 引用表中的一项。
#[derive(Debug, Clone)]
struct RefEntry {
    /// handle 值（guest 可见）。
    id: u32,
    /// 指向的对象 ID。
    object_id: ObjectId,
    /// 引用种类。
    kind: RefKind,
}

/// JNI 引用表。
///
/// 所有 new / delete / clear_frame 操作都在此结构上执行。
#[derive(Debug, Default)]
pub struct RefTable {
    entries: Vec<RefEntry>,
    next_id: u32,
    /// 当前 frame 中创建的 local ref 的 handle 列表，用于 `clear_frame()` 批量清理。
    local_frame: Vec<u32>,
}

impl RefTable {
    /// 创建空引用表（handle 从 1 开始计数，0 为无效 handle）。
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 1,
            local_frame: Vec::new(),
        }
    }

    /// 创建新的 local reference。
    ///
    /// 返回 guest 可见的 handle（u32）。
    /// local ref 会在下次 `clear_frame()` 时被清除。
    pub fn new_local(&mut self, object_id: ObjectId) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(RefEntry { id, object_id, kind: RefKind::Local });
        self.local_frame.push(id);
        id
    }

    /// 创建新的 global reference。
    ///
    /// global ref 不受 `clear_frame()` 影响，
    /// 需要显式调用 `delete_global()` 删除。
    pub fn new_global(&mut self, object_id: ObjectId) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(RefEntry { id, object_id, kind: RefKind::Global });
        id
    }

    /// 创建新的 weak global reference。
    pub fn new_weak(&mut self, object_id: ObjectId) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(RefEntry { id, object_id, kind: RefKind::WeakGlobal });
        id
    }

    /// 删除 local reference。
    ///
    /// 如果 handle 存在且为 local ref，则移除并返回 true。
    pub fn delete_local(&mut self, handle: u32) -> Result<(), JniError> {
        let pos = self.entries.iter().position(|e| e.id == handle)
            .ok_or_else(|| JniError::Internal(format!("ref handle 不存在: {handle}")))?;
        if self.entries[pos].kind != RefKind::Local {
            return Err(JniError::Internal(format!("handle {handle} 不是 local ref")));
        }
        self.entries.remove(pos);
        self.local_frame.retain(|&h| h != handle);
        Ok(())
    }

    /// 删除 global reference。
    pub fn delete_global(&mut self, handle: u32) -> Result<(), JniError> {
        let pos = self.entries.iter().position(|e| e.id == handle)
            .ok_or_else(|| JniError::Internal(format!("ref handle 不存在: {handle}")))?;
        if self.entries[pos].kind != RefKind::Global {
            return Err(JniError::Internal(format!("handle {handle} 不是 global ref")));
        }
        self.entries.remove(pos);
        Ok(())
    }

    /// 清理当前 frame 的所有 local refs。
    ///
    /// 每次 JNI method 调用返回后应调用此方法。
    pub fn clear_frame(&mut self) {
        let local_ids: Vec<u32> = self.local_frame.drain(..).collect();
        self.entries.retain(|e| !local_ids.contains(&e.id));
    }

    /// 通过 handle 解析 ObjectId。
    ///
    /// 如果 handle 存在且有效则返回 ObjectId，否则返回 None。
    pub fn resolve(&self, handle: u32) -> Option<ObjectId> {
        self.entries.iter().find(|e| e.id == handle).map(|e| e.object_id)
    }

    /// 获取 handle 的引用种类。
    pub fn kind(&self, handle: u32) -> Option<RefKind> {
        self.entries.iter().find(|e| e.id == handle).map(|e| e.kind)
    }

    /// 当前引用表的总条目数（用于测试/调试）。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 引用表是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ref_lifecycle() {
        let mut table = RefTable::new();
        let obj_id = ObjectId(100);
        let handle = table.new_local(obj_id);
        assert_eq!(table.len(), 1);
        assert_eq!(table.resolve(handle), Some(obj_id));
        assert_eq!(table.kind(handle), Some(RefKind::Local));

        table.clear_frame();
        assert_eq!(table.len(), 0);
        assert_eq!(table.resolve(handle), None);
    }

    #[test]
    fn global_ref_survives_clear() {
        let mut table = RefTable::new();
        let obj_id = ObjectId(200);
        let handle = table.new_global(obj_id);
        assert_eq!(table.kind(handle), Some(RefKind::Global));

        table.clear_frame();
        assert_eq!(table.len(), 1);
        assert_eq!(table.resolve(handle), Some(obj_id));
    }

    #[test]
    fn delete_global() {
        let mut table = RefTable::new();
        let handle = table.new_global(ObjectId(300));
        table.delete_global(handle).unwrap();
        assert_eq!(table.resolve(handle), None);
    }

    #[test]
    fn delete_local_wrong_kind_fails() {
        let mut table = RefTable::new();
        let handle = table.new_global(ObjectId(400));
        assert!(table.delete_local(handle).is_err());
    }
}
