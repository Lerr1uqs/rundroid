//! PRNG 语义方法（kernel 域）。
//!
//! `impl LinuxRuntime` 的 getrandom：xorshift64 PRNG，推进共享 `rng_seed`，
//! 返回随机字节（syscall 层负责回写到目标侧缓冲）。

use super::LinuxRuntime;

impl LinuxRuntime {
    /// getrandom：用 xorshift64 PRNG 产生 `count` 字节。
    ///
    /// 推进共享 `rng_seed`（与 builtin urandom factory 共用同一种子源）。
    /// 返回字节序列——目标侧回写由 syscall 层经
    /// [`MemoryBridge::write`](crate::memory_bridge::MemoryBridge::write) 落地。
    pub fn getrandom_bytes(&mut self, count: usize) -> Vec<u8> {
        let mut rng = *self.rng_seed.lock().unwrap();
        let mut buf = Vec::with_capacity(count);
        for _ in 0..count {
            // xorshift64：三轮位移异或，保证非零种子的周期与分布。
            let mut x = rng;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            rng = x;
            buf.push((x & 0xFF) as u8);
        }
        *self.rng_seed.lock().unwrap() = rng;
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// getrandom_bytes 对相同种子确定性可复现。
    #[test]
    fn getrandom_bytes_is_deterministic_for_same_seed() {
        let mut rt = LinuxRuntime::new();
        rt.seed_rng(0x1234);
        let a = rt.getrandom_bytes(16);

        rt.seed_rng(0x1234);
        let b = rt.getrandom_bytes(16);

        assert_eq!(a, b, "相同种子应产生相同字节序列");
        assert_eq!(a.len(), 16);
    }

    /// getrandom_bytes 对合理种子产生非零字节（PRNG 真正运转）。
    #[test]
    fn getrandom_bytes_produces_nonzero_bytes() {
        let mut rt = LinuxRuntime::new();
        rt.seed_rng(0xABCD_1234);
        let bytes = rt.getrandom_bytes(32);
        assert!(
            bytes.iter().any(|b| *b != 0),
            "32 字节中应至少有一个非零字节，实际: {bytes:?}"
        );
    }

    /// getrandom_bytes(0) 返回空且不 panic。
    #[test]
    fn getrandom_bytes_zero_length_is_empty() {
        let mut rt = LinuxRuntime::new();
        assert!(rt.getrandom_bytes(0).is_empty());
    }
}
