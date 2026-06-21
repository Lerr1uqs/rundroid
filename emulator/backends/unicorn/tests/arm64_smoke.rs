//! ARM64 smoke test：跑通最小执行路径。
//!
//! 构造一段在 guest 内存里执行 `mov x0, #42; ret` 的 stub，
//! 让 Unicorn 执行一条指令后停下，再读回 x0 验证等于 42。
//! 这条路径是后续 ELF 导出符号调用的最小前置条件。

use rundroid_backend::{Arm64Reg, Backend, MemPerms};
use rundroid_backend_unicorn::UnicornBackend;

// ARM64 机器码（小端序）：
//   mov x0, #42   ->  0xD2800540   movz x0, #42
//   ret           ->  0xD65F03C0
const STUB: [u8; 8] = [0x40, 0x05, 0x80, 0xD2, 0xC0, 0x03, 0x5F, 0xD6];

#[test]
fn executes_mov_x0_constant() {
    let engine = UnicornBackend::new().open(rundroid_core::Arch::Arm64).unwrap();

    let mut eng = engine;
    const CODE_ADDR: u64 = 0x10_000;
    const STACK_TOP: u64 = 0x80_010_000;

    eng.mem_map(CODE_ADDR, 0x1000, MemPerms::READ_EXEC).unwrap();
    eng.mem_map(0x80_000_000, 0x10_000, MemPerms::READ_WRITE).unwrap();

    eng.mem_write(CODE_ADDR, &STUB).unwrap();
    eng.reg_write(Arm64Reg::Sp, STACK_TOP).unwrap();
    eng.reg_write(Arm64Reg::Pc, CODE_ADDR).unwrap();

    // 只执行一条指令（mov），避免 ret 跳到未映射的 LR 触发异常。
    eng.emu_start(CODE_ADDR, None, None, Some(1)).unwrap();

    let x0 = eng.reg_read(Arm64Reg::X(0)).unwrap();
    assert_eq!(x0, 42, "mov x0, #42 应当让 x0 == 42");
}
