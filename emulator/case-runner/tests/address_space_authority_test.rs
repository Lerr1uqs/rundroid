//! guest 地址空间 authority 回归测试。
//!
//! 确认 case-runner 的 scratch / JNI / ELF 装载都进入同一份
//! [`MemoryAddressSpace`]，并且固定布局冲突会立即报 overlap。

use rundroid_case_runner::{GuestRuntime, RuntimeAssemblyError};
use rundroid_core::RuntimeConfig;
use rundroid_jni::AndroidVM;
use rundroid_memory::{MemoryError, MemoryUsage};
use std::path::Path;
use std::sync::{Arc, Mutex};

fn read_fixture(relative: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("无法读取 fixture {path:?}: {e}"))
}

#[test]
fn multiple_consumers_share_one_address_space_truth() {
    let mut rt = GuestRuntime::assemble(RuntimeConfig::default()).unwrap();
    let vm = Arc::new(Mutex::new(AndroidVM::new()));
    rt.init_jni(vm).unwrap();

    let smoke = read_fixture("../../resources/smoke/build/libsmoke.so");
    let jnitest = read_fixture("../../resources/jnitest/build/libjnitest.so");
    rt.load_and_link("libsmoke.so", &smoke, &mut |_| None).unwrap();
    rt.load_and_link("libjnitest.so", &jnitest, &mut |_| None).unwrap();

    let mut modules: Vec<(u64, u64)> = rt
        .graph
        .modules
        .values()
        .map(|module| (module.base, module.size))
        .collect();
    modules.sort_by_key(|(base, _)| *base);
    assert!(modules.len() >= 2, "应至少存在两个已装载模块");
    for pair in modules.windows(2) {
        let (left_base, left_size) = pair[0];
        let (right_base, _) = pair[1];
        assert!(
            left_base + left_size <= right_base,
            "两个模块在共享 authority 中不应重叠: left={left_base:#x}+{left_size:#x}, right={right_base:#x}"
        );
    }

    let env_ptr = rt.jni_env_pointer.expect("init_jni 后必须有 JNIEnv 指针");
    let space = rt.address_space.lock().unwrap();
    assert_eq!(space.find(0x800_000).unwrap().usage, MemoryUsage::Scratch);
    assert_eq!(space.find(env_ptr).unwrap().usage, MemoryUsage::JNIEnv);
    for (base, _size) in modules {
        assert_eq!(space.find(base).unwrap().usage, MemoryUsage::ELFImage);
    }
}

#[test]
fn repeated_fixed_layout_mapping_fails_with_overlap() {
    let mut rt = GuestRuntime::assemble(RuntimeConfig::default()).unwrap();
    let vm = Arc::new(Mutex::new(AndroidVM::new()));
    rt.init_jni(Arc::clone(&vm)).unwrap();

    let err = rt.init_jni(vm).unwrap_err();
    match err {
        RuntimeAssemblyError::Memory(MemoryError::Overlap { .. }) => {}
        other => panic!("重复映射固定 JNI 布局应当先命中 overlap，实际: {other:?}"),
    }
}
