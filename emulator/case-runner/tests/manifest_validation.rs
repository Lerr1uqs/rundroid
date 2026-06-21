//! manifest 字段生效路径测试（review finding 4）。
//!
//! 验证：
//! - 不支持的 arch / backend 在 run_case 入口直接 fail-fast
//! - 这条路径不需要装载任何 .so，纯 manifest 校验

use rundroid_case_runner::{run_case, CaseManifest};

#[test]
fn rejects_unsupported_arch() {
    let manifest = CaseManifest {
        name: "bad-arch".to_string(),
        arch: "x86_64".to_string(),
        backend: "unicorn".to_string(),
        module: "resource:smoke/build/libsmoke.so".to_string(),
        entry: None,
        seed: None,
        call: vec![],
        telemetry: "events_only".to_string(),
    };
    let err = run_case(&manifest, std::path::Path::new("resources")).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("arch"), "err should mention arch: {msg}");
}

#[test]
fn rejects_unsupported_backend() {
    let manifest = CaseManifest {
        name: "bad-backend".to_string(),
        arch: "arm64".to_string(),
        backend: "qemu".to_string(),
        module: "resource:smoke/build/libsmoke.so".to_string(),
        entry: None,
        seed: None,
        call: vec![],
        telemetry: "events_only".to_string(),
    };
    let err = run_case(&manifest, std::path::Path::new("resources")).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("backend"), "err should mention backend: {msg}");
}
