//! 顶层 case 执行入口。
//!
//! 把 manifest → resource → runtime → call → artifact 串成一条函数。

use crate::artifacts::{
    backend_info_from, Artifacts, BackendInfo, CallOutcome, CaseResult, Outcome,
};
use crate::manifest::{CaseCall, CaseManifest};
use crate::resource::resolve_resource;
use crate::runtime::GuestRuntime;
use rundroid_core::{Arch, BackendKind, RuntimeConfig};
use rundroid_telemetry::TelemetryMode;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaseRunError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest parse error: {0}")]
    Manifest(String),
    #[error("resource error: {0}")]
    Resource(#[from] crate::resource::ResourceError),
    #[error("runtime assembly error: {0}")]
    Assembly(#[from] crate::runtime::RuntimeAssemblyError),
    #[error("backend error during call: {0}")]
    Backend(String),
    #[error("entry symbol `{0}` not resolvable in any loaded module")]
    EntryUnresolved(String),
}

/// 执行一个 case，返回三个 artifact。
///
/// `resources_root` 是 `resources/` 目录绝对路径；
/// 成功执行后调用方负责把 [`Artifacts`] 落盘。
pub fn run_case(
    manifest: &CaseManifest,
    resources_root: &Path,
) -> Result<Artifacts, CaseRunError> {
    // manifest 字段校验：bootstrap 只支持 arm64 + unicorn，
    // 不符时直接 fail-fast，而不是静默用默认实现跑（review finding 4）。
    if manifest.arch != "arm64" {
        return Err(CaseRunError::Manifest(format!(
            "arch `{}` not supported by bootstrap (only `arm64`)",
            manifest.arch
        )));
    }
    if manifest.backend != "unicorn" {
        return Err(CaseRunError::Manifest(format!(
            "backend `{}` not supported by bootstrap (only `unicorn`)",
            manifest.backend
        )));
    }
    let telemetry = match manifest.telemetry.as_str() {
        "disabled" => TelemetryMode::Disabled,
        "full" => TelemetryMode::Full,
        _ => TelemetryMode::EventsOnly,
    };
    let mut config = RuntimeConfig::bootstrap();
    config.telemetry = telemetry;

    let mut rt = GuestRuntime::assemble(config)?;

    // 把 manifest.seed 真正注入 LinuxRuntime 的 PRNG，
    // 否则 case 写 seed=42 看起来像"确定性验证"但不生效。
    if let Some(seed) = manifest.seed {
        rt.seed_rng(seed);
    }

    // 资源解析：把 module URI 翻成字节流。
    let module_bytes = resolve_resource(&manifest.module, resources_root)?;
    let module_name = manifest
        .module
        .rsplit('/')
        .next()
        .unwrap_or("module.so")
        .to_string();

    // DT_NEEDED 依赖按 "<root pack>/<soname>" 解析。
    // 例如 root = "resource:smoke/build/libfoo.so"，DT_NEEDED=libbar.so
    // → "resource:smoke/libbar.so"（同 pack 下查找）。
    // 这与 Android bionic 的 rootfs 挂载语义一致。
    let root_pack = manifest
        .module
        .strip_prefix("resource:")
        .and_then(|s| s.split_once('/'))
        .map(|(p, _)| p.to_string())
        .unwrap_or_else(|| "smoke".to_string());
    let resources_root_owned = resources_root.to_path_buf();
    let mut dep_provider = move |soname: &str| -> Option<Vec<u8>> {
        let uri = format!("resource:{root_pack}/{soname}");
        resolve_resource(&uri, &resources_root_owned).ok()
    };

    let assemble_result = rt.load_and_link(&module_name, &module_bytes, &mut dep_provider);

    let mut calls_out: Vec<CallOutcome> = Vec::new();
    let mut error: Option<String> = None;
    let mut outcome = Outcome::Pass;

    if let Err(e) = assemble_result {
        // 装载/链接失败：仍产出 artifact，但标记失败并停止。
        error = Some(format!("assembly failed: {e}"));
        outcome = Outcome::Fail;
    } else {
        // 逐 call 执行。
        for call in &manifest.call {
            match run_one_call(&mut rt, call) {
                Ok(returned) => {
                    let matched = match (returned, call.expect_return) {
                        (Some(r), Some(exp)) => r as i64 == exp,
                        _ => true,
                    };
                    if !matched {
                        outcome = Outcome::Fail;
                    }
                    calls_out.push(CallOutcome {
                        symbol: call.symbol.clone(),
                        args: call.args.clone(),
                        returned: returned.map(|v| v as i64),
                        expected: call.expect_return,
                        matched,
                    });
                }
                Err(e) => {
                    error = Some(format!("call `{}` failed: {e}", call.symbol));
                    outcome = Outcome::Fail;
                    calls_out.push(CallOutcome {
                        symbol: call.symbol.clone(),
                        args: call.args.clone(),
                        returned: None,
                        expected: call.expect_return,
                        matched: false,
                    });
                    break;
                }
            }
        }
    }

    // 从 LinuxRuntime 取出 stdout / exit_code 用于 artifact。
    let (stdout, exit_code) = {
        let linux = rt.linux();
        (linux.stdout.clone(), linux.exit_code)
    };
    let stdout_preview = String::from_utf8_lossy(&stdout[..stdout.len().min(256)]).into_owned();

    let result = CaseResult {
        case: manifest.name.clone(),
        outcome,
        calls: calls_out,
        stdout_len: stdout.len(),
        stdout_utf8_preview: stdout_preview,
        error,
    };
    let backend: BackendInfo = backend_info_from(
        Arch::Arm64,
        BackendKind::Unicorn,
        &rt.regions,
        exit_code,
    );
    let events = rt.take_events();
    Ok(Artifacts {
        result,
        backend,
        events,
    })
}

fn run_one_call(
    rt: &mut GuestRuntime,
    call: &CaseCall,
) -> Result<Option<u64>, CaseRunError> {
    let entry = rt
        .resolve_symbol(&call.symbol)
        .ok_or_else(|| CaseRunError::EntryUnresolved(call.symbol.clone()))?;
    let returned = rt
        .call_export(entry, &call.args)
        .map_err(|e| CaseRunError::Backend(e.to_string()))?;
    Ok(Some(returned))
}
