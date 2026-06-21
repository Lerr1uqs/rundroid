//! 用真实 ARM64 .so 验证 parser。
//!
//! fixture 来自 unidbg 的 example_binaries；如果该路径不存在则 skip。
//! 不把 .so 提交进 rundroid resources，避免仓库膨胀；
//! 后续 testing-harness task 会引入 `resource:` URI 体系正式管理 fixture。

use rundroid_elf_parser::{ElfCrateParser, ElfParser, ParseInput};

const FIXTURE: &str =
    "F:/reverse-workspace/unidbg/unidbg-android/src/test/resources/example_binaries/arm64-v8a/libjnidispatch.so";

#[test]
fn parses_real_arm64_so() {
    let bytes = match std::fs::read(FIXTURE) {
        Ok(b) => b,
        Err(_) => {
            eprintln!("skipping: fixture not present at {FIXTURE}");
            return;
        }
    };

    let parsed = ElfCrateParser::new()
        .parse(ParseInput::new("libjnidispatch.so", &bytes))
        .expect("parse should succeed for a valid ARM64 .so");

    // 身份：bootstrap 要求 ELF64 / little-endian / AArch64。
    assert!(parsed.file.is_64bit);
    assert!(parsed.file.little_endian);
    assert_eq!(parsed.file.machine, 183); // EM_AARCH64

    // PT_LOAD 段按 vaddr 升序、至少一段。
    assert!(!parsed.segments.is_empty());
    let mut prev = None;
    for seg in &parsed.segments {
        if let Some(p) = prev {
            assert!(seg.vaddr >= p, "segments must be sorted by vaddr");
        }
        prev = Some(seg.vaddr);
    }

    // dynsym 至少有 NULL 符号（index 0）。
    assert!(!parsed.symbols.is_empty());

    // 至少出现一个重定位（.so 几乎不可能没有 RELA）。
    assert!(
        !parsed.relocations.is_empty(),
        "expected at least one relocation in libjnidispatch.so"
    );

    // 模块名透传。
    assert_eq!(parsed.module_name, "libjnidispatch.so");
}

#[test]
fn rejects_non_elf_bytes() {
    let bytes = [0u8; 64];
    let err = ElfCrateParser::new()
        .parse(ParseInput::new("garbage", &bytes))
        .unwrap_err();
    // 任何 ElfParseError 变体都可接受，关键是不 panic。
    let _ = err;
}
