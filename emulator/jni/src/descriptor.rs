//! JNI descriptor 解析器。
//!
//! 这是 JNI foundation 的入口闸门——所有注册操作都先把 descriptor
//! 解析为 canonical `MethodSig` / `FieldSig`，后续 dispatch / verify
//! 不再重新解析原始字符串。
//!
//! # 输入格式
//!
//! - method descriptor: `"className.methodName(args)returnType"`
//!   例如: `"android/content/pm/Signature.hashCode()I"`
//!         `"java/lang/Object.equals(Ljava/lang/Object;)Z"`
//! - field descriptor: `"className.fieldName:fieldType"`
//!   例如: `"java/lang/System.out:Ljava/io/PrintStream;"`
//!
//! # 规则
//!
//! - 内部 class name 使用 slash-separated 形式
//! - 非法 descriptor 在解析阶段直接失败
//! - 不支持宽松容错，严格遵循 JNI descriptor 语法

use crate::error::JniError;
use crate::types::{FieldSig, JType, MethodSig};

impl MethodSig {
    /// 解析 method descriptor 为 canonical `MethodSig`。
    ///
    /// # 格式
    /// `"className.methodName(argType1argType2...)returnType"`
    ///
    /// class name 和 method name 由 `.` 分隔，
    /// 参数列表由 `(` 和 `)` 包围，
    /// 返回值类型在 `)` 之后。
    ///
    /// # 示例
    /// - `"android/os/Bundle.getInt(Ljava/lang/String;)I"` → MethodSig { class: "android/os/Bundle",
    ///   name: "getInt", args: [Object("java/lang/String"), Int], ret: Int }
    /// - `"hashCode()I"` → MethodSig { class: "", name: "hashCode", args: [], ret: Int }
    ///   （仅含 method 名，class 由外层 decorator 提供）
    pub fn parse(raw: &str) -> Result<MethodSig, JniError> {
        // 查找 `.` 分隔 class name 和 method 部分。
        let dot_pos = raw.find('.');

        let (class, method_part) = if let Some(pos) = dot_pos {
            let before_dot = &raw[..pos];
            // `.` 前出现 `(` 或 `)` 说明 descriptor 不合法
            if before_dot.contains('(') || before_dot.contains(')') {
                return Err(JniError::InvalidDescriptor(raw.to_string()));
            }
            (before_dot.to_string(), &raw[pos + 1..])
        } else {
            (String::new(), raw)
        };

        // 查找 `(` 分隔 method name 和参数列表
        let paren_open = method_part.find('(')
            .ok_or_else(|| JniError::InvalidDescriptor(raw.to_string()))?;

        let name = method_part[..paren_open].to_string();
        if name.is_empty() {
            return Err(JniError::InvalidDescriptor(raw.to_string()));
        }

        // 查找 `)` 结束参数列表
        let after_open = &method_part[paren_open + 1..];
        let paren_close = after_open.find(')')
            .ok_or_else(|| JniError::InvalidDescriptor(raw.to_string()))?;

        let args_str = &after_open[..paren_close];
        let ret_str = &after_open[paren_close + 1..];

        // 解析参数类型和返回值类型
        let args = parse_type_list(args_str)
            .map_err(|_| JniError::InvalidDescriptor(raw.to_string()))?;
        let (ret, _) = parse_type(ret_str)
            .map_err(|_| JniError::InvalidDescriptor(raw.to_string()))?;

        Ok(MethodSig { class, name, args, ret })
    }
}

impl FieldSig {
    /// 解析 field descriptor 为 canonical `FieldSig`。
    ///
    /// # 格式
    /// - `"className.fieldName:fieldType"` — 完整格式
    /// - `"fieldName:fieldType"` — 仅含 field name，class 由外层提供
    ///
    /// # 示例
    /// - `"java/lang/System.out:Ljava/io/PrintStream;"` → FieldSig { class: "java/lang/System",
    ///   name: "out", ty: Object("java/io/PrintStream") }
    /// - `"count:I"` → FieldSig { class: "", name: "count", ty: Int }
    pub fn parse(raw: &str) -> Result<FieldSig, JniError> {
        let colon_pos = raw.rfind(':')
            .ok_or_else(|| JniError::InvalidDescriptor(raw.to_string()))?;

        let field_part = &raw[..colon_pos];
        let ty_str = &raw[colon_pos + 1..];

        // 查找 `.` 分隔 class name 和 field name
        let (class, name) = if let Some(dot_pos) = field_part.rfind('.') {
            let class = field_part[..dot_pos].to_string();
            let name = field_part[dot_pos + 1..].to_string();
            if class.is_empty() || name.is_empty() {
                return Err(JniError::InvalidDescriptor(raw.to_string()));
            }
            (class, name)
        } else {
            // 没有 `.`：只有 field name，class name 由外层 decorator 提供
            let name = field_part.to_string();
            if name.is_empty() {
                return Err(JniError::InvalidDescriptor(raw.to_string()));
            }
            (String::new(), name)
        };

        let (ty, remainder) = parse_type(ty_str)
            .map_err(|_| JniError::InvalidDescriptor(raw.to_string()))?;
        if !remainder.is_empty() {
            return Err(JniError::InvalidDescriptor(raw.to_string()));
        }

        Ok(FieldSig { class, name, ty })
    }
}

/// 解析用 JNI descriptor 语法编写的类型。
///
/// 返回解析出的 `JType` 和剩余的未解析字符串。
///
/// # JNI type descriptor 语法
/// - `V` = void, `Z` = boolean, `B` = byte, `C` = char
/// - `S` = short, `I` = int, `J` = long, `F` = float, `D` = double
/// - `Lfull/class/name;` = object
/// - `[type` = array
fn parse_type(s: &str) -> Result<(JType, &str), JniError> {
    if s.is_empty() {
        return Err(JniError::InvalidDescriptor("空类型字符串".to_string()));
    }

    match s.chars().next().unwrap() {
        'V' => Ok((JType::Void, &s[1..])),
        'Z' => Ok((JType::Boolean, &s[1..])),
        'B' => Ok((JType::Byte, &s[1..])),
        'C' => Ok((JType::Char, &s[1..])),
        'S' => Ok((JType::Short, &s[1..])),
        'I' => Ok((JType::Int, &s[1..])),
        'J' => Ok((JType::Long, &s[1..])),
        'F' => Ok((JType::Float, &s[1..])),
        'D' => Ok((JType::Double, &s[1..])),
        'L' => {
            let semi = s.find(';')
                .ok_or_else(|| JniError::InvalidDescriptor(format!("对象类型缺少 ';': `{s}`")))?;
            let class_name = s[1..semi].to_string();
            if class_name.is_empty() {
                return Err(JniError::InvalidDescriptor("对象类型缺少 class name".to_string()));
            }
            Ok((JType::Object(class_name), &s[semi + 1..]))
        }
        '[' => {
            let (elem_type, remainder) = parse_type(&s[1..])?;
            Ok((JType::Array(Box::new(elem_type)), remainder))
        }
        c => Err(JniError::InvalidDescriptor(format!("未知类型字符: '{c}' (0x{:02X})", c as u32))),
    }
}

/// 解析参数类型列表（在 `()` 之间）。
fn parse_type_list(s: &str) -> Result<Vec<JType>, JniError> {
    let mut types = Vec::new();
    let mut remaining = s;
    while !remaining.is_empty() {
        let (ty, rest) = parse_type(remaining)?;
        types.push(ty);
        remaining = rest;
    }
    Ok(types)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_method() {
        let sig = MethodSig::parse("android/os/Bundle.hashCode()I").unwrap();
        assert_eq!(sig.class, "android/os/Bundle");
        assert_eq!(sig.name, "hashCode");
        assert!(sig.args.is_empty());
        assert_eq!(sig.ret, JType::Int);
    }

    #[test]
    fn parse_method_no_class() {
        let sig = MethodSig::parse("hashCode()I").unwrap();
        assert_eq!(sig.class, "");
        assert_eq!(sig.name, "hashCode");
        assert!(sig.args.is_empty());
        assert_eq!(sig.ret, JType::Int);
    }

    #[test]
    fn parse_method_with_object_arg() {
        let sig = MethodSig::parse("java/lang/Object.equals(Ljava/lang/Object;)Z").unwrap();
        assert_eq!(sig.class, "java/lang/Object");
        assert_eq!(sig.name, "equals");
        assert_eq!(sig.args.len(), 1);
        assert_eq!(sig.args[0], JType::Object("java/lang/Object".into()));
        assert_eq!(sig.ret, JType::Boolean);
    }

    #[test]
    fn parse_method_with_multiple_args() {
        let sig = MethodSig::parse("MyClass.foo(ILjava/lang/String;Z)V").unwrap();
        assert_eq!(sig.args.len(), 3);
        assert_eq!(sig.args[0], JType::Int);
        assert_eq!(sig.args[1], JType::Object("java/lang/String".into()));
        assert_eq!(sig.args[2], JType::Boolean);
        assert_eq!(sig.ret, JType::Void);
    }

    #[test]
    fn parse_field() {
        let sig = FieldSig::parse("java/lang/System.out:Ljava/io/PrintStream;").unwrap();
        assert_eq!(sig.class, "java/lang/System");
        assert_eq!(sig.name, "out");
        assert_eq!(sig.ty, JType::Object("java/io/PrintStream".into()));
    }

    #[test]
    fn parse_field_int() {
        let sig = FieldSig::parse("MyClass.count:I").unwrap();
        assert_eq!(sig.name, "count");
        assert_eq!(sig.ty, JType::Int);
    }

    #[test]
    fn parse_invalid_descriptor_fails() {
        assert!(MethodSig::parse("not a valid descriptor").is_err());
        assert!(MethodSig::parse("").is_err());
        assert!(FieldSig::parse("no_colon").is_err());
    }

    #[test]
    fn parse_type_primitives() {
        assert_eq!(parse_type("I").unwrap(), (JType::Int, ""));
        assert_eq!(parse_type("J").unwrap(), (JType::Long, ""));
        assert_eq!(parse_type("Z").unwrap(), (JType::Boolean, ""));
    }

    #[test]
    fn parse_type_array() {
        let (ty, rest) = parse_type("[I").unwrap();
        assert_eq!(ty, JType::Array(Box::new(JType::Int)));
        assert_eq!(rest, "");

        let (ty, rest) = parse_type("[[Ljava/lang/String;").unwrap();
        assert_eq!(ty, JType::Array(Box::new(JType::Array(Box::new(JType::Object("java/lang/String".into()))))));
        assert_eq!(rest, "");
    }

    #[test]
    fn parse_field_no_class() {
        let sig = FieldSig::parse("count:I").unwrap();
        assert_eq!(sig.class, "");
        assert_eq!(sig.name, "count");
        assert_eq!(sig.ty, JType::Int);
    }
}
