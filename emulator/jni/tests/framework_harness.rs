//! Framework stub harness cases —— 端到端覆盖 testing-harness capability 要求的四个场景。
//!
//! 每个 case 走真实 dispatch 主线（`JniRegistry::dispatch_call`），验证 framework
//! builtin 经 `FrameworkRegistry::install` 注册后行为正确。这是 change
//! `android-framework-stubs` 的回归 harness，任何 giant signature switch 回归或
//! 注册主线偏离都会在此暴露。
//!
//! # 覆盖场景（testing-harness spec）
//!
//! - `getPackageName()` —— package 元数据路径
//! - `getPackageInfo()` + `PackageInfo.getVersionName()` —— package info 路径
//! - `Signature.hashCode()` —— 签名 hash 路径
//! - `getSystemService()` —— service registry 路径

use rundroid_jni::{
    AndroidVM, ApkContext, FrameworkRegistry, JniArgs, JType, JValue, MethodSig,
    ObjectStorage, ObjectId, RefTable,
};

// ============================================================================
// harness helpers
// ============================================================================

/// 构造一个装好 framework stub 的 AndroidVM（带给定 APK）。
fn runtime_with_apk(apk: ApkContext) -> (AndroidVM, FrameworkRegistry) {
    let mut vm = AndroidVM::new().with_apk(apk);
    let mut reg = FrameworkRegistry::new();
    reg.install(&mut vm).expect("framework install 失败");
    (vm, reg)
}

/// 把 Rust 字符串落成 Java String 对象（经 vm 的对象池 + id 分配器）。
fn intern_string(vm: &mut AndroidVM, s: &str) -> ObjectId {
    let oid = vm.object_id_alloc.lock().unwrap().object();
    vm.objects
        .lock()
        .unwrap()
        .insert(oid, "java/lang/String".into(), ObjectStorage::String(s.into()))
        .expect("intern_string insert");
    oid
}

/// 读取一个 String 对象的内容。
fn read_string(vm: &AndroidVM, oid: ObjectId) -> String {
    let store = vm.objects.lock().unwrap();
    match store.storage(oid) {
        Some(ObjectStorage::String(s)) => s.clone(),
        other => panic!("期望 String storage，实际 {other:?}"),
    }
}

/// 调用 instance method（自动构造临时 RefTable）。
fn call(vm: &AndroidVM, sig: &MethodSig, this: ObjectId, args: Vec<JValue>) -> JValue {
    let mut jni_args = JniArgs::from_vec(args);
    jni_args.set_this(this);
    let mut refs = RefTable::new();
    vm.classes.dispatch_call(sig, &jni_args, &mut refs).expect(&format!("dispatch {sig} 失败"))
}

/// 独立实现 Java `Arrays.hashCode(byte[])`，用于校验 Signature.hashCode 用的是规范公式。
fn java_arrays_hash(bytes: &[u8]) -> i32 {
    let mut r: i32 = 1;
    for &b in bytes {
        r = r.wrapping_mul(31).wrapping_add(b as i8 as i32);
    }
    r
}

// ============================================================================
// 场景 1：getPackageName()
// ============================================================================

#[test]
fn harness_get_package_name_reads_apk() {
    let apk = ApkContext::new("com.example.app".into());
    let (vm, reg) = runtime_with_apk(apk);

    let ctx_oid = reg.new_stub_instance(&vm, "android/content/Context").unwrap();
    let sig = MethodSig {
        class: "android/content/Context".into(),
        name: "getPackageName".into(),
        args: vec![],
        ret: JType::Object("java/lang/String".into()),
    };
    let result = call(&vm, &sig, ctx_oid, vec![]);
    let oid = result.as_object().expect("getPackageName 应返回 String 对象");
    assert_eq!(read_string(&vm, oid), "com.example.app");
}

#[test]
fn harness_get_package_name_without_apk_returns_mock() {
    // 无 APK 运行（mock 数据路径）—— design.md 要求
    let mut vm = AndroidVM::new();
    let mut reg = FrameworkRegistry::new();
    reg.install(&mut vm).unwrap();

    let ctx_oid = reg.new_stub_instance(&vm, "android/content/Context").unwrap();
    let sig = MethodSig {
        class: "android/content/Context".into(),
        name: "getPackageName".into(),
        args: vec![],
        ret: JType::Object("java/lang/String".into()),
    };
    let result = call(&vm, &sig, ctx_oid, vec![]);
    let oid = result.as_object().expect("无 APK 也应返回 mock String");
    assert_eq!(read_string(&vm, oid), "com.rundroid.unknown");
}

// ============================================================================
// 场景 2：getPackageInfo() + PackageInfo 元数据
// ============================================================================

#[test]
fn harness_get_package_info_returns_packageinfo_with_metadata() {
    let apk = ApkContext::new("com.example.app".into())
        .with_version(Some("1.2.3".into()), Some(42))
        .with_signature(vec![0xCA, 0xFE]);
    let (mut vm, reg) = runtime_with_apk(apk);

    // 1. 取 PackageManager（经 Context.getPackageManager，验证单例链路）
    let ctx_oid = reg.new_stub_instance(&vm, "android/content/Context").unwrap();
    let get_pm = MethodSig {
        class: "android/content/Context".into(),
        name: "getPackageManager".into(),
        args: vec![],
        ret: JType::Object("android/content/pm/PackageManager".into()),
    };
    let pm_oid = call(&vm, &get_pm, ctx_oid, vec![]).as_object().expect("getPackageManager 应返回对象");

    // 2. getPackageInfo(name, flags)
    let name_oid = intern_string(&mut vm, "com.example.app");
    let get_info = MethodSig {
        class: "android/content/pm/PackageManager".into(),
        name: "getPackageInfo".into(),
        args: vec![JType::Object("java/lang/String".into()), JType::Int],
        ret: JType::Object("android/content/pm/PackageInfo".into()),
    };
    let info_result = call(&vm, &get_info, pm_oid, vec![JValue::Object(name_oid), JValue::Int(0)]);
    let info_oid = info_result.as_object().expect("getPackageInfo 应返回 PackageInfo");

    // 类型正确
    {
        let objects = vm.objects.lock().unwrap();
        assert_eq!(
            objects.class_name(info_oid),
            Some("android/content/pm/PackageInfo")
        );
    }

    // 3. PackageInfo.getVersionName() → "1.2.3"
    let get_vn = MethodSig {
        class: "android/content/pm/PackageInfo".into(),
        name: "getVersionName".into(),
        args: vec![],
        ret: JType::Object("java/lang/String".into()),
    };
    let vn_oid = call(&vm, &get_vn, info_oid, vec![]).as_object().expect("versionName 应非空");
    assert_eq!(read_string(&vm, vn_oid), "1.2.3");

    // 4. PackageInfo.getVersionCode() → 42
    let get_vc = MethodSig {
        class: "android/content/pm/PackageInfo".into(),
        name: "getVersionCode".into(),
        args: vec![],
        ret: JType::Int,
    };
    assert_eq!(call(&vm, &get_vc, info_oid, vec![]), JValue::Int(42));
}

// ============================================================================
// 场景 3：Signature.hashCode()
// ============================================================================

#[test]
fn harness_signature_hash_code_matches_canonical_formula() {
    let apk = ApkContext::new("com.example.app".into());
    let (vm, reg) = runtime_with_apk(apk);

    let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02];
    let sig_oid = reg.new_signature(&vm, bytes.clone()).unwrap();

    let hash_sig = MethodSig {
        class: "android/content/pm/Signature".into(),
        name: "hashCode".into(),
        args: vec![],
        ret: JType::Int,
    };
    let result = call(&vm, &hash_sig, sig_oid, vec![]);
    assert_eq!(result, JValue::Int(java_arrays_hash(&bytes)));
}

#[test]
fn harness_signature_to_byte_array_roundtrip() {
    let apk = ApkContext::new("com.example.app".into());
    let (vm, reg) = runtime_with_apk(apk);

    let bytes = vec![0x11, 0x22, 0x33];
    let sig_oid = reg.new_signature(&vm, bytes).unwrap();

    let to_ba = MethodSig {
        class: "android/content/pm/Signature".into(),
        name: "toByteArray".into(),
        args: vec![],
        ret: JType::Array(Box::new(JType::Byte)),
    };
    let arr_oid = call(&vm, &to_ba, sig_oid, vec![]).as_object().expect("toByteArray 应返回 byte[]");

    let store = vm.objects.lock().unwrap();
    match store.storage(arr_oid) {
        Some(ObjectStorage::PrimitiveArray { jtype, elements }) => {
            assert_eq!(*jtype, JType::Byte);
            let got: Vec<u8> = elements.iter().map(|v| match v {
                JValue::Byte(b) => *b as u8,
                _ => panic!("非 Byte 元素"),
            }).collect();
            assert_eq!(got, vec![0x11, 0x22, 0x33]);
        }
        other => panic!("期望 PrimitiveArray，实际 {other:?}"),
    }
}

#[test]
fn harness_signature_equals_by_bytes() {
    let apk = ApkContext::new("com.example.app".into());
    let (vm, reg) = runtime_with_apk(apk);

    let a = reg.new_signature(&vm, vec![1, 2, 3]).unwrap();
    let b = reg.new_signature(&vm, vec![1, 2, 3]).unwrap();
    let c = reg.new_signature(&vm, vec![9, 9]).unwrap();

    let eq = MethodSig {
        class: "android/content/pm/Signature".into(),
        name: "equals".into(),
        args: vec![JType::Object("java/lang/Object".into())],
        ret: JType::Boolean,
    };
    assert_eq!(call(&vm, &eq, a, vec![JValue::Object(b)]), JValue::Boolean(true));
    assert_eq!(call(&vm, &eq, a, vec![JValue::Object(c)]), JValue::Boolean(false));
    assert_eq!(call(&vm, &eq, a, vec![JValue::Null]), JValue::Boolean(false));
}

// ============================================================================
// 场景 4：getSystemService() —— service registry
// ============================================================================

#[test]
fn harness_get_system_service_returns_registered_stub() {
    let apk = ApkContext::new("com.example.app".into());
    let (mut vm, reg) = runtime_with_apk(apk);

    let ctx_oid = reg.new_stub_instance(&vm, "android/content/Context").unwrap();
    // getSystemService 的入参 String 须经对象池；handler 内经 read_string_arg 读取 name。
    let phone_oid = intern_string(&mut vm, "phone");
    let nope_oid = intern_string(&mut vm, "definitely-not-a-service");

    let gss = MethodSig {
        class: "android/content/Context".into(),
        name: "getSystemService".into(),
        args: vec![JType::Object("java/lang/String".into())],
        ret: JType::Object("java/lang/Object".into()),
    };

    // 已注册的 service（"phone"）→ 返回稳定 stub
    let first = call(&vm, &gss, ctx_oid, vec![JValue::Object(phone_oid)]);
    let first_oid = first.as_object().expect("已注册 service 应返回 stub 对象");

    // 再查一次 → 同一 oid（稳定 stub）
    let second = call(&vm, &gss, ctx_oid, vec![JValue::Object(phone_oid)]);
    assert_eq!(second.as_object(), Some(first_oid), "service stub 必须稳定");

    // 未知 service → null（与真实 Android 一致）
    let unknown = call(&vm, &gss, ctx_oid, vec![JValue::Object(nope_oid)]);
    assert!(unknown.is_null(), "未知 service 应返回 null");
}

// ============================================================================
// 附加：Java 基础类型 stub 行为
// ============================================================================

#[test]
fn harness_integer_int_value_reads_wrapper() {
    let apk = ApkContext::new("com.example.app".into());
    let (mut vm, _reg) = runtime_with_apk(apk);

    // 构造一个 Integer wrapper 对象
    let oid = vm.object_id_alloc.lock().unwrap().object();
    vm.objects.lock().unwrap().insert(
        oid,
        "java/lang/Integer".into(),
        ObjectStorage::Wrapper { jtype: JType::Int, value: JValue::Int(2024) },
    ).unwrap();

    let sig = MethodSig {
        class: "java/lang/Integer".into(),
        name: "intValue".into(),
        args: vec![],
        ret: JType::Int,
    };
    assert_eq!(call(&vm, &sig, oid, vec![]), JValue::Int(2024));
}

#[test]
fn harness_string_length_and_hashcode() {
    let apk = ApkContext::new("com.example.app".into());
    let (mut vm, _reg) = runtime_with_apk(apk);

    let oid = intern_string(&mut vm, "hello");

    let len_sig = MethodSig {
        class: "java/lang/String".into(),
        name: "length".into(),
        args: vec![],
        ret: JType::Int,
    };
    assert_eq!(call(&vm, &len_sig, oid, vec![]), JValue::Int(5));

    // Java "hello".hashCode() == 99162322
    let hc_sig = MethodSig {
        class: "java/lang/String".into(),
        name: "hashCode".into(),
        args: vec![],
        ret: JType::Int,
    };
    assert_eq!(call(&vm, &hc_sig, oid, vec![]), JValue::Int(99162322));
}

// ============================================================================
// 附加：builtin 与 Python shim 共用同一 class/member 主线
// ============================================================================

/// 验证 Rust builtin framework class 可被 Python-shim 风格的 override 合并覆盖，
/// 证明两者写入同一套 `JClassDef` authority（change spec 要求）。
#[test]
fn harness_builtin_and_override_share_authority() {
    use rundroid_jni::dispatch::MethodImpl;
    use std::sync::Arc;

    let apk = ApkContext::new("com.example.app".into());
    let (mut vm, _reg) = runtime_with_apk(apk);

    // 用一个 override class def 合并覆盖 Context.getPackageName
    let mut override_def = rundroid_jni::JClassDef::new(
        rundroid_jni::types::ClassId(0),
        "android/content/Context".into(),
    );
    let sig = MethodSig {
        class: "android/content/Context".into(),
        name: "getPackageName".into(),
        args: vec![],
        ret: JType::Object("java/lang/String".into()),
    };
    override_def
        .override_method(sig.clone(), false, MethodImpl::RustNative(Arc::new(|_| Ok(JValue::Int(-1)))))
        .unwrap();
    vm.classes.register_or_merge_class(override_def).unwrap();

    // 现在调用 getPackageName 应命中 override（返回 Int(-1) 而非原 builtin 的 String）
    let ctx_oid = _reg.new_stub_instance(&vm, "android/content/Context").unwrap();
    let mut refs = RefTable::new();
    let mut args = JniArgs::new();
    args.set_this(ctx_oid);
    let result = vm.classes.dispatch_call(&sig, &args, &mut refs).unwrap();
    assert_eq!(result, JValue::Int(-1), "Python-shim 风格 override 应覆盖 builtin");
}
