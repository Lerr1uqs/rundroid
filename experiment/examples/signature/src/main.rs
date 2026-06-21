//! 示例：用 proc-macro 注册 `android/content/pm/Signature`
//!
//! 对应 Python 版本：
//! ```python
//! @java_class("android/content/pm/Signature")
//! class Signature(JavaObject):
//!     def __init__(self):
//!         self._msig = bytes([])
//!
//!     @java_method("Signature([B)V")
//!     def signature_init(self, sig) -> JVoid:
//!         self._msig = sig
//!
//!     @java_method("hashCode()I")
//!     def hash_code(self) -> JInt:
//!         pass
//!
//!     @java_field(name="mSignature", sig="[B")
//!     def member_signature(self) -> JArray[JByte]:
//!         return self._msig
//! ```

use java_bind::JavaObject;
use java_bind_macros::{java_class, java_impl};

// ============================================================================
// 1. 定义 struct + @java_class 映射
// ============================================================================

#[java_class(name = "android/content/pm/Signature")]
#[derive(Debug)]
struct Signature {
    _msig: Vec<u8>,
}

// ============================================================================
// 2. 实现方法 + @java_impl 自动注册
// ============================================================================

#[java_impl(class = "android/content/pm/Signature")]
impl Signature {
    /// 构造函数 — 对应 Java `Signature([B)V`（返回 Self，自动识别为 ctor）
    #[java_method = "Signature([B)V"]
    fn new(sig: Vec<u8>) -> Self {
        Self { _msig: sig }
    }

    /// 实例方法 — 对应 Java `hashCode()I`
    #[java_method = "hashCode()I"]
    fn hash_code(&self) -> i32 {
        // 简单 hash：用 _msig 长度（实际 unidbg 用 CRC32）
        self._msig.len() as i32
    }

    /// Field getter — 对应 Java field `mSignature:[B`
    #[java_field(name = "mSignature", sig = "[B")]
    fn member_signature(&self) -> Vec<u8> {
        self._msig.clone()
    }

    /// 额外的 Rust 方法（无 java 注解 → 不注册到 JNI）
    fn is_empty(&self) -> bool {
        self._msig.is_empty()
    }
}

// ============================================================================
// 3. 第二个类：用于展示多 class 注册
// ============================================================================

#[java_class(name = "java/util/ArrayList")]
struct ArrayList {
    items: Vec<String>,
}

#[java_impl(class = "java/util/ArrayList")]
impl ArrayList {
    #[java_method = "ArrayList()V"]
    fn new() -> Self {
        Self { items: Vec::new() }
    }

    #[java_method = "size()I"]
    fn size(&self) -> i32 {
        self.items.len() as i32
    }

    #[java_method = "add(Ljava/lang/Object;)Z"]
    fn add(&mut self, item: String) -> bool {
        self.items.push(item);
        true
    }
}

// ============================================================================
// main — 验证注册效果
// ============================================================================

fn main() {
    println!("=== 验证 java-bind proc-macro 自动注册 ===\n");

    // 触发注册：调用 reflect() 初始化 type_map + REGISTRY
    let sig_meta = Signature::reflect();
    let list_meta = ArrayList::reflect();

    println!(
        "✓ Signature::reflect() → class = {}",
        sig_meta.java_class
    );
    println!(
        "✓ ArrayList::reflect()  → class = {}",
        list_meta.java_class
    );

    // 打印完整注册表
    java_bind::dump_registry();

    // 按 class name 查询
    println!("\n=== 按 class name 查询 ===");
    let meta = java_bind::lookup_class_meta("android/content/pm/Signature");
    println!("  methods: {:?}", meta.methods);
    println!("  fields:  {:?}", meta.fields);

    // type_map 反查
    println!("\n=== type_map 反查 ===");
    let jn = java_bind::java_class_of_type(std::any::type_name::<Signature>());
    println!(
        "  type_name::<Signature>() → {:?}",
        jn
    );

    // 列表所有已注册 class
    println!("\n=== 所有已注册 class ===");
    for name in java_bind::list_classes() {
        println!("  - {name}");
    }

    // 实例化验证
    let sig = Signature::new(vec![0xDE, 0xAD, 0xBE, 0xEF]);
    println!("\n=== 实例化验证 ===");
    println!("  Signature::new([DE AD BE EF])");
    println!("  signature.hash_code() = {:#x}", sig.hash_code());
    println!(
        "  signature.member_signature() = {:?}",
        sig.member_signature()
    );
    println!("  signature.is_empty() = {}", sig.is_empty());
}
