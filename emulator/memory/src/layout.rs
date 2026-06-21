//! guest 固定布局的地址计算。
//!
//! bootstrap 阶段只需要 stack 与 TLS 两类固定布局。
//! 地址选择遵循 Android bionic 的常规做法：
//! - 栈放在地址空间高端，向下生长
//! - TLS 区放在栈下方一点，避免与栈碰撞
//!
//! 这里只做"算地址"，不做映射；映射仍由 loader 调用 backend 完成。

/// 栈布局。bootstrap 阶段只描述"一整块连续栈区"，不做 guard page / 二级栈。
#[derive(Debug, Clone, Copy)]
pub struct StackLayout {
    /// 栈区起始地址（低端，向下生长的栈不会触及这里）。
    pub base: u64,
    /// 栈区大小（字节）。
    pub size: u64,
}

impl StackLayout {
    /// 栈顶地址，即初始 SP。
    ///
    /// ARM64 ABI 要求 SP 16 字节对齐；`base + size` 在常规 page-aligned 配置下天然 16 对齐，
    /// 但仍显式 round 一下，避免上层传非对齐 size。
    pub fn initial_sp(&self) -> u64 {
        let top = self.base.checked_add(self.size).expect("stack overflow");
        top & !0xFu64
    }
}

/// TLS 模板区布局。
///
/// 真正的 TLS 还涉及 TCB、TPIDR_EL0 指向等细节，
/// bootstrap smoke 阶段只预留一块连续区给静态 TLS 模板，足够多数导出函数调用。
#[derive(Debug, Clone, Copy)]
pub struct TlsLayout {
    pub base: u64,
    pub size: u64,
}

impl TlsLayout {
    /// TLS 区结束地址（exclusive）。
    pub fn end(&self) -> u64 {
        self.base.checked_add(self.size).expect("tls overflow")
    }
}

/// 根据地址空间顶与各自大小，规划一组不冲突的 stack / TLS 地址。
///
/// 返回的栈顶贴近 `address_space_top`，TLS 在其下方，中间留一页 guard。
/// bootstrap smoke 路径只需要这一组合理默认。
pub fn plan_default(
    address_space_top: u64,
    stack_size: u64,
    tls_size: u64,
) -> (StackLayout, TlsLayout) {
    // 栈：贴顶向下分配一整块。
    // 注意：栈本身是 [stack_base, stack_base + stack_size)，初始 SP = 顶端。
    let stack_base = address_space_top
        .checked_sub(stack_size)
        .expect("stack size exceeds address space");
    let stack = StackLayout {
        base: stack_base,
        size: stack_size,
    };

    // TLS：在栈底下方留一页（4KiB）guard，再放 TLS 区。
    const GUARD: u64 = 0x1000;
    let tls_end = stack_base.checked_sub(GUARD).expect("guard underflow");
    let tls_base = tls_end.checked_sub(tls_size).expect("tls underflow");
    let tls = TlsLayout {
        base: tls_base,
        size: tls_size,
    };

    (stack, tls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_top_is_aligned_and_below_as_top() {
        let (stack, _) = plan_default(0xFFFF_FFFF_F000_0000, 64 * 1024, 4 * 1024);
        let sp = stack.initial_sp();
        assert_eq!(sp % 16, 0);
        assert!(sp <= 0xFFFF_FFFF_F000_0000);
    }

    #[test]
    fn tls_sits_below_stack_with_guard() {
        let top = 0xFFFF_FFFF_F000_0000;
        let (stack, tls) = plan_default(top, 64 * 1024, 4 * 1024);
        // TLS 末端 + guard 应当 == stack 起点。
        assert_eq!(tls.end() + 0x1000, stack.base);
    }
}
