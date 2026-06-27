//! JNI 函数表端到端验证测试。
//!
//! 加载用 NDK 编译的 `libjnitest.so`，
//! 通过 Rust 侧注册 framework stub class，
//! guest 侧通过 `(*env)->FindClass / GetStaticMethodID / CallStaticIntMethod`
//! 调用 JNI 函数表，验证完整 dispatch 链路。

use rundroid_case_runner::GuestRuntime;
use rundroid_core::RuntimeConfig;
use rundroid_jni::{
    AndroidVM, ClassId, JClassDef, JValue, MethodImpl, RustFieldHandler, SharedField,
};
use std::sync::{Arc, Mutex};

/// 构造一个已注册 test/JniTest + test/Counter 的 AndroidVM。
fn build_test_vm() -> (Arc<Mutex<AndroidVM>>,) {
    let mut vm = AndroidVM::new();

    // ===== test/JniTest =====
    // static method: nativePing()I → 返回 42 (0x2A)
    // instance method: doNothing()V → 无操作
    {
        let cls_name = "test/JniTest";
        let sig_ping = rundroid_jni::MethodSig {
            class: cls_name.into(),
            name: "nativePing".into(),
            args: vec![],
            ret: rundroid_jni::JType::Int,
        };
        let sig_nothing = rundroid_jni::MethodSig {
            class: cls_name.into(),
            name: "doNothing".into(),
            args: vec![],
            ret: rundroid_jni::JType::Void,
        };
        let sig_init = rundroid_jni::MethodSig {
            class: cls_name.into(),
            name: "<init>".into(),
            args: vec![],
            ret: rundroid_jni::JType::Void,
        };

        let mut class_def = JClassDef::new(ClassId(0), cls_name.into());
        class_def
            .add_method(sig_ping, true, MethodImpl::RustNative(Arc::new(|_args| {
                Ok(JValue::Int(42))
            })))
            .unwrap();
        class_def
            .add_method(sig_nothing, false, MethodImpl::RustNative(Arc::new(|_args| {
                Ok(JValue::Void)
            })))
            .unwrap();
        class_def
            .add_method(sig_init, false, MethodImpl::RustNative(Arc::new(|_args| {
                Ok(JValue::Void)
            })))
            .unwrap();
        vm.classes.register_class(class_def).unwrap();
    }

    // ===== test/Counter =====
    // instance field: count:I（通过 Arc<SharedField> 共享）
    // instance method: getAndIncrement()I → 返回当前值，然后 +1
    // instance method: <init>()V → 初始化 count=100
    {
        let cls_name = "test/Counter";
        let counter: Arc<SharedField> = Arc::new(SharedField::new(JValue::Int(100)));

        let counter_for_get = counter.clone();
        let sig_get = rundroid_jni::MethodSig {
            class: cls_name.into(),
            name: "getAndIncrement".into(),
            args: vec![],
            ret: rundroid_jni::JType::Int,
        };
        let sig_init = rundroid_jni::MethodSig {
            class: cls_name.into(),
            name: "<init>".into(),
            args: vec![],
            ret: rundroid_jni::JType::Void,
        };

        let mut class_def = JClassDef::new(ClassId(0), cls_name.into());
        class_def
            .add_method(
                sig_get,
                false,
                MethodImpl::RustNative(Arc::new(move |_args| {
                    let current = counter_for_get.get();
                    if let JValue::Int(n) = current {
                        counter_for_get.set(JValue::Int(n + 1)).ok();
                        Ok(JValue::Int(n))
                    } else {
                        Err(rundroid_jni::JniError::Internal("类型错误".into()))
                    }
                })),
            )
            .unwrap();
        class_def
            .add_method(sig_init, false, MethodImpl::RustNative(Arc::new(|_args| {
                Ok(JValue::Void)
            })))
            .unwrap();
        vm.classes.register_class(class_def).unwrap();
    }

    (Arc::new(Mutex::new(vm)),)
}

/// 辅助：从 workspace root 读取 fixture .so 字节。
fn read_fixture() -> Vec<u8> {
    // 从 case-runner crate 目录向上走到 workspace root
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../resources/jnitest/build/libjnitest.so");
    std::fs::read(&path)
        .unwrap_or_else(|e| panic!("无法读取 fixture: {path:?}: {e}"))
}

#[test]
fn test_get_version_via_jni() {
    let (vm,) = build_test_vm();
    let config = RuntimeConfig::default();
    let mut rt = GuestRuntime::assemble(config).unwrap();
    rt.init_jni(vm).unwrap();

    let bytes = read_fixture();
    rt.load_and_link("libjnitest.so", &bytes, &mut |_| None).unwrap();

    let entry = rt.resolve_symbol("test_get_version")
        .expect("test_get_version 符号未找到");

    let env_ptr = rt.jni_env_pointer.unwrap();
    let ret = rt.call_export(entry, &[env_ptr]).unwrap();

    assert_eq!(ret, 0, "test_get_version 应返回 0（成功），实际返回 {ret}");
}

#[test]
fn test_find_class_via_jni() {
    let (vm,) = build_test_vm();
    let config = RuntimeConfig::default();
    let mut rt = GuestRuntime::assemble(config).unwrap();
    rt.init_jni(vm).unwrap();

    let bytes = read_fixture();
    let _module_id = rt.load_and_link("libjnitest.so", &bytes, &mut |_| None).unwrap();

    // 查找 test_find_class 导出
    let entry = rt.resolve_symbol("test_find_class")
        .expect("test_find_class 符号未找到");

    // 调用 test_find_class(JNIEnv* env)
    let env_ptr = rt.jni_env_pointer.unwrap();
    let ret = rt.call_export(entry, &[env_ptr]).unwrap();

    // test_find_class 返回 0 表示成功
    assert_eq!(ret, 0, "test_find_class 应返回 0（成功），实际返回 {ret}");
}

#[test]
fn test_get_static_method_id_via_jni() {
    let (vm,) = build_test_vm();
    let config = RuntimeConfig::default();
    let mut rt = GuestRuntime::assemble(config).unwrap();
    rt.init_jni(vm).unwrap();

    let bytes = read_fixture();
    rt.load_and_link("libjnitest.so", &bytes, &mut |_| None).unwrap();

    let entry = rt.resolve_symbol("test_get_static_method_id")
        .expect("test_get_static_method_id 符号未找到");

    let env_ptr = rt.jni_env_pointer.unwrap();
    let ret = rt.call_export(entry, &[env_ptr]).unwrap();

    assert_eq!(ret, 0, "test_get_static_method_id 应返回 0，实际返回 {ret}");
}

#[test]
fn test_call_static_int_method_via_jni() {
    let (vm,) = build_test_vm();
    let config = RuntimeConfig::default();
    let mut rt = GuestRuntime::assemble(config).unwrap();
    rt.init_jni(vm).unwrap();

    let bytes = read_fixture();
    rt.load_and_link("libjnitest.so", &bytes, &mut |_| None).unwrap();

    let entry = rt.resolve_symbol("test_call_static_int_method")
        .expect("test_call_static_int_method 符号未找到");

    let env_ptr = rt.jni_env_pointer.unwrap();
    let ret = rt.call_export(entry, &[env_ptr]).unwrap();

    // Rust 侧 nativePing 返回 42 (0x2A)，guest 原样返回
    assert_eq!(ret, 42, "test_call_static_int_method 应返回 42，实际返回 {ret}");
}

#[test]
fn test_call_void_method_via_jni() {
    let (vm,) = build_test_vm();
    let config = RuntimeConfig::default();
    let mut rt = GuestRuntime::assemble(config).unwrap();
    rt.init_jni(vm).unwrap();

    let bytes = read_fixture();
    rt.load_and_link("libjnitest.so", &bytes, &mut |_| None).unwrap();

    let entry = rt.resolve_symbol("test_call_void_method")
        .expect("test_call_void_method 符号未找到");

    let env_ptr = rt.jni_env_pointer.unwrap();
    let ret = rt.call_export(entry, &[env_ptr]).unwrap();

    assert_eq!(ret, 0, "test_call_void_method 应返回 0，实际返回 {ret}");
}

#[test]
fn test_jni_full_flow_counter() {
    let (vm,) = build_test_vm();
    let config = RuntimeConfig::default();
    let mut rt = GuestRuntime::assemble(config).unwrap();
    rt.init_jni(vm).unwrap();

    let bytes = read_fixture();
    rt.load_and_link("libjnitest.so", &bytes, &mut |_| None).unwrap();

    let entry = rt.resolve_symbol("jni_full_flow")
        .expect("jni_full_flow 符号未找到");

    let env_ptr = rt.jni_env_pointer.unwrap();
    let ret = rt.call_export(entry, &[env_ptr]).unwrap();

    // jni_full_flow 返回 (r1 << 16) | r2 = (100 << 16) | 101
    let expected = (100u64 << 16) | 101;
    assert_eq!(
        ret, expected,
        "jni_full_flow 应返回 0x{:X}（100<<16|101），实际返回 0x{ret:X}",
        expected
    );
}

/// 验证 guest 通过 JavaVM invoke table 调 GetEnv 拿到有效 JNIEnv*。
///
/// guest 侧 `test_get_env_via_javavm(JavaVM*)`：
/// 1. `(*vm)->GetEnv(vm, &env, JNI_VERSION_1_6)` → 期望 JNI_OK + env 非空
/// 2. 用返回的 env 调 GetVersion / FindClass 验证 env 对当前 VM 有效
///
/// 覆盖 jni-abi-surfaces 的 JavaVMABI invoke table 主线（GetEnv 入口）。
#[test]
fn test_get_env_via_javavm() {
    let (vm,) = build_test_vm();
    let config = RuntimeConfig::default();
    let mut rt = GuestRuntime::assemble(config).unwrap();
    rt.init_jni(vm).unwrap();

    let bytes = read_fixture();
    rt.load_and_link("libjnitest.so", &bytes, &mut |_| None).unwrap();

    let entry = rt.resolve_symbol("test_get_env_via_javavm")
        .expect("test_get_env_via_javavm 符号未找到");

    // 传入 JavaVM* 作为 x0，guest 通过 invoke table 调 GetEnv
    let java_vm_ptr = rt.java_vm_pointer.unwrap();
    let ret = rt.call_export(entry, &[java_vm_ptr]).unwrap();

    assert_eq!(ret, 0, "test_get_env_via_javavm 应返回 0（成功），实际返回 {ret}");
}

/// 验证 JavaVM invoke table 的 AttachCurrentThread / DetachCurrentThread 端到端。
///
/// guest 侧 `test_attach_via_javavm(JavaVM*)`：
/// 1. `(*vm)->AttachCurrentThread(vm, &env, NULL)` → JNI_OK + env 非空
/// 2. 用 attach 返回的 env 调 FindClass 验证 env 有效
/// 3. `(*vm)->DetachCurrentThread(vm)` → JNI_OK
///
/// 覆盖 jni-abi-surfaces task 5 三个 invoke 入口的另两个（GetEnv 见上）。
#[test]
fn test_attach_detach_via_javavm() {
    let (vm,) = build_test_vm();
    let config = RuntimeConfig::default();
    let mut rt = GuestRuntime::assemble(config).unwrap();
    rt.init_jni(vm).unwrap();

    let bytes = read_fixture();
    rt.load_and_link("libjnitest.so", &bytes, &mut |_| None).unwrap();

    let entry = rt.resolve_symbol("test_attach_via_javavm")
        .expect("test_attach_via_javavm 符号未找到");

    let java_vm_ptr = rt.java_vm_pointer.unwrap();
    let ret = rt.call_export(entry, &[java_vm_ptr]).unwrap();

    assert_eq!(ret, 0, "test_attach_via_javavm 应返回 0（成功），实际返回 {ret}");
}
