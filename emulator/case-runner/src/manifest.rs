//! `case.toml` 解析。
//!
//! 最小 schema：
//! ```toml
//! name = "urandom-read"
//! arch = "arm64"
//! backend = "unicorn"
//! module = "resource:smoke/libfoo.so"
//! entry = "Java_pkg_smoke_read"
//! seed = 42           # 可选，urandom PRNG 种子
//!
//! [[call]]
//! symbol = "Java_pkg_smoke_read"
//! args = [0, 0x1000]
//! expect_return = 0
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseManifest {
    pub name: String,
    #[serde(default = "default_arch")]
    pub arch: String,
    #[serde(default = "default_backend")]
    pub backend: String,
    /// 模块的 resource URI（例如 `resource:smoke/libfoo.so`）。
    pub module: String,
    /// 默认入口符号；可被 `[[call]]` 覆盖。
    #[serde(default)]
    pub entry: Option<String>,
    /// urandom PRNG 种子；省略时使用内置默认。
    #[serde(default)]
    pub seed: Option<u64>,
    /// 要执行的调用列表。空时仅装载链接、不调用。
    #[serde(default)]
    pub call: Vec<CaseCall>,
    /// telemetry mode：`disabled` / `events_only` / `full`。
    #[serde(default = "default_telemetry")]
    pub telemetry: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseCall {
    pub symbol: String,
    #[serde(default)]
    pub args: Vec<u64>,
    #[serde(default)]
    pub expect_return: Option<i64>,
}

fn default_arch() -> String {
    "arm64".to_string()
}
fn default_backend() -> String {
    "unicorn".to_string()
}
fn default_telemetry() -> String {
    "events_only".to_string()
}

impl CaseManifest {
    /// 从 TOML 文本解析。
    pub fn parse_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// 从文件读取并解析。
    pub fn load(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let text = std::fs::read_to_string(path)?;
        Self::parse_toml(&text).map_err(Into::into)
    }
}
