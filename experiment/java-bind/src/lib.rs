//! `java-bind` — Rust 侧 JNI class 自动注册 runtime
//!
//! 配合 `java-bind-macros` proc-macro 使用：
//! - `#[java_class(name = "...")]` 标注 struct → 自动实现 `JavaObject` trait
//! - `#[java_impl(class = "...")]` 标注 impl block → 自动收集 method/field 元数据
//!
//! 所有注册通过 `inventory` 在链接期自动收集，
//! 首次调用 `lookup_class_meta()` 时构建完整的 `ClassMeta`。

pub use inventory;

use std::collections::HashMap;
use std::sync::Mutex;

// ============================================================================
// JNI 类型标签
// ============================================================================

/// JNI 类型描述符
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JniType {
    Void,
    Boolean,
    Byte,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
    /// Object 类型，携带 slash-separated class name
    Object(String),
    /// 数组类型，携带元素类型
    Array(Box<JniType>),
}

impl JniType {
    /// 返回 JNI descriptor 字符串
    pub fn sig(&self) -> String {
        match self {
            JniType::Void => "V".into(),
            JniType::Boolean => "Z".into(),
            JniType::Byte => "B".into(),
            JniType::Char => "C".into(),
            JniType::Short => "S".into(),
            JniType::Int => "I".into(),
            JniType::Long => "J".into(),
            JniType::Float => "F".into(),
            JniType::Double => "D".into(),
            JniType::Object(name) => format!("L{};", name),
            JniType::Array(elem) => format!("[{}", elem.sig()),
        }
    }
}

// ============================================================================
// 方法注册条目（通过 inventory 自动收集）
// ============================================================================

/// 方法种类
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MethodKind {
    /// 普通实例方法
    Instance,
    /// 静态方法
    Static,
    /// 构造函数
    Constructor,
}

/// 编译期注册的方法条目（由 #[java_impl] 生成 inventory::submit!）
#[derive(Debug, Clone)]
pub struct MethodReg {
    /// Java class 全限定名（slash-separated，如 "android/content/pm/Signature"）
    pub class_name: &'static str,
    /// Java 方法名（如 "hashCode", "Signature"）
    pub java_name: &'static str,
    /// 完整 JNI method descriptor（如 "hashCode()I"）
    pub jni_sig: &'static str,
    /// 对应的 Rust 方法名
    pub rust_fn_name: &'static str,
    /// 方法种类
    pub kind: MethodKind,
}

impl PartialEq for MethodReg {
    fn eq(&self, other: &Self) -> bool {
        self.class_name == other.class_name
            && self.jni_sig == other.jni_sig
            && self.kind == other.kind
    }
}
impl Eq for MethodReg {}

impl PartialOrd for MethodReg {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for MethodReg {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.class_name
            .cmp(other.class_name)
            .then_with(|| self.jni_sig.cmp(other.jni_sig))
            .then_with(|| self.kind.cmp(&other.kind))
    }
}

// ============================================================================
// 字段注册条目
// ============================================================================

/// 编译期注册的字段条目（由 #[java_impl] 生成 inventory::submit!）
#[derive(Debug, Clone)]
pub struct FieldReg {
    /// Java class 全限定名
    pub class_name: &'static str,
    /// Java 字段名
    pub field_name: &'static str,
    /// JNI field type descriptor（如 "[B"）
    pub jni_sig: &'static str,
    /// 对应的 Rust getter 方法名
    pub rust_fn_name: &'static str,
}

impl PartialEq for FieldReg {
    fn eq(&self, other: &Self) -> bool {
        self.class_name == other.class_name && self.field_name == other.field_name
    }
}
impl Eq for FieldReg {}

impl PartialOrd for FieldReg {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for FieldReg {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.class_name
            .cmp(other.class_name)
            .then_with(|| self.field_name.cmp(other.field_name))
    }
}

// ============================================================================
// inventory 收集点
// ============================================================================

inventory::collect!(MethodReg);
inventory::collect!(FieldReg);

// ============================================================================
// 运行时元数据
// ============================================================================

/// 方法描述（运行时）
#[derive(Debug, Clone)]
pub struct MethodDesc {
    pub name: String,
    pub jni_sig: String,
    pub is_constructor: bool,
    pub is_static: bool,
    pub rust_fn_name: String,
}

/// 字段描述（运行时）
#[derive(Debug, Clone)]
pub struct FieldDesc {
    pub name: String,
    pub jni_sig: String,
    pub rust_fn_name: String,
}

/// 类元数据（运行时，由 inventory 条目构建）
#[derive(Debug, Clone)]
pub struct ClassMeta {
    pub java_class: String,
    pub methods: Vec<MethodDesc>,
    pub fields: Vec<FieldDesc>,
}

// ============================================================================
// 全局注册表
// ============================================================================

/// 全局 JNI class 注册表（按 java class name 索引）
static REGISTRY: Mutex<Option<HashMap<String, ClassMeta>>> = Mutex::new(None);

/// Rust type_name → java class name 映射（由 #[java_class] 在首次 reflect 时注册）
static TYPE_MAP: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// 初始化注册表：遍历所有 inventory 条目并按 class_name 分组
fn init_registry() {
    let mut guard = REGISTRY.lock().unwrap();
    if guard.is_some() {
        return;
    }

    let mut map: HashMap<String, ClassMeta> = HashMap::new();

    for method in inventory::iter::<MethodReg> {
        let entry = map
            .entry(method.class_name.to_string())
            .or_insert_with(|| ClassMeta {
                java_class: method.class_name.to_string(),
                methods: Vec::new(),
                fields: Vec::new(),
            });
        entry.methods.push(MethodDesc {
            name: method.java_name.to_string(),
            jni_sig: method.jni_sig.to_string(),
            is_constructor: matches!(method.kind, MethodKind::Constructor),
            is_static: matches!(method.kind, MethodKind::Static),
            rust_fn_name: method.rust_fn_name.to_string(),
        });
    }

    for field in inventory::iter::<FieldReg> {
        let entry = map
            .entry(field.class_name.to_string())
            .or_insert_with(|| ClassMeta {
                java_class: field.class_name.to_string(),
                methods: Vec::new(),
                fields: Vec::new(),
            });
        entry.fields.push(FieldDesc {
            name: field.field_name.to_string(),
            jni_sig: field.jni_sig.to_string(),
            rust_fn_name: field.rust_fn_name.to_string(),
        });
    }

    *guard = Some(map);
}

/// 注册 type_map 条目（由 #[java_class] 的 JavaObject::reflect() 调用）
///
/// 一般在 `JavaObject::reflect()` 首次调用时自动触发，无需手动调用。
pub fn register_type_map(rust_type_name: &str, java_class_name: &str) {
    let mut guard = TYPE_MAP.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(rust_type_name.to_string(), java_class_name.to_string());
}

// ============================================================================
// JavaObject trait
// ============================================================================

/// 可被 Java 绑定的 Rust struct。
///
/// 由 `#[java_class]` proc-macro 自动实现。
pub trait JavaObject: 'static {
    /// 返回该 class 的完整元数据（延迟初始化）
    fn class_meta() -> &'static ClassMeta
    where
        Self: Sized;

    /// 返回 Java class 名
    fn java_class_name() -> String
    where
        Self: Sized;

    /// 触发注册（确保 class 在 REGISTRY 中有条目）
    fn reflect() -> &'static ClassMeta
    where
        Self: Sized,
    {
        init_registry();
        let java_name = Self::java_class_name();

        // 注册 type_map（Rust type → Java class）
        register_type_map(std::any::type_name::<Self>(), &java_name);

        // 返回 class_meta（会从 REGISTRY 查找）
        Self::class_meta()
    }
}

// ============================================================================
// 查询 API
// ============================================================================

/// 根据 java class name 查找 ClassMeta
pub fn lookup_class_meta(class_name: &str) -> &'static ClassMeta {
    init_registry();
    let guard = REGISTRY.lock().unwrap();
    let map = guard.as_ref().expect("registry 未初始化");
    if let Some(meta) = map.get(class_name) {
        // 安全：ClassMeta 存储在全局 Mutex 中永不释放
        // 这里用 Box::leak 把副本转成 'static
        Box::leak(Box::new(meta.clone()))
    } else {
        panic!(
            "class `{class_name}` 未注册！已注册的 class: {:?}",
            map.keys().collect::<Vec<_>>()
        );
    }
}

/// 获取指定 class 的元数据（可选）
pub fn get_class_meta(class_name: &str) -> Option<ClassMeta> {
    init_registry();
    REGISTRY
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|map| map.get(class_name).cloned())
}

/// 列出所有已注册的 class 名
pub fn list_classes() -> Vec<String> {
    init_registry();
    REGISTRY
        .lock()
        .unwrap()
        .as_ref()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

/// 打印注册表（调试用）
pub fn dump_registry() {
    init_registry();
    let guard = REGISTRY.lock().unwrap();
    let map = guard.as_ref().unwrap();
    println!("=== JNI Registry ({} classes) ===", map.len());
    for (name, meta) in map {
        println!("  class: {name}");
        for m in &meta.methods {
            let kind = if m.is_constructor {
                "ctor"
            } else if m.is_static {
                "static"
            } else {
                "method"
            };
            println!("    [{kind}] {} → rust fn `{}`", m.jni_sig, m.rust_fn_name);
        }
        for f in &meta.fields {
            println!(
                "    [field] {}:{} → rust fn `{}`",
                f.name, f.jni_sig, f.rust_fn_name
            );
        }
    }
}

// ============================================================================
// 类型转换工具（运行时反射调用辅助）
// ============================================================================

/// 通过 type_map 查找 Rust 类型对应的 Java class name
pub fn java_class_of_type(rust_type_name: &str) -> Option<String> {
    let guard = TYPE_MAP.lock().unwrap();
    guard
        .as_ref()
        .and_then(|map| map.get(rust_type_name).cloned())
}
