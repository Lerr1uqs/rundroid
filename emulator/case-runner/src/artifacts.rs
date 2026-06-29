//! case 执行后的 artifact 输出。
//!
//! 三个 artifact：
//! - `result.json`：case 的总体结果（pass / fail、call 返回值、stdout 摘要）
//! - `backend.json`：backend 层面的信息（arch、backend kind、mapped regions）
//! - `events.jsonl`：telemetry 事件流，每行一条 JSON

use crate::runtime::EventRecord;
use rundroid_core::{Arch, BackendKind};
use rundroid_memory::MemoryRegion;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseResult {
    pub case: String,
    pub outcome: Outcome,
    pub calls: Vec<CallOutcome>,
    pub stdout_len: usize,
    pub stdout_utf8_preview: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    Fail,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallOutcome {
    pub symbol: String,
    pub args: Vec<u64>,
    pub returned: Option<i64>,
    pub expected: Option<i64>,
    pub matched: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    pub arch: String,
    pub backend: String,
    pub regions: Vec<RegionEntry>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionEntry {
    pub addr: u64,
    pub size: u64,
    pub origin: String,
}

/// 三个 artifact 的打包容器，便于一次性写入。
#[derive(Debug)]
pub struct Artifacts {
    pub result: CaseResult,
    pub backend: BackendInfo,
    pub events: Vec<EventRecord>,
}

impl Artifacts {
    /// 把三个 artifact 写到 `out_dir` 下。
    ///
    /// `events.jsonl` 是每行一条 JSON；其余两个是单文件 JSON。
    pub fn write_to(&self, out_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(out_dir)?;
        std::fs::write(
            out_dir.join("result.json"),
            serde_json::to_string_pretty(&self.result)?,
        )?;
        std::fs::write(
            out_dir.join("backend.json"),
            serde_json::to_string_pretty(&self.backend)?,
        )?;
        let mut lines = String::new();
        for ev in &self.events {
            lines.push_str(&serde_json::to_string(ev)?);
            lines.push('\n');
        }
        std::fs::write(out_dir.join("events.jsonl"), lines)?;
        Ok(())
    }
}

/// 从 runtime 状态构造 [`BackendInfo`]。
pub fn backend_info_from(
    arch: Arch,
    backend: BackendKind,
    regions: &[MemoryRegion],
    exit_code: Option<i32>,
) -> BackendInfo {
    let regions: Vec<RegionEntry> = regions
        .iter()
        .map(|r| RegionEntry {
            addr: r.addr,
            size: r.size,
            origin: format!("{:?}", r.usage).to_lowercase(),
        })
        .collect();
    BackendInfo {
        arch: format!("{:?}", arch).to_lowercase(),
        backend: format!("{:?}", backend).to_lowercase(),
        regions,
        exit_code,
    }
}
