//! Android packed relocation (SHT_ANDROID_RELA / SHT_ANDROID_REL) 解码。
//!
//! Android 为减小 .so 体积把 RELA 段压成 SLEB128 delta 编码的格式，
//! section magic = `APS2`。bionic 的 `unpacker` 是事实标准实现，
//! 这里按同样算法还原成标准 RELA 记录。
//!
//! 参考实现：bionic/linker/elf_reloc_iterators.h 中的 `converted_rel_t` / `unpacker`。
//!
//! 为什么放在 parser crate：
//! spec 要求 parser 把 REL / RELA / Android packed 全部归一化成统一记录流，
//! linker 不应该感知 packed encoding。这层就是归一化的发生地。

use crate::error::ElfParseError;
use crate::model::{RelocationKind, RelocationRecord};

/// Android packed relocation magic。
const APS2_MAGIC: &[u8; 4] = b"APS2";

/// group_flags 取值（来自 bionic）。
const GROUPED_BY_OFFSET_DELTA: u64 = 1;
const GROUPED_BY_INFO: u64 = 2;
const GROUP_HAS_ADDEND: u64 = 4;
const GROUPED_BY_ADDEND: u64 = 8;

/// 把一个 Android packed RELA section 的字节解码成 [`RelocationRecord`] 列表。
///
/// `is_rela_section`：true = SHT_ANDROID_RELA（带 addend），false = SHT_ANDROID_REL（无 addend）。
/// 当前 AArch64 .so 上几乎只见 RELA 变体；REL 路径同样支持但默认 addend=0。
pub fn decode_packed(
    bytes: &[u8],
    is_rela_section: bool,
) -> Result<Vec<RelocationRecord>, ElfParseError> {
    if bytes.len() < 4 {
        return Err(ElfParseError::Truncated("packed reloc section too short"));
    }
    if &bytes[0..4] != APS2_MAGIC {
        // 不是 APS2 段：不在这里报错，让调用方继续按 REL/RELA 处理。
        return Err(ElfParseError::MalformedDynamic(
            "packed reloc section missing APS2 magic",
        ));
    }

    let mut cur = SlebCursor::new(&bytes[4..]);

    let reloc_count: u64 = cur.read_sleb()? as u64;
    let _offset_to_first: u64 = cur.read_sleb()? as u64; // 已弃用，bionic 也不读，保留兼容

    let mut out = Vec::with_capacity(reloc_count as usize);
    let mut offset: u64 = 0;
    let mut addend: i64 = 0;

    while (out.len() as u64) < reloc_count {
        let group_size: i64 = cur.read_sleb()?;
        let group_flags: u64 = cur.read_sleb()? as u64;

        if group_size < 0 {
            // bionic 用 -1 标记"按相反方向"，bootstrap 不支持，直接报错。
            return Err(ElfParseError::MalformedDynamic(
                "packed reloc: negative group_size not supported",
            ));
        }
        let group_size = group_size as u64;

        // 整组的"组级"常量（按 flag 决定是否在头里读取）。
        let grouped_offset_delta =
            group_flags & GROUPED_BY_OFFSET_DELTA == GROUPED_BY_OFFSET_DELTA;
        let grouped_info = group_flags & GROUPED_BY_INFO == GROUPED_BY_INFO;
        let group_has_addend = group_flags & GROUP_HAS_ADDEND == GROUP_HAS_ADDEND;
        let grouped_addend = group_flags & GROUPED_BY_ADDEND == GROUPED_BY_ADDEND;

        let mut group_offset_delta: u64 = 0;
        if grouped_offset_delta {
            group_offset_delta = cur.read_sleb()? as u64;
        }
        let mut group_info: u64 = 0;
        if grouped_info {
            group_info = cur.read_sleb()? as u64;
        }
        if group_has_addend && grouped_addend {
            addend = cur.read_sleb()?;
        }

        for _ in 0..group_size {
            if grouped_offset_delta {
                offset += group_offset_delta;
            } else {
                offset += cur.read_sleb()? as u64;
            }
            let info: u64 = if grouped_info {
                group_info
            } else {
                cur.read_sleb()? as u64
            };
            if group_has_addend && !grouped_addend {
                addend += cur.read_sleb()?;
            }

            // info 拆分：r_sym (高 32) + r_type (低 32)，与 ELF64 RELA 一致。
            let r_sym = (info >> 32) as u32;
            let r_type = (info & 0xFFFF_FFFF) as u32;

            let kind = classify_aarch64_reloc(r_type);
            let symbol_index = if r_sym == 0 { None } else { Some(r_sym) };
            let final_addend = if is_rela_section { addend } else { 0 };

            out.push(RelocationRecord {
                offset,
                symbol_index,
                addend: final_addend,
                kind,
            });
        }
    }

    Ok(out)
}

/// AArch64 relocation type → 我们的 [`RelocationKind`] 归一化。
///
/// 与 `parser_elf.rs::classify_aarch64_reloc` 算法相同；这里复制一份
/// 是因为 packed 模块和主 parser 模块互不依赖，避免循环。
fn classify_aarch64_reloc(r_type: u32) -> RelocationKind {
    match r_type {
        1027 => RelocationKind::Relative,
        1025 => RelocationKind::GlobDat,
        1026 => RelocationKind::JumpSlot,
        257 => RelocationKind::Abs64,
        _ => RelocationKind::Other(r_type),
    }
}

/// SLEB128 reader。Android packed 全用 SLEB128 编码（包括"无符号"字段也走 SLEB）。
struct SlebCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SlebCursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_sleb(&mut self) -> Result<i64, ElfParseError> {
        let mut result: i64 = 0;
        let mut shift: u32 = 0;
        const STOP: u8 = 0x80;
        const PAYLOAD: u8 = 0x7F;
        const SIGN_BIT: u8 = 0x40;

        let mut sign_byte;
        loop {
            if self.pos >= self.data.len() {
                return Err(ElfParseError::Truncated(
                    "packed reloc: sleb128 ran past section end",
                ));
            }
            sign_byte = self.data[self.pos];
            self.pos += 1;
            // LEB128 拼接：低 7 位按小端顺序累加到 result 的对应 shift 段。
            // 用 u64 中转避免 i64 算术溢出告警。
            let chunk = (sign_byte & PAYLOAD) as u64;
            if shift < 64 {
                result |= (chunk << shift) as i64;
            }
            shift += 7;
            if sign_byte & STOP == 0 {
                break;
            }
        }
        // 符号扩展：当最高 payload bit 为 1 且 shift < 64 时把高位补 1。
        if shift < 64 && (sign_byte & SIGN_BIT) != 0 {
            result |= -1_i64 << shift;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手工构造一个最小的 APS2 段：1 条 R_AARCH64_RELATIVE（r_sym=0）。
    ///
    /// 编码：
    ///   magic = b"APS2"
    ///   count = 1 (sleb128: 0x01)
    ///   offset_to_first = 0 (sleb128: 0x00)
    ///   group_size = 1 (0x01)
    ///   group_flags = GROUPED_BY_INFO | GROUPED_BY_ADDEND | GROUP_HAS_ADDEND
    ///                = 2 | 8 | 4 = 14 (0x0E)
    ///   group_info = (0 << 32) | 1027 = 1027 (sleb128: 多 byte)
    ///   group_addend = 0x100 (sleb128)
    ///   offset_delta = 0x40 (sleb128)
    ///
    /// 期望解码出一条：offset=0x40, symbol=None, kind=Relative, addend=0x100。
    #[test]
    fn decodes_minimal_relative_packed() {
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(APS2_MAGIC);
        bytes.push(0x01); // count = 1
        bytes.push(0x00); // offset_to_first = 0
        bytes.push(0x01); // group_size = 1
        bytes.push(0x0E); // group_flags = 14
        // group_info = 1027 = 0x403。sleb128: 0x83, 0x08（0x403 低 7 位 = 0x03，高位 = 0x08）
        bytes.push(0x83);
        bytes.push(0x08);
        // group_addend = 256 = 0x100。sleb128: 0x80, 0x02
        bytes.push(0x80);
        bytes.push(0x02);
        // offset_delta = 64 = 0x40。
        // 注意：单字节 0x40 在 sleb128 里符号位被置 1，会被解释为 -64。
        // 正确编码 +64 需要两字节：0xC0 (低 7 位 0x40, 续位置 1), 0x00。
        bytes.push(0xC0);
        bytes.push(0x00);

        let recs = decode_packed(&bytes, true).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].offset, 0x40);
        assert_eq!(recs[0].symbol_index, None);
        assert_eq!(recs[0].addend, 0x100);
        assert_eq!(recs[0].kind, RelocationKind::Relative);
    }

    #[test]
    fn rejects_bad_magic() {
        let bytes = [0u8; 8];
        assert!(decode_packed(&bytes, true).is_err());
    }
}
