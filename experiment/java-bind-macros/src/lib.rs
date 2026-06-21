//! `java-bind-macros` — proc-macro 实现
//!
//! 提供两个 attribute macro：
//! - `#[java_class(name = "full/Class/Name")]` — 标注 struct，实现 `JavaObject`
//! - `#[java_impl(class = "full/Class/Name")]` — 标注 impl block，扫描方法注解并生成 inventory 条目
//!
//! # 设计说明
//!
//! `#[java_class]` 和 `#[java_impl]` 是独立的 proc-macro 调用，编译期无法共享 class name。
//! 解决方案：`#[java_impl]` 上显式标注 `class = "..."`。
//!
//! 这是当前 Rust proc-macro 架构的固有限制。未来可考虑：
//! - 用 `inventory` 在运行时按 type_name 自动关联（需要额外的 dispatch 层）
//! - 用 `linkme` 做全量扫描后匹配

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input, Attribute, ItemImpl, ItemStruct, Meta,
};

// ============================================================================
// 参数解析
// ============================================================================

struct NameValue {
    name: String,
}

impl Parse for NameValue {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let meta: Meta = input.parse()?;
        match meta {
            Meta::NameValue(nv) if nv.path.is_ident("name") => {
                let s = extract_str(&nv.value)?;
                Ok(NameValue { name: s })
            }
            _ => Err(syn::Error::new_spanned(
                meta,
                r#"期望格式: name = "full/Class/Name""#,
            )),
        }
    }
}

struct ClassValue {
    class: String,
}

impl Parse for ClassValue {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let meta: Meta = input.parse()?;
        match meta {
            Meta::NameValue(nv) if nv.path.is_ident("class") => {
                let s = extract_str(&nv.value)?;
                Ok(ClassValue { class: s })
            }
            _ => Err(syn::Error::new_spanned(
                meta,
                r#"期望格式: class = "full/Class/Name""#,
            )),
        }
    }
}

fn extract_str(expr: &syn::Expr) -> syn::Result<String> {
    if let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = expr
    {
        Ok(s.value())
    } else {
        Err(syn::Error::new_spanned(expr, "期望字符串字面量"))
    }
}

// ============================================================================
// #[java_class] — struct 属性宏
// ============================================================================

/// 标注 struct 对应 Java class，自动生成 `JavaObject` trait 实现。
///
/// 首次调用 `Type::reflect()` 时自动注册 type_map。
///
/// # 用法
/// ```ignore
/// #[java_class(name = "android/content/pm/Signature")]
/// struct Signature { _msig: Vec<u8> }
/// ```
#[proc_macro_attribute]
pub fn java_class(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as NameValue);
    let input = parse_macro_input!(item as ItemStruct);
    let struct_name = &input.ident;
    let java_name = &args.name;

    let expanded = quote! {
        #input

        impl ::java_bind::JavaObject for #struct_name {
            fn class_meta() -> &'static ::java_bind::ClassMeta {
                // 首次调用 class_meta 时自动注册 type_map
                use ::std::sync::OnceLock;
                static CLASS_META: OnceLock<&::java_bind::ClassMeta> = OnceLock::new();
                CLASS_META.get_or_init(|| {
                    ::java_bind::register_type_map(
                        ::std::any::type_name::<#struct_name>(),
                        #java_name,
                    );
                    ::java_bind::lookup_class_meta(#java_name)
                })
            }

            fn java_class_name() -> String {
                #java_name.to_string()
            }
        }
    };

    expanded.into()
}

// ============================================================================
// #[java_impl] — impl block 属性宏
// ============================================================================

/// 标注 impl block，扫描方法上的 `#[java_method]` / `#[java_field]` 注解，
/// 自动生成 `inventory::submit!` 注册条目。
///
/// 方法返回 `Self` 时自动识别为构造函数。
///
/// # 子注解
///
/// - `#[java_method = "descriptor"]` — 声明 JNI 方法映射（返回 Self → 自动识别为 ctor）
/// - `#[java_field(name = "fieldName", sig = "type")]` — 声明 JNI 字段 getter
///
/// # 用法
/// ```ignore
/// #[java_impl(class = "android/content/pm/Signature")]
/// impl Signature {
///     #[java_method = "Signature([B)V"]   // 返回 Self → 构造函数
///     fn new(sig: Vec<u8>) -> Self { Self { _msig: sig } }
///
///     #[java_method = "hashCode()I"]      // 返回 i32 → 普通实例方法
///     fn hash_code(&self) -> i32 { 0x12345678 }
///
///     #[java_field(name = "mSignature", sig = "[B")]
///     fn member_signature(&self) -> Vec<u8> { self._msig.clone() }
/// }
/// ```
#[proc_macro_attribute]
pub fn java_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ClassValue);
    let class_name = &args.class;
    let mut input = parse_macro_input!(item as ItemImpl);

    let mut method_submits = Vec::new();
    let mut field_submits = Vec::new();

    // 处理 impl items：剥离 java 辅助属性 + 收集 inventory submissions
    for item in &mut input.items {
        if let syn::ImplItem::Fn(method) = item {
            let (java_attrs, cleaned_attrs): (Vec<_>, Vec<_>) =
                method.attrs.iter().cloned().partition(|a| is_java_helper_attr(a));

            if java_attrs.is_empty() {
                continue;
            }

            // 剥离 java 辅助属性
            method.attrs = cleaned_attrs;

            let rust_fn_name = method.sig.ident.to_string();

            // 判断是否为 static method（没有 &self / &mut self / self 参数）
            let is_static = !method
                .sig
                .inputs
                .first()
                .map_or(false, |a| matches!(a, syn::FnArg::Receiver(_)));

            // 判断是否为构造函数：返回类型是 Self
            let returns_self = match &method.sig.output {
                syn::ReturnType::Type(_, ty) => is_self_type(ty),
                syn::ReturnType::Default => false,
            };

            for attr in &java_attrs {
                process_java_attr(
                    attr,
                    class_name,
                    &rust_fn_name,
                    is_static,
                    returns_self,
                    &mut method_submits,
                    &mut field_submits,
                );
            }
        }
    }

    // 直接 quote 修改后的 ItemImpl（它实现了 ToTokens）
    let expanded = quote! {
        #input

        #(#method_submits)*
        #(#field_submits)*
    };

    expanded.into()
}

// ============================================================================
// 辅助函数
// ============================================================================

fn is_java_helper_attr(attr: &Attribute) -> bool {
    attr.path().is_ident("java_method") || attr.path().is_ident("java_field")
}

fn process_java_attr(
    attr: &Attribute,
    class_name: &str,
    rust_fn_name: &str,
    is_static: bool,
    is_constructor: bool,
    method_submits: &mut Vec<proc_macro2::TokenStream>,
    field_submits: &mut Vec<proc_macro2::TokenStream>,
) {
    if attr.path().is_ident("java_method") {
        if let Some(jni_sig) = parse_string_value(attr) {
            let java_name = method_name_from_sig(&jni_sig);
            let kind = if is_constructor {
                quote! { ::java_bind::MethodKind::Constructor }
            } else if is_static {
                quote! { ::java_bind::MethodKind::Static }
            } else {
                quote! { ::java_bind::MethodKind::Instance }
            };
            method_submits.push(quote! {
                ::java_bind::inventory::submit! {
                    ::java_bind::MethodReg {
                        class_name: #class_name,
                        java_name: #java_name,
                        jni_sig: #jni_sig,
                        rust_fn_name: #rust_fn_name,
                        kind: #kind,
                    }
                }
            });
        }
    } else if attr.path().is_ident("java_field") {
        if let Some((field_name, jni_sig)) = parse_field_attr(attr) {
            field_submits.push(quote! {
                ::java_bind::inventory::submit! {
                    ::java_bind::FieldReg {
                        class_name: #class_name,
                        field_name: #field_name,
                        jni_sig: #jni_sig,
                        rust_fn_name: #rust_fn_name,
                    }
                }
            });
        }
    }
}

/// 解析 `#[attr = "value"]` 格式（Meta::NameValue）
fn parse_string_value(attr: &Attribute) -> Option<String> {
    if let Meta::NameValue(nv) = &attr.meta {
        if let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) = &nv.value
        {
            return Some(s.value());
        }
    }
    None
}

/// 解析 `#[java_field(name = "xxx", sig = "yyy")]` 格式（Meta::List）
fn parse_field_attr(attr: &Attribute) -> Option<(String, String)> {
    if let Meta::List(list) = &attr.meta {
        let tokens = list.tokens.to_string();
        let mut name = None;
        let mut sig = None;
        for part in tokens.split(',') {
            if let Some((k, v)) = part.trim().split_once('=') {
                let k = k.trim();
                let v = v.trim().trim_matches('"');
                match k {
                    "name" => name = Some(v.to_string()),
                    "sig" => sig = Some(v.to_string()),
                    _ => {}
                }
            }
        }
        if let (Some(n), Some(s)) = (name, sig) {
            return Some((n, s));
        }
    }
    None
}

/// 从 JNI descriptor 提取方法名（`(` 之前的部分）
fn method_name_from_sig(jni_sig: &str) -> String {
    jni_sig
        .find('(')
        .map(|p| &jni_sig[..p])
        .unwrap_or(jni_sig)
        .to_string()
}

/// 判断 syn::Type 是否是 `Self`
fn is_self_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        type_path.path.segments.len() == 1
            && type_path.path.segments[0].ident == "Self"
    } else {
        false
    }
}
