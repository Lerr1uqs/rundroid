//! Framework builtin class catalog —— 所有 Android framework / Java utility class 的
//! 声明式定义集中于此。
//!
//! 每个函数构建一个 [`FrameworkClassSpec`]，handler 闭包捕获共享的 [`FrameworkCtx`]
//! 读写 VM 状态。`build_all` 汇总全部 builtin class，供 [`crate::framework::FrameworkRegistry`]
//! 物化注册进统一 `JniRegistry` authority。
//!
//! # 覆盖范围（design.md 初始 class 集）
//!
//! - context 家族：`Application` / `Context` / `ContextWrapper`
//! - package 家族：`PackageManager` / `PackageInfo` / `Signature` / `ApplicationInfo`
//! - 资源/容器：`AssetManager` / `Bundle`
//! - binder：`IBinder` / `IServiceManager`（interface 壳）
//! - Java 基础类型：`String` / `Class` / `Integer` / `Long` / `Boolean`
//! - Java 集合：`ArrayList`（带 per-instance 状态）/ `List`/`Map`/`Set`/`Iterator`/`Enumeration`（interface 壳）

use crate::apk_context::ApkContext;
use crate::args::JniArgs;
use crate::class::ClassKind;
use crate::dispatch::MethodImpl;
use crate::framework::context::FrameworkCtx;
use crate::framework::spec::{FrameworkClassSpec, FrameworkMethodSpec};
use crate::object_store::ObjectStorage;
use crate::types::{JType, JValue, MethodSig, ObjectId};
use std::sync::Arc;

// ============================================================================
// 常量 / 数据载体
// ============================================================================

/// 无 APK 运行时 `getPackageName()` 等返回的 mock 包名（design.md 要求支持 mock 数据路径）。
const MOCK_PACKAGE_NAME: &str = "com.rundroid.unknown";

/// 取 mock 包名（registry 装配单例时复用）。
pub(crate) fn mock_package_name() -> &'static str {
    MOCK_PACKAGE_NAME
}

/// `PackageInfo` 实例携带的元数据（存入 StubInstance）。
#[derive(Debug, Clone)]
pub struct PackageInfoData {
    /// 包名。
    pub package_name: String,
    /// 版本名（可选）。
    pub version_name: Option<String>,
    /// 版本号（可选）。
    pub version_code: Option<i32>,
    /// 签名对象 ObjectId 列表（`Signature` 实例）。
    pub signatures: Vec<ObjectId>,
}

/// `ApplicationInfo` 实例携带的元数据（存入 StubInstance）。
#[derive(Debug, Clone)]
pub struct ApplicationInfoData {
    /// 包名。
    pub package_name: String,
}

// ============================================================================
// Java 标准哈希算法（stub 行为需要确定可测）
// ============================================================================

/// Java `Arrays.hashCode(byte[])` —— `result = 1; result = 31*result + element`，
/// 全程 32 位回绕。element 为有符号 byte（符号扩展）。
pub(crate) fn arrays_hash_code(bytes: &[u8]) -> i32 {
    let mut result: i32 = 1;
    for &b in bytes {
        result = result.wrapping_mul(31).wrapping_add(b as i8 as i32);
    }
    result
}

/// Java `String.hashCode()` —— `h = 31*h + char`，全程 32 位回绕。
///
/// # 注意
///
/// 按 Unicode scalar value（`char`）迭代；对 BMP 字符与 Java 完全一致，
/// 对增补平面字符（surrogate pair）Java 会按两个 UTF-16 code unit 计算而这里按一个——
/// 这是 stub 的已知近似，签名/包名场景几乎不涉及增补字符。
pub(crate) fn string_hash_code(s: &str) -> i32 {
    let mut h: i32 = 0;
    for c in s.chars() {
        h = h.wrapping_mul(31).wrapping_add(c as i32);
    }
    h
}

// ============================================================================
// build_all —— 汇总全部 builtin class
// ============================================================================

/// 构建全部 framework builtin class spec。
///
/// 顺序无强约束，但 context 家族在最前便于阅读。
pub fn build_all(ctx: &FrameworkCtx) -> Vec<FrameworkClassSpec> {
    vec![
        // —— context 家族 ——
        build_context(ctx),
        build_context_wrapper(ctx),
        build_application(ctx),
        // —— package 家族 ——
        build_package_manager(ctx),
        build_package_info(ctx),
        build_signature(ctx),
        build_application_info(ctx),
        // —— 资源 / 容器 ——
        build_asset_manager(ctx),
        build_bundle(ctx),
        // —— binder（interface 壳）——
        build_interface_shell("android/os/IBinder"),
        build_interface_shell("android/os/IServiceManager"),
        // —— Java 基础类型 ——
        build_string(ctx),
        build_class(ctx),
        build_integer(ctx),
        build_long(ctx),
        build_boolean(ctx),
        // —— Java 集合 ——
        build_array_list(ctx),
        build_interface_shell("java/util/List"),
        build_interface_shell("java/util/Map"),
        build_interface_shell("java/util/Set"),
        build_interface_shell("java/util/Iterator"),
        build_interface_shell("java/util/Enumeration"),
    ]
}

// ============================================================================
// context 家族
// ============================================================================

/// 给一个 context-like class 追加 Context 行为方法（getPackageName / getSystemService 等）。
///
/// `Context` / `ContextWrapper` / `Application` 在真实 Android 上都表现为 context，
/// 这里把同一组方法注册到三者，使 dispatch 不依赖继承解析（当前 dispatch 按精确 class 查）。
fn add_context_methods(spec: &mut FrameworkClassSpec, ctx: &FrameworkCtx) {
    let class = spec.class_name.clone();

    // getPackageName()Ljava/lang/String;
    let ctx_pkg = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.clone(),
            name: "getPackageName".into(),
            args: vec![],
            ret: JType::Object("java/lang/String".into()),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |_args: &JniArgs| {
            // 优先读 ApkContext；无 APK 走 mock 包名（design.md mock 数据路径）
            let name = {
                let apk = ctx_pkg.apk.read().unwrap();
                apk.as_ref().map(|a| a.package_name.clone()).unwrap_or_else(|| MOCK_PACKAGE_NAME.to_string())
            };
            let oid = ctx_pkg.intern_string(&name)?;
            Ok(JValue::Object(oid))
        })),
    });

    // getPackageManager()Landroid/content/pm/PackageManager;
    let ctx_pm = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.clone(),
            name: "getPackageManager".into(),
            args: vec![],
            ret: JType::Object("android/content/pm/PackageManager".into()),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |_args: &JniArgs| {
            // 单例在 install 时分配；这里只读
            let s = ctx_pm.singletons.lock().unwrap();
            match s.package_manager {
                Some(oid) => Ok(JValue::Object(oid)),
                None => Ok(JValue::Null),
            }
        })),
    });

    // getApplicationInfo()Landroid/content/pm/ApplicationInfo;
    let ctx_ai = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.clone(),
            name: "getApplicationInfo".into(),
            args: vec![],
            ret: JType::Object("android/content/pm/ApplicationInfo".into()),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |_args: &JniArgs| {
            let s = ctx_ai.singletons.lock().unwrap();
            match s.application_info {
                Some(oid) => Ok(JValue::Object(oid)),
                None => Ok(JValue::Null),
            }
        })),
    });

    // getAssets()Landroid/content/res/AssetManager;
    let ctx_am = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.clone(),
            name: "getAssets".into(),
            args: vec![],
            ret: JType::Object("android/content/res/AssetManager".into()),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |_args: &JniArgs| {
            let s = ctx_am.singletons.lock().unwrap();
            match s.asset_manager {
                Some(oid) => Ok(JValue::Object(oid)),
                None => Ok(JValue::Null),
            }
        })),
    });

    // getSystemService(Ljava/lang/String;)Ljava/lang/Object;
    let ctx_ss = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class,
            name: "getSystemService".into(),
            args: vec![JType::Object("java/lang/String".into())],
            ret: JType::Object("java/lang/Object".into()),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            // null name → null；未知 service → null（与真实 Android 一致）
            let name = match ctx_ss.read_string_arg(args, 0)? {
                Some(n) => n,
                None => return Ok(JValue::Null),
            };
            Ok(ctx_ss.lookup_service(&name).map(JValue::Object).unwrap_or(JValue::Null))
        })),
    });
}

/// `android/content/Context` —— 抽象 context（注册为 class，承载 context 方法）。
fn build_context(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    let mut spec = FrameworkClassSpec::builder("android/content/Context")
        .kind(ClassKind::Class)
        .build();
    add_context_methods(&mut spec, ctx);
    spec
}

/// `android/content/ContextWrapper` —— 继承 Context，行为同 context。
fn build_context_wrapper(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    let mut spec = FrameworkClassSpec::builder("android/content/ContextWrapper")
        .kind(ClassKind::Class)
        .superclass(Some("android/content/Context"))
        .build();
    add_context_methods(&mut spec, ctx);
    spec
}

/// `android/app/Application` —— 继承 Context，行为同 context。
fn build_application(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    let mut spec = FrameworkClassSpec::builder("android/app/Application")
        .kind(ClassKind::Class)
        .superclass(Some("android/content/ContextWrapper"))
        .build();
    add_context_methods(&mut spec, ctx);
    spec
}

// ============================================================================
// package 家族
// ============================================================================

/// `android/content/pm/PackageManager`。
///
/// `getPackageInfo(name, flags)` 从 `ApkContext` 构建 `PackageInfo` 实例返回。
fn build_package_manager(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    let class = "android/content/pm/PackageManager";
    let mut spec = FrameworkClassSpec::builder(class)
        .kind(ClassKind::Class)
        .build();

    // getPackageInfo(Ljava/lang/String;I)Landroid/content/pm/PackageInfo;
    let ctx_gpi = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.into(),
            name: "getPackageInfo".into(),
            args: vec![JType::Object("java/lang/String".into()), JType::Int],
            ret: JType::Object("android/content/pm/PackageInfo".into()),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            // name 入参（当前 stub 不按 name 过滤，统一返回当前 apk 的 PackageInfo；
            // 无 APK 时仍构造一个 mock PackageInfo，保证无 APK 可运行）
            let _requested = ctx_gpi.read_string_arg(args, 0)?;

            // 取出 apk 元数据（owned），释放 apk 读锁后再分配对象
            let (pkg, ver_name, ver_code, sig_oids) = {
                let apk = ctx_gpi.apk.read().unwrap();
                let apk_ref: Option<&ApkContext> = apk.as_ref();
                let pkg = apk_ref.map(|a| a.package_name.clone()).unwrap_or_else(|| MOCK_PACKAGE_NAME.to_string());
                let ver_name = apk_ref.and_then(|a| a.version_name.clone());
                let ver_code = apk_ref.and_then(|a| a.version_code);
                let sig_oids: Vec<ObjectId> = apk_ref
                    .map(|a| {
                        a.signatures
                            .iter()
                            .filter_map(|s| ctx_gpi.intern_stub(
                                "android/content/pm/Signature",
                                ObjectStorage::StubInstance { data: Box::new(s.bytes.clone()) },
                            ).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                (pkg, ver_name, ver_code, sig_oids)
            };

            let data = PackageInfoData {
                package_name: pkg,
                version_name: ver_name,
                version_code: ver_code,
                signatures: sig_oids,
            };
            let oid = ctx_gpi.intern_stub(
                "android/content/pm/PackageInfo",
                ObjectStorage::StubInstance { data: Box::new(data) },
            )?;
            Ok(JValue::Object(oid))
        })),
    });

    spec
}

/// `android/content/pm/PackageInfo` —— 读取自身 StubInstance 元数据。
fn build_package_info(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    let class = "android/content/pm/PackageInfo";
    let mut spec = FrameworkClassSpec::builder(class)
        .kind(ClassKind::Class)
        .interface("android/os/Parcelable")
        .build();

    // getVersionName()Ljava/lang/String;
    let ctx_vn = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.into(),
            name: "getVersionName".into(),
            args: vec![],
            ret: JType::Object("java/lang/String".into()),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("getVersionName 缺少 this".into()))?;
            let data: PackageInfoData = ctx_vn.read_stub(this)?;
            match data.version_name {
                Some(name) => {
                    let oid = ctx_vn.intern_string(&name)?;
                    Ok(JValue::Object(oid))
                }
                None => Ok(JValue::Null),
            }
        })),
    });

    // getVersionCode()I
    let ctx_vc = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.into(),
            name: "getVersionCode".into(),
            args: vec![],
            ret: JType::Int,
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("getVersionCode 缺少 this".into()))?;
            let data: PackageInfoData = ctx_vc.read_stub(this)?;
            Ok(JValue::Int(data.version_code.unwrap_or(0)))
        })),
    });

    // getPackageName()Ljava/lang/String;
    let ctx_pn = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.into(),
            name: "getPackageName".into(),
            args: vec![],
            ret: JType::Object("java/lang/String".into()),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("getPackageName 缺少 this".into()))?;
            let data: PackageInfoData = ctx_pn.read_stub(this)?;
            let oid = ctx_pn.intern_string(&data.package_name)?;
            Ok(JValue::Object(oid))
        })),
    });

    spec
}

/// `android/content/pm/Signature` —— 实例持有原始签名字节（StubInstance<Vec<u8>>）。
fn build_signature(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    let class = "android/content/pm/Signature";
    let mut spec = FrameworkClassSpec::builder(class)
        .kind(ClassKind::Class)
        .interface("android/os/Parcelable")
        .build();

    // 构造器 <init>([B)V —— 物化为 <init> instance method。
    // 把入参 byte[] 存入 this 的 StubInstance（覆盖空壳）。
    let ctx_init = ctx.clone();
    spec.constructors.push(crate::framework::spec::FrameworkConstructorSpec {
        args: vec![JType::Array(Box::new(JType::Byte))],
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("<init> 缺少 this".into()))?;
            let bytes = ctx_init.read_byte_array_arg(args, 0)?.unwrap_or_default();
            // 覆盖 this 的 StubInstance 数据为签名字节
            let mut store = ctx_init.objects.lock().unwrap();
            if let Some(ObjectStorage::StubInstance { data }) = store.storage_mut(this) {
                *data = Box::new(bytes);
            } else {
                return Err(crate::error::JniError::Internal(format!(
                    "Signature.<init> 的 this({this}) 不是 StubInstance"
                )));
            }
            Ok(JValue::Void)
        })),
    });

    // hashCode()I —— Arrays.hashCode(mSignature)
    let ctx_hc = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig { class: class.into(), name: "hashCode".into(), args: vec![], ret: JType::Int },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("hashCode 缺少 this".into()))?;
            let bytes: Vec<u8> = ctx_hc.read_stub(this)?;
            Ok(JValue::Int(arrays_hash_code(&bytes)))
        })),
    });

    // equals(Ljava/lang/Object;)Z —— 逐字节比较签名字节
    let ctx_eq = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.into(),
            name: "equals".into(),
            args: vec![JType::Object("java/lang/Object".into())],
            ret: JType::Boolean,
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("equals 缺少 this".into()))?;
            let other = match args.object_at(0)? {
                Some(oid) => oid,
                None => return Ok(JValue::Boolean(false)),
            };
            let self_bytes: Vec<u8> = ctx_eq.read_stub(this)?;
            // 另一个对象必须是同型 Signature stub
            let other_bytes: Vec<u8> = ctx_eq.read_stub(other).unwrap_or_default();
            Ok(JValue::Boolean(self_bytes == other_bytes))
        })),
    });

    // toByteArray()[B —— 返回签名字节的副本
    let ctx_ba = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.into(),
            name: "toByteArray".into(),
            args: vec![],
            ret: JType::Array(Box::new(JType::Byte)),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("toByteArray 缺少 this".into()))?;
            let bytes: Vec<u8> = ctx_ba.read_stub(this)?;
            let oid = ctx_ba.intern_byte_array(&bytes)?;
            Ok(JValue::Object(oid))
        })),
    });

    spec
}

/// `android/content/pm/ApplicationInfo` —— 实例持有包名（StubInstance）。
fn build_application_info(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    let class = "android/content/pm/ApplicationInfo";
    let mut spec = FrameworkClassSpec::builder(class)
        .kind(ClassKind::Class)
        .interface("android/os/Parcelable")
        .build();

    // getPackageName()Ljava/lang/String;
    let ctx_pn = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.into(),
            name: "getPackageName".into(),
            args: vec![],
            ret: JType::Object("java/lang/String".into()),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("getPackageName 缺少 this".into()))?;
            let data: ApplicationInfoData = ctx_pn.read_stub(this)?;
            let oid = ctx_pn.intern_string(&data.package_name)?;
            Ok(JValue::Object(oid))
        })),
    });

    spec
}

// ============================================================================
// 资源 / 容器（最小 stub）
// ============================================================================

/// `android/content/res/AssetManager` —— 最小 stub（class 壳）。
///
/// assets 实际内容通过 `ApkContext::asset_names` 暴露名册；
/// 具体 IO/读取延后到 APK 提取 change。
fn build_asset_manager(_ctx: &FrameworkCtx) -> FrameworkClassSpec {
    FrameworkClassSpec::builder("android/content/res/AssetManager")
        .kind(ClassKind::Class)
        .build()
}

/// `android/os/Bundle` —— 最小 stub（class 壳）。
///
/// string/int 的存取语义延后到完整 Bundle 实现；当前仅声明类型，
/// 供 guest 经 JNI `FindClass` 能定位。
fn build_bundle(_ctx: &FrameworkCtx) -> FrameworkClassSpec {
    FrameworkClassSpec::builder("android/os/Bundle")
        .kind(ClassKind::Class)
        .superclass(Some("java/lang/Object"))
        .interface("android/os/Parcelable")
        .build()
}

/// 通用 interface 壳：仅声明类型与继承，不带 method 实现。
///
/// 用于 `IBinder` / `IServiceManager` / `List` / `Map` / `Set` / `Iterator` / `Enumeration`
/// 等「需要作为类型存在、但当前不需要行为」的 interface。
fn build_interface_shell(name: &str) -> FrameworkClassSpec {
    FrameworkClassSpec::builder(name)
        .kind(ClassKind::Interface)
        .superclass(None)
        .build()
}

// ============================================================================
// Java 基础类型
// ============================================================================

/// `java/lang/String` —— 读自身 String storage。
fn build_string(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    let class = "java/lang/String";
    let mut spec = FrameworkClassSpec::builder(class)
        .kind(ClassKind::Class)
        .superclass(None)
        .interface("java/io/Serializable")
        .interface("java/lang/Comparable")
        .build();

    // length()I
    let ctx_len = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig { class: class.into(), name: "length".into(), args: vec![], ret: JType::Int },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("length 缺少 this".into()))?;
            let s = ctx_len.read_string_value(this)?;
            // Java length() 返回 UTF-16 code unit 数；这里按 char 数近似（BMP 一致）
            Ok(JValue::Int(s.chars().count() as i32))
        })),
    });

    // hashCode()I —— Java String.hashCode
    let ctx_hc = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig { class: class.into(), name: "hashCode".into(), args: vec![], ret: JType::Int },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("hashCode 缺少 this".into()))?;
            let s = ctx_hc.read_string_value(this)?;
            Ok(JValue::Int(string_hash_code(&s)))
        })),
    });

    // equals(Ljava/lang/Object;)Z
    let ctx_eq = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.into(),
            name: "equals".into(),
            args: vec![JType::Object("java/lang/Object".into())],
            ret: JType::Boolean,
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("equals 缺少 this".into()))?;
            let other = match args.object_at(0)? {
                Some(oid) => oid,
                None => return Ok(JValue::Boolean(false)),
            };
            let a = ctx_eq.read_string_value(this)?;
            // 另一对象也必须是 String
            let b = ctx_eq.read_string_value(other).unwrap_or_default();
            Ok(JValue::Boolean(a == b))
        })),
    });

    // getBytes()[B
    let ctx_gb = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.into(),
            name: "getBytes".into(),
            args: vec![],
            ret: JType::Array(Box::new(JType::Byte)),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("getBytes 缺少 this".into()))?;
            let s = ctx_gb.read_string_value(this)?;
            let oid = ctx_gb.intern_byte_array(s.as_bytes())?;
            Ok(JValue::Object(oid))
        })),
    });

    spec
}

/// `java/lang/Class` —— 实例持有类名（StubInstance<String>）。
fn build_class(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    let class = "java/lang/Class";
    let mut spec = FrameworkClassSpec::builder(class)
        .kind(ClassKind::Class)
        .superclass(None)
        .build();

    // getName()Ljava/lang/String;
    let ctx_gn = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.into(),
            name: "getName".into(),
            args: vec![],
            ret: JType::Object("java/lang/String".into()),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("getName 缺少 this".into()))?;
            let name: String = ctx_gn.read_stub(this)?;
            let oid = ctx_gn.intern_string(&name)?;
            Ok(JValue::Object(oid))
        })),
    });

    spec
}

/// `java/lang/Integer` —— `intValue()` 读自身 Wrapper。
fn build_integer(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    wrapper_class("java/lang/Integer", "intValue", JType::Int, ctx)
}

/// `java/lang/Long` —— `longValue()` 读自身 Wrapper。
fn build_long(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    wrapper_class("java/lang/Long", "longValue", JType::Long, ctx)
}

/// `java/lang/Boolean` —— `booleanValue()` 读自身 Wrapper。
fn build_boolean(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    wrapper_class("java/lang/Boolean", "booleanValue", JType::Boolean, ctx)
}

/// 通用 primitive wrapper class 构造（Integer/Long/Boolean 共用）。
///
/// `xxxValue()` 读自身 Wrapper storage 的 value 并原样返回。
fn wrapper_class(class: &str, value_method: &str, primitive: JType, ctx: &FrameworkCtx) -> FrameworkClassSpec {
    let class_owned = class.to_string();
    let method_name = value_method.to_string();
    let mut spec = FrameworkClassSpec::builder(class)
        .kind(ClassKind::Class)
        .superclass(Some("java/lang/Number"))
        .build();

    let ctx_val = ctx.clone();
    let expected = primitive.clone();
    let method_name_for_err = method_name.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class_owned,
            name: method_name,
            args: vec![],
            ret: primitive,
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args
                .this()
                .ok_or_else(|| crate::error::JniError::Internal(format!("{method_name_for_err} 缺少 this")))?;
            let val = ctx_val.read_wrapper_value(this)?;
            // 校验 wrapper 存储的 primitive 与期望一致（fail-fast）
            if val.jtype() != expected {
                return Err(crate::error::JniError::TypeMismatch {
                    expected: expected.clone(),
                    actual: val.jtype(),
                });
            }
            Ok(val)
        })),
    });

    spec
}

// ============================================================================
// Java 集合
// ============================================================================

/// `java/util/ArrayList` —— per-instance `Vec<ObjectId>` 状态（StubInstance）。
///
/// 演示 framework stub 如何持有可变 per-instance 状态：
/// StubInstance 数据为 `Arc<Mutex<Vec<ObjectId>>>`，多个 handler 共享同一 Arc。
fn build_array_list(ctx: &FrameworkCtx) -> FrameworkClassSpec {
    let class = "java/util/ArrayList";
    let mut spec = FrameworkClassSpec::builder(class)
        .kind(ClassKind::Class)
        .superclass(Some("java/util/AbstractList"))
        .interface("java/util/List")
        .build();

    // 构造器 <init>()V —— 初始化空 Vec
    let ctx_init = ctx.clone();
    spec.constructors.push(crate::framework::spec::FrameworkConstructorSpec {
        args: vec![],
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("<init> 缺少 this".into()))?;
            let mut store = ctx_init.objects.lock().unwrap();
            if let Some(ObjectStorage::StubInstance { data }) = store.storage_mut(this) {
                *data = Box::new(std::sync::Mutex::new(Vec::<ObjectId>::new()));
            } else {
                return Err(crate::error::JniError::Internal(format!(
                    "ArrayList.<init> 的 this({this}) 不是 StubInstance"
                )));
            }
            Ok(JValue::Void)
        })),
    });

    // add(Ljava/lang/Object;)Z —— 追加元素，返回 true
    let ctx_add = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.into(),
            name: "add".into(),
            args: vec![JType::Object("java/lang/Object".into())],
            ret: JType::Boolean,
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("add 缺少 this".into()))?;
            let elem = args.object_at(0)?.unwrap_or(ObjectId(0));
            let list: std::sync::Arc<std::sync::Mutex<Vec<ObjectId>>> = ctx_add.read_stub(this)?;
            list.lock().unwrap().push(elem);
            Ok(JValue::Boolean(true))
        })),
    });

    // size()I
    let ctx_size = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig { class: class.into(), name: "size".into(), args: vec![], ret: JType::Int },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("size 缺少 this".into()))?;
            let list: std::sync::Arc<std::sync::Mutex<Vec<ObjectId>>> = ctx_size.read_stub(this)?;
            Ok(JValue::Int(list.lock().unwrap().len() as i32))
        })),
    });

    // get(I)Ljava/lang/Object;
    let ctx_get = ctx.clone();
    spec.instance_methods.push(FrameworkMethodSpec {
        sig: MethodSig {
            class: class.into(),
            name: "get".into(),
            args: vec![JType::Int],
            ret: JType::Object("java/lang/Object".into()),
        },
        is_static: false,
        imp: MethodImpl::RustNative(Arc::new(move |args: &JniArgs| {
            let this = args.this().ok_or_else(|| crate::error::JniError::Internal("get 缺少 this".into()))?;
            let idx = args.int_at(0)?;
            let list: std::sync::Arc<std::sync::Mutex<Vec<ObjectId>>> = ctx_get.read_stub(this)?;
            let guard = list.lock().unwrap();
            match guard.get(idx as usize) {
                Some(oid) => Ok(JValue::Object(*oid)),
                None => Ok(JValue::Null),
            }
        })),
    });

    spec
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::context::FrameworkCtx;
    use crate::framework::service::ServiceRegistry;
    use crate::object_store::ObjectStore;
    use crate::types::IdAllocator;
    use std::sync::{Arc, Mutex, RwLock};

    fn empty_ctx() -> FrameworkCtx {
        FrameworkCtx::new(
            Arc::new(Mutex::new(ObjectStore::new())),
            Arc::new(Mutex::new(IdAllocator::new())),
            Arc::new(RwLock::new(None)),
            Arc::new(Mutex::new(ServiceRegistry::new())),
        )
    }

    #[test]
    fn build_all_covers_initial_class_set() {
        let ctx = empty_ctx();
        let specs = build_all(&ctx);
        let names: Vec<&str> = specs.iter().map(|s| s.class_name.as_str()).collect();

        // design.md 初始 class 集
        for required in [
            "android/app/Application",
            "android/content/Context",
            "android/content/ContextWrapper",
            "android/content/pm/PackageManager",
            "android/content/pm/PackageInfo",
            "android/content/pm/Signature",
            "android/content/pm/ApplicationInfo",
            "android/content/res/AssetManager",
            "android/os/Bundle",
            "android/os/IBinder",
            "android/os/IServiceManager",
            "java/lang/String",
            "java/lang/Class",
            "java/lang/Integer",
            "java/lang/Long",
            "java/lang/Boolean",
            "java/util/ArrayList",
            "java/util/List",
            "java/util/Map",
            "java/util/Set",
            "java/util/Iterator",
            "java/util/Enumeration",
        ] {
            assert!(names.contains(&required), "build_all 缺少 builtin class `{required}`");
        }
    }

    #[test]
    fn all_specs_materialize_cleanly() {
        // 每个 builtin spec 都应能无错物化成 JClassDef（catalog 内部无重复签名）
        let ctx = empty_ctx();
        for spec in build_all(&ctx) {
            let name = spec.class_name.clone();
            spec.materialize().unwrap_or_else(|e| panic!("spec {name} 物化失败: {e}"));
        }
    }

    #[test]
    fn arrays_hash_code_matches_java_semantics() {
        // Java: Arrays.hashCode(new byte[]{1,2,3})
        //   r=1; r=31*1+1=32; r=31*32+2=994; r=31*994+3=30817
        assert_eq!(arrays_hash_code(&[1, 2, 3]), 30817);
        // 空数组 → 1（Java 语义：初始 result=1，无元素迭代）
        assert_eq!(arrays_hash_code(&[]), 1);
    }

    #[test]
    fn string_hash_code_matches_java_empty() {
        assert_eq!(string_hash_code(""), 0);
        // Java "A".hashCode() == 65
        assert_eq!(string_hash_code("A"), 65);
    }

    #[test]
    fn interface_shells_are_interface_kind() {
        let ctx = empty_ctx();
        let specs = build_all(&ctx);
        let ibinder = specs.iter().find(|s| s.class_name == "android/os/IBinder").unwrap();
        assert_eq!(ibinder.kind, ClassKind::Interface);
        assert!(ibinder.instance_methods.is_empty());
    }
}
