//! JNI foundation canonical 类型模型。
//!
//! 定义所有 JNI 路径上使用的稳定类型：primitive 类型、值承载、
//! method signature 和 field signature。
//!
//! # 规则
//!
//! - `Null` 只允许出现在 object / array 兼容位置
//! - primitive 返回值不允许以 `Null` 代替
//! - runtime 不做 silent widening / narrowing
//! - 内部 class name 使用 slash-separated 形式（如 `java/lang/Object`）

use std::fmt;

// ============================================================================
// ObjectId
// ============================================================================

/// JNI 对象在 Rust 侧的唯一标识。
///
/// guest 可见的是 handle（ref table 分配的 u32），
/// `ObjectId` 是 Rust 内部用于追踪对象生命周期的标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ObjectId(pub u64);

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "obj#{}", self.0)
    }
}

// ============================================================================
// ClassId / MethodId / FieldId — typed ID 体系
// ============================================================================

/// JNI class 在 Rust 侧的唯一标识。
///
/// `ClassId` 对应一个完整的 class definition，
/// 是 method / field 的聚合根。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ClassId(pub u64);

impl fmt::Display for ClassId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "class#{}", self.0)
    }
}

/// JNI method 在 Rust 侧的唯一标识。
///
/// `MethodId` 归属于某个 `ClassId`，是 class-local ID。
/// 不作为与 class 并列的全局顶层权威。
/// 如果需要在全局范围引用一个 method，使用 `(ClassId, MethodId)` 二元组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MethodId(pub u64);

impl fmt::Display for MethodId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "method#{}", self.0)
    }
}

/// JNI field 在 Rust 侧的唯一标识。
///
/// `FieldId` 归属于某个 `ClassId`，是 class-local ID。
/// 不作为与 class 并列的全局顶层权威。
/// 如果需要在全局范围引用一个 field，使用 `(ClassId, FieldId)` 二元组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FieldId(pub u64);

impl fmt::Display for FieldId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "field#{}", self.0)
    }
}

/// Typed ID 分配器。
///
/// 为 `ClassId` 和 `ObjectId` 分配全局唯一 ID。
///
/// # 注意：`MethodId` / `FieldId` 不是全局分配
///
/// `MethodId` 和 `FieldId` 是 class-local ID，
/// 由 `JClassDef::add_method()` / `add_field()` 在 class 内部按插入顺序编号。
/// 这是有意为之——method / field 不作为与 class 并列的顶层权威，
/// 其 ID 归属 class 作用域，不需要全局唯一。
///
/// 如果后续需要全局 method/field 索引（如 telemetry 追踪），
/// 可以用 `(ClassId, MethodId)` 二元组作为全局 key。
#[derive(Debug, Default)]
pub struct IdAllocator {
    next: u64,
}

impl IdAllocator {
    /// 创建新的 ID 分配器（从 1 开始）。
    pub fn new() -> Self {
        Self { next: 1 }
    }

    /// 分配一个全局唯一的 ClassId。
    pub fn class(&mut self) -> ClassId {
        let id = ClassId(self.next);
        self.next += 1;
        id
    }

    /// 分配一个全局唯一的 ObjectId。
    pub fn object(&mut self) -> ObjectId {
        let id = ObjectId(self.next);
        self.next += 1;
        id
    }
}

// ============================================================================
// JType
// ============================================================================

/// JNI 类型标签。
///
/// 用于 method signature 的参数和返回值类型描述。
/// 注意：这不是运行时值，是类型层面的元数据。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JType {
    Void,
    Boolean,
    Byte,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
    /// 对象类型，携带 slash-separated class name（如 `"java/lang/String"`）。
    Object(String),
    /// 数组类型，携带元素类型。
    Array(Box<JType>),
}

impl JType {
    /// 判断此类型是否兼容 Null 值。
    ///
    /// 只有 Object 和 Array 类型可以接受 Null，
    /// primitive 和 Void 位置不允许 Null。
    pub fn nullable(&self) -> bool {
        matches!(self, JType::Object(_) | JType::Array(_))
    }

    /// 返回 JNI descriptor 中此类型的单字符表示（primitive 时）。
    pub fn primitive_char(&self) -> Option<char> {
        match self {
            JType::Void => Some('V'),
            JType::Boolean => Some('Z'),
            JType::Byte => Some('B'),
            JType::Char => Some('C'),
            JType::Short => Some('S'),
            JType::Int => Some('I'),
            JType::Long => Some('J'),
            JType::Float => Some('F'),
            JType::Double => Some('D'),
            JType::Object(_) | JType::Array(_) => None,
        }
    }
}

impl fmt::Display for JType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JType::Void => write!(f, "void"),
            JType::Boolean => write!(f, "boolean"),
            JType::Byte => write!(f, "byte"),
            JType::Char => write!(f, "char"),
            JType::Short => write!(f, "short"),
            JType::Int => write!(f, "int"),
            JType::Long => write!(f, "long"),
            JType::Float => write!(f, "float"),
            JType::Double => write!(f, "double"),
            JType::Object(name) => write!(f, "L{};", name),
            JType::Array(elem) => write!(f, "[{}", elem),
        }
    }
}

// ============================================================================
// JValue
// ============================================================================

/// JNI 运行时值。
///
/// 这是 method 参数和返回值的实际承载类型。
/// Primitive 值按其自然宽度存储，不做隐式转换。
#[derive(Debug, Clone, PartialEq)]
pub enum JValue {
    Void,
    Boolean(bool),
    Byte(i8),
    Char(u16),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    /// 对象引用，携带 ObjectId。
    Object(ObjectId),
    /// 空引用。只能用于 Object / Array 类型位置。
    Null,
}

impl JValue {
    /// 返回此值对应的 JType。
    pub fn jtype(&self) -> JType {
        match self {
            JValue::Void => JType::Void,
            JValue::Boolean(_) => JType::Boolean,
            JValue::Byte(_) => JType::Byte,
            JValue::Char(_) => JType::Char,
            JValue::Short(_) => JType::Short,
            JValue::Int(_) => JType::Int,
            JValue::Long(_) => JType::Long,
            JValue::Float(_) => JType::Float,
            JValue::Double(_) => JType::Double,
            JValue::Object(_) => JType::Object(String::new()),
            JValue::Null => JType::Object(String::new()),
        }
    }

    /// 安全取出 Boolean 值。类型不匹配时返回 None。
    pub fn as_boolean(&self) -> Option<bool> {
        match self { JValue::Boolean(v) => Some(*v), _ => None }
    }

    /// 安全取出 Int 值。
    pub fn as_int(&self) -> Option<i32> {
        match self { JValue::Int(v) => Some(*v), _ => None }
    }

    /// 安全取出 Long 值。
    pub fn as_long(&self) -> Option<i64> {
        match self { JValue::Long(v) => Some(*v), _ => None }
    }

    /// 安全取出 Object 引用。
    pub fn as_object(&self) -> Option<ObjectId> {
        match self { JValue::Object(id) => Some(*id), _ => None }
    }

    /// 是否为 Null。
    pub fn is_null(&self) -> bool {
        matches!(self, JValue::Null)
    }
}

impl fmt::Display for JValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JValue::Void => write!(f, "void"),
            JValue::Boolean(v) => write!(f, "{v}"),
            JValue::Byte(v) => write!(f, "{v}b"),
            JValue::Char(v) => write!(f, "'{v}'"),
            JValue::Short(v) => write!(f, "{v}s"),
            JValue::Int(v) => write!(f, "{v}"),
            JValue::Long(v) => write!(f, "{v}L"),
            JValue::Float(v) => write!(f, "{v}f"),
            JValue::Double(v) => write!(f, "{v}"),
            JValue::Object(id) => write!(f, "{id}"),
            JValue::Null => write!(f, "null"),
        }
    }
}

// ============================================================================
// MethodSig / FieldSig
// ============================================================================

/// 解析后的 method signature。
///
/// Registry 以此作为 method 的 canonical key，
/// 避免用原始 descriptor 字符串做查找和比较。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MethodSig {
    /// slash-separated class name（如 `"android/content/pm/Signature"`）。
    pub class: String,
    /// method 名称（如 `"hashCode"`）。
    pub name: String,
    /// 参数类型列表。
    pub args: Vec<JType>,
    /// 返回值类型。
    pub ret: JType,
}

impl fmt::Display for MethodSig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}(", self.class, self.name)?;
        for (i, arg) in self.args.iter().enumerate() {
            if i > 0 { write!(f, ",")?; }
            write!(f, "{arg}")?;
        }
        write!(f, "){}", self.ret)
    }
}

/// 解析后的 field signature。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FieldSig {
    /// slash-separated class name。
    pub class: String,
    /// field 名称。
    pub name: String,
    /// field 类型。
    pub ty: JType,
}

impl fmt::Display for FieldSig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}:{}", self.class, self.name, self.ty)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // —— Typed ID 测试 ——

    #[test]
    fn typed_ids_distinct_types() {
        // ClassId / ObjectId / MethodId / FieldId 是不同的类型，不能互相赋值
        let c = ClassId(1);
        let o = ObjectId(1);
        let m = MethodId(1);
        let f = FieldId(1);

        // 同数值但不同类型，各自独立
        assert_eq!(c.0, 1);
        assert_eq!(o.0, 1);
        assert_eq!(m.0, 1);
        assert_eq!(f.0, 1);
    }

    #[test]
    fn id_allocator_sequential() {
        let mut alloc = IdAllocator::new();

        let c1 = alloc.class();
        let c2 = alloc.class();
        assert_eq!(c1.0, 1);
        assert_eq!(c2.0, 2);

        let o1 = alloc.object();
        assert_eq!(o1.0, 3);
        assert_eq!(c1.0, 1); // ClassId 不受后续分配影响
    }

    #[test]
    fn id_display_format() {
        assert_eq!(format!("{}", ClassId(42)), "class#42");
        assert_eq!(format!("{}", ObjectId(42)), "obj#42");
        assert_eq!(format!("{}", MethodId(42)), "method#42");
        assert_eq!(format!("{}", FieldId(42)), "field#42");
    }

    #[test]
    fn typed_ids_eq_and_hash() {
        let a = ClassId(1);
        let b = ClassId(1);
        let c = ClassId(2);

        assert_eq!(a, b);
        assert_ne!(a, c);

        // 可用作 HashMap key
        let mut map: std::collections::HashMap<ClassId, &str> = std::collections::HashMap::new();
        map.insert(a, "hello");
        assert_eq!(map.get(&b), Some(&"hello"));
    }

}
