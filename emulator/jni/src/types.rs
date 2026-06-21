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
