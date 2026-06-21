//! Native 方法注册表 — RegisterNatives 绑定 + Java_* fallback 查找。
//!
//! [`NativeRegistry`] 存储通过 `RegisterNatives` 显式注册的 guest native 函数绑定。
//! key 为 [`MethodId`](crate::types::MethodId)，value 为 guest 函数地址。
//!
//! 同时提供 [`mangle_java_method`] 将 class name + method name 转换为
//! `Java_*` mangled C 符号名，用于未显式注册时的 fallback 查找。
//!
//! # 规则
//!
//! - `RegisterNatives` 绑定优先于 `Java_*` dynamic lookup
//! - 同一 MethodId 重复注册时覆盖（后注册者优先）

use crate::types::MethodId;
use std::collections::HashMap;

/// Guest 函数指针（绝对地址）。
pub type GuestPtr = u64;

// ============================================================================
// NativeRegistry
// ============================================================================

/// Native 方法注册表。
///
/// 存储通过 `RegisterNatives` 显式注册的 native 方法绑定。
/// key 为 `MethodId`，value 为 guest 函数地址。
///
/// # 与 `JniRegistry` 的关系
///
/// `JniRegistry` 存储所有 class/method/field 的元数据和 Rust/Python 实现，
/// `NativeRegistry` 仅存储 guest native 函数地址的覆盖绑定——
/// 两者互补：registry 里的 `MethodImpl` 是 Rust/Python 侧实现，
/// native registry 将此 MethodId 重新指向 guest 地址。
#[derive(Debug, Default)]
pub struct NativeRegistry {
    /// 已注册的 native 方法：MethodId → guest 函数地址。
    pub by_method: HashMap<MethodId, GuestPtr>,
}

impl NativeRegistry {
    /// 创建空的 native registry。
    pub fn new() -> Self {
        Self {
            by_method: HashMap::new(),
        }
    }

    /// 注册一个 native 方法绑定。
    ///
    /// 如果同一个 `MethodId` 已被注册，则覆盖（后注册者优先）。
    pub fn register(&mut self, method_id: MethodId, fn_ptr: GuestPtr) {
        self.by_method.insert(method_id, fn_ptr);
    }

    /// 查找已注册的 native 方法。
    ///
    /// 返回 guest 函数地址（如果已注册）。
    pub fn lookup(&self, method_id: MethodId) -> Option<GuestPtr> {
        self.by_method.get(&method_id).copied()
    }

    /// 检查是否有已注册的 native 方法。
    pub fn is_empty(&self) -> bool {
        self.by_method.is_empty()
    }

    /// 已注册的 native 方法数量。
    pub fn len(&self) -> usize {
        self.by_method.len()
    }
}

// ============================================================================
// Java_* name mangling
// ============================================================================

/// 将 class name（slash-separated）和 method name 转换为 `Java_*` C 符号名。
///
/// JNI 规范定义的原生方法名称 mangling 规则：
/// - 前缀 `Java_`
/// - 完整限定类名：`/` 替换为 `_`，原有的 `_` 转义为 `_1`
/// - 方法名：原有的 `_` 转义为 `_1`
///
/// # 示例
///
/// ```
/// // class="com/example/MyClass", method="nativeMethod"
/// // → "Java_com_example_MyClass_nativeMethod"
/// ```
///
/// # 注意
///
/// - 此函数生成**无重载**的符号名（不带 `__` 后缀）
/// - 重载方法通过 `mangle_java_method_overloaded` 生成带签名后缀的变体
pub fn mangle_java_method(class_name: &str, method_name: &str) -> String {
    let mut result = String::from("Java_");
    // JNI 规范：先对原始 class name 中的 '_' 做 escape（_ → _1），
    // 再把 '/' 替换为 '_'。顺序不可颠倒，否则 '/' 替换产生的 '_' 也会被 escape。
    mangle_class_to_cname(class_name, &mut result);
    result.push('_');
    mangle_identifier(method_name, &mut result);
    result
}

/// 将 class name + method name + JNI descriptor 转换为重载符号名。
///
/// 与 [`mangle_java_method`] 不同，此函数在方法名后追加 `__` + mangled arg types，
/// 用于区分重载方法。
///
/// arg types 从 descriptor 的 `(...)` 部分提取，不包含返回类型。
/// 然后按 JNI 规范转义特殊字符：
/// - `_` → `_1`
/// - `;` → `_2`
/// - `[` → `_3`
/// - `/` → `_`
///
/// # 示例
///
/// ```
/// // class="com/example/MyClass", method="foo", sig="(II)I"
/// // → "Java_com_example_MyClass_foo__II"
/// ```
pub fn mangle_java_method_overloaded(class_name: &str, method_name: &str, sig_descriptor: &str) -> String {
    let base = mangle_java_method(class_name, method_name);
    let mut result = base;
    result.push_str("__");
    // 提取 descriptor 中 `(...)` 部分的参数类型
    let arg_types = extract_arg_types(sig_descriptor);
    mangle_signature_for_cname(&arg_types, &mut result);
    result
}

/// 从 JNI descriptor 中提取参数类型部分（`(...)` 之间）。
///
/// 例如 `"(II)I"` → `"II"`，`"(Ljava/lang/String;I)V"` → `"Ljava/lang/String;I"`。
fn extract_arg_types(descriptor: &str) -> &str {
    if let Some(start) = descriptor.find('(') {
        if let Some(end) = descriptor.rfind(')') {
            if start + 1 < end {
                return &descriptor[start + 1..end];
            }
        }
    }
    "" // 无参数或解析失败
}

/// 对标识符片段进行 JNI escape（`_` → `_1`）。
fn mangle_identifier(ident: &str, out: &mut String) {
    for ch in ident.chars() {
        match ch {
            '_' => out.push_str("_1"),
            _ => out.push(ch),
        }
    }
}

/// 对 class name 做 JNI C 符号名转义：先 escape `_` → `_1`，再 `/` → `_`。
///
/// 顺序不可颠倒：如果先把 `/` 替换为 `_` 再 escape，
/// 则 class name 中本不存在的 `_` 也会被 escape，产生错误输出。
fn mangle_class_to_cname(class_name: &str, out: &mut String) {
    for ch in class_name.chars() {
        match ch {
            '_' => out.push_str("_1"),
            '/' => out.push('_'),
            _ => out.push(ch),
        }
    }
}

/// 对 JNI descriptor 中的特殊字符进行 C 符号名 escape。
///
/// 按 JNI 规范对 `_`, `;`, `[`, `/` 进行转义。
fn mangle_signature_for_cname(sig: &str, out: &mut String) {
    for ch in sig.chars() {
        match ch {
            '_' => out.push_str("_1"),
            ';' => out.push_str("_2"),
            '[' => out.push_str("_3"),
            '/' => out.push('_'),
            _ => out.push(ch),
        }
    }
}

/// 给定一个符号名，检查它是否为 `Java_*` 格式的 native 方法符号。
///
/// 如果是，返回 `Some((class_name, method_name))`，
/// 其中 class_name 为 slash-separated 格式，
/// method_name 为 unmangled 的方法名。
///
/// 如果不是 `Java_` 前缀或解析失败，返回 `None`。
pub fn unmangle_java_symbol(symbol: &str) -> Option<(String, String)> {
    let rest = symbol.strip_prefix("Java_")?;
    if rest.is_empty() {
        return None;
    }

    // 分离 class 部分和 method 部分：最后一个 `_` 后跟非数字字符的位置
    // 需要正确处理 `_1` escape。
    // 简化策略：从左到右解析，遇到 `_1` 当作 escape 跳过，
    // 找到最后一个未被 escape 的 `_` 作为 class/method 分隔符。
    let chars: Vec<char> = rest.chars().collect();
    let mut last_sep = None;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '_' {
            if i + 1 < chars.len() && chars[i + 1] == '1' {
                // _1 escape：这属于一个标识符内部字符，跳过后面的 1
                i += 2;
                continue;
            }
            // 非 escape 的 _：潜在的 class/method 分隔符
            last_sep = Some(i);
        }
        i += 1;
    }

    let sep = last_sep?;
    let class_part: String = chars[..sep].iter().collect();
    let method_part: String = chars[sep + 1..].iter().collect();

    // unmangle class: _1 → _, _ → /
    let class_name = unmangle_class_identifier(&class_part);
    let method_name = unmangle_simple_identifier(&method_part);

    Some((class_name, method_name))
}

/// Unmangle class 标识符：`_1` → `_`, `_` → `/`。
fn unmangle_class_identifier(mangled: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = mangled.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '_' {
            if i + 1 < chars.len() && chars[i + 1] == '1' {
                result.push('_');
                i += 2;
                continue;
            }
            // 独立的 _ → /
            result.push('/');
        } else {
            result.push(chars[i]);
        }
        i += 1;
    }
    result
}

/// Unmangle 方法名：`_1` → `_`。
fn unmangle_simple_identifier(mangled: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = mangled.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '_' && i + 1 < chars.len() && chars[i + 1] == '1' {
            result.push('_');
            i += 2;
            continue;
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

// ============================================================================
// JNI version 常量
// ============================================================================

/// JNI version 常量（`JNI_OnLoad` 返回值校验用）。
pub const JNI_VERSION_1_1: u64 = 0x0001_0001;
pub const JNI_VERSION_1_2: u64 = 0x0001_0002;
pub const JNI_VERSION_1_4: u64 = 0x0001_0004;
pub const JNI_VERSION_1_6: u64 = 0x0001_0006;
pub const JNI_VERSION_1_8: u64 = 0x0001_0008;

/// 当前支持的 JNI version 列表（降序）。
pub const SUPPORTED_JNI_VERSIONS: &[u64] = &[
    JNI_VERSION_1_8,
    JNI_VERSION_1_6,
    JNI_VERSION_1_4,
    JNI_VERSION_1_2,
    JNI_VERSION_1_1,
];

/// 校验 `JNI_OnLoad` 返回的 version 是否合法。
///
/// 返回 `true` 表示版本号在支持列表中。
pub fn validate_jni_version(version: u64) -> bool {
    SUPPORTED_JNI_VERSIONS.contains(&version)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MethodId;

    // —— NativeRegistry 测试 ——

    #[test]
    fn native_registry_empty() {
        let reg = NativeRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert_eq!(reg.lookup(MethodId(1)), None);
    }

    #[test]
    fn native_registry_register_and_lookup() {
        let mut reg = NativeRegistry::new();
        reg.register(MethodId(42), 0x4000_1000);
        assert!(!reg.is_empty());
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.lookup(MethodId(42)), Some(0x4000_1000));
        assert_eq!(reg.lookup(MethodId(99)), None);
    }

    #[test]
    fn native_registry_overwrite() {
        let mut reg = NativeRegistry::new();
        reg.register(MethodId(1), 0x1000);
        reg.register(MethodId(1), 0x2000); // 覆盖
        assert_eq!(reg.lookup(MethodId(1)), Some(0x2000));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn native_registry_multiple_methods() {
        let mut reg = NativeRegistry::new();
        reg.register(MethodId(1), 0x1000);
        reg.register(MethodId(2), 0x2000);
        reg.register(MethodId(3), 0x3000);
        assert_eq!(reg.len(), 3);
        assert_eq!(reg.lookup(MethodId(1)), Some(0x1000));
        assert_eq!(reg.lookup(MethodId(2)), Some(0x2000));
        assert_eq!(reg.lookup(MethodId(3)), Some(0x3000));
    }

    // —— Java_* mangling 测试 ——

    #[test]
    fn mangle_simple_class_method() {
        // com.example.MyClass.nativeMethod → Java_com_example_MyClass_nativeMethod
        let result = mangle_java_method("com/example/MyClass", "nativeMethod");
        assert_eq!(result, "Java_com_example_MyClass_nativeMethod");
    }

    #[test]
    fn mangle_with_underscores() {
        // 类名中的 _ 被转义为 _1
        let result = mangle_java_method("com/example/My_Class", "my_method");
        assert_eq!(result, "Java_com_example_My_1Class_my_1method");
    }

    #[test]
    fn mangle_android_class() {
        // android.app.NativeActivity.onStart
        let result = mangle_java_method("android/app/NativeActivity", "onStart");
        assert_eq!(result, "Java_android_app_NativeActivity_onStart");
    }

    #[test]
    fn mangle_overloaded() {
        // 重载方法带签名后缀
        let result = mangle_java_method_overloaded("com/example/MyClass", "foo", "(II)I");
        assert_eq!(result, "Java_com_example_MyClass_foo__II");
    }

    #[test]
    fn mangle_overloaded_with_object_args() {
        // 签名中的特殊字符被转义
        let result = mangle_java_method_overloaded(
            "com/example/Test",
            "process",
            "(Ljava/lang/String;I)V",
        );
        assert_eq!(result, "Java_com_example_Test_process__Ljava_lang_String_2I");
    }

    #[test]
    fn mangle_overloaded_with_array() {
        // descriptor "([B)[B": arg types = "[B"（只有参数部分）
        let result = mangle_java_method_overloaded(
            "com/example/Test",
            "getArray",
            "([B)[B",
        );
        assert_eq!(result, "Java_com_example_Test_getArray___3B");
    }

    // —— unmangle 测试 ——

    #[test]
    fn unmangle_simple() {
        let (class, method) = unmangle_java_symbol("Java_com_example_MyClass_nativeMethod").unwrap();
        assert_eq!(class, "com/example/MyClass");
        assert_eq!(method, "nativeMethod");
    }

    #[test]
    fn unmangle_with_underscore_escape() {
        let (class, method) = unmangle_java_symbol("Java_com_example_My_1Class_my_1method").unwrap();
        assert_eq!(class, "com/example/My_Class");
        assert_eq!(method, "my_method");
    }

    #[test]
    fn unmangle_android_class() {
        let (class, method) = unmangle_java_symbol("Java_android_app_NativeActivity_onStart").unwrap();
        assert_eq!(class, "android/app/NativeActivity");
        assert_eq!(method, "onStart");
    }

    #[test]
    fn unmangle_not_java_prefix() {
        assert!(unmangle_java_symbol("my_custom_function").is_none());
    }

    #[test]
    fn unmangle_empty_after_prefix() {
        assert!(unmangle_java_symbol("Java_").is_none());
    }

    // —— JNI version 校验测试 ——

    #[test]
    fn valid_jni_versions() {
        assert!(validate_jni_version(JNI_VERSION_1_1));
        assert!(validate_jni_version(JNI_VERSION_1_2));
        assert!(validate_jni_version(JNI_VERSION_1_4));
        assert!(validate_jni_version(JNI_VERSION_1_6));
        assert!(validate_jni_version(JNI_VERSION_1_8));
    }

    #[test]
    fn invalid_jni_versions() {
        assert!(!validate_jni_version(0x0000_0000));
        assert!(!validate_jni_version(0x0001_0007));
        assert!(!validate_jni_version(0xFFFF_0000));
        assert!(!validate_jni_version(1));
    }

    #[test]
    fn jni_version_constants_match_unidbg() {
        // 确保 version 常量与 Android / unidbg 一致
        assert_eq!(JNI_VERSION_1_6, 0x0001_0006);
        assert_eq!(JNI_VERSION_1_4, 0x0001_0004);
    }
}
