//! `rundroid-cli`
//!
//! 命令行入口。bootstrap 阶段支持的子命令：
//! - `rundroid case <case.toml>`：执行一个 case，artifact 写到 `./artifacts/<case-name>/`
//! - `rundroid list <cases-dir>`：列出目录下所有 case
//!
//! 用法示例：
//! ```bash
//! rundroid case tests/cases/01-pure-export-call/case.toml
//! ```

use rundroid_case_runner::{run_case, CaseManifest};
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage(&args[0]);
        std::process::exit(2);
    }
    let cmd = args[1].as_str();
    let result = match cmd {
        "case" => run_case_cmd(&args[2..]),
        "list" => list_cases_cmd(&args[2..]),
        "help" | "-h" | "--help" => {
            usage(&args[0]);
            Ok(())
        }
        other => {
            eprintln!("unknown command: {other}");
            usage(&args[0]);
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn usage(prog: &str) {
    eprintln!("usage:");
    eprintln!("  {prog} case <case.toml>     run a single case");
    eprintln!("  {prog} list  <cases-dir>    list cases in directory");
}

fn run_case_cmd(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        return Err("missing case.toml path".into());
    }
    let case_path = Path::new(&args[0]);
    let manifest = CaseManifest::load(case_path)?;

    // resources_root：从仓库根算。简单约定 = case.toml 所在仓库根下的 resources/。
    // 这里用 cwd 上溯 3 层找 resources/，避免硬编码绝对路径。
    let resources_root = find_resources_root(case_path)?;

    let artifacts = run_case(&manifest, &resources_root)?;
    let out_dir = artifacts_out_dir(&manifest.name);
    artifacts.write_to(&out_dir)?;

    println!(
        "case `{}` outcome={:?} -> {}",
        manifest.name,
        artifacts.result.outcome,
        out_dir.display()
    );
    Ok(())
}

fn list_cases_cmd(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        return Err("missing cases dir".into());
    }
    let dir = Path::new(&args[0]);
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let case_toml = entry.path().join("case.toml");
        if case_toml.is_file() {
            if let Ok(m) = CaseManifest::load(&case_toml) {
                println!("{}", m.name);
            }
        }
    }
    Ok(())
}

/// 把 artifact 写到 `./artifacts/<case-name>/`。
fn artifacts_out_dir(name: &str) -> PathBuf {
    PathBuf::from("./artifacts").join(name)
}

/// 从给定路径往上找包含 `resources/` 目录的祖先。
fn find_resources_root(start: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut cur = start.canonicalize()?;
    loop {
        if cur.join("resources").is_dir() {
            return Ok(cur.join("resources"));
        }
        if !cur.pop() {
            return Err("could not locate resources/ from case path".into());
        }
    }
}
