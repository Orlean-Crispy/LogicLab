//! 值模型（v3 §4 / §5.2 / ADR-4 / ADR-22）
//!
//! - 位宽参数化：{1, 4, 8, 16, 32}
//! - 二值信号（0/1）+ 位级"未知"标记（X），用于多驱动冲突与未定义显示
//! - 逻辑层全整数，不出现浮点

/// 信号位宽
pub type Width = u8;

/// 规格允许的位宽档位（v3 §4）
pub const WIDTHS: [Width; 5] = [1, 4, 8, 16, 32];
/// 最大位宽
pub const MAX_WIDTH: Width = 32;

/// 把任意位宽归一到最近的可选档位（向上取档）
pub fn normalize_width(w: u32) -> Width {
    if w <= 1 {
        1
    } else if w <= 4 {
        4
    } else if w <= 8 {
        8
    } else if w <= 16 {
        16
    } else {
        32
    }
}

/// 位宽掩码；32 位时返回 u32::MAX（避免移位溢出）
#[inline]
pub fn width_mask(w: Width) -> u32 {
    if w >= 32 {
        u32::MAX
    } else {
        (1u32 << w) - 1
    }
}

/// 表示 `bits` 位无符号量所需的最小位宽（至少 1）
pub fn bits_needed(bits: u32) -> Width {
    let mut w: Width = 1;
    while (1u32 << w) < bits.max(1) && w < MAX_WIDTH {
        w += 1;
    }
    w
}

/// 单 bit 三态取值
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bit {
    Zero,
    One,
    /// 未知 / 冲突（多驱动打架）
    X,
}

impl Bit {
    #[inline]
    pub fn from_bool(b: bool) -> Bit {
        if b {
            Bit::One
        } else {
            Bit::Zero
        }
    }

    /// 未知按 0 参与运算（ADR-5：部分位无驱动 = 0）
    #[inline]
    pub fn as_bool(self) -> bool {
        matches!(self, Bit::One)
    }
}

/// 端口 / NET 上的值：`val` 为位值，`unk` 为"该位未知"掩码。
///
/// 未连接的 NET 读作全 0（ADR-5）；"是否有驱动"由网表判断，不编码进值。
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Hash)]
pub struct NetValue {
    pub val: u32,
    pub unk: u32,
}

impl NetValue {
    pub const ZERO: NetValue = NetValue { val: 0, unk: 0 };
    pub const ONE: NetValue = NetValue { val: 1, unk: 0 };

    #[inline]
    pub const fn new(val: u32, unk: u32) -> Self {
        Self { val, unk }
    }

    /// 由整数构造（按位宽截断）
    #[inline]
    pub fn from_u32(v: u32, w: Width) -> Self {
        Self { val: v & width_mask(w), unk: 0 }
    }

    #[inline]
    pub fn from_bit(b: bool) -> Self {
        if b {
            Self::ONE
        } else {
            Self::ZERO
        }
    }

    /// 全 1
    #[inline]
    pub fn all_ones(w: Width) -> Self {
        Self { val: width_mask(w), unk: 0 }
    }

    /// 全未知
    #[inline]
    pub fn all_unknown(w: Width) -> Self {
        Self { val: 0, unk: width_mask(w) }
    }

    /// 截断到给定位宽
    #[inline]
    pub fn with_width(self, w: Width) -> Self {
        let m = width_mask(w);
        Self { val: self.val & m, unk: self.unk & m }
    }

    #[inline]
    pub fn is_zero(self) -> bool {
        self.val == 0 && self.unk == 0
    }

    #[inline]
    pub fn has_unknown(self) -> bool {
        self.unk != 0
    }

    /// 取值（未知位按 0）
    #[inline]
    pub fn get(self, w: Width) -> u32 {
        self.val & width_mask(w)
    }

    /// 第 i 位
    #[inline]
    pub fn bit(self, i: u8) -> Bit {
        let m = 1u32 << (i & 31);
        if self.unk & m != 0 {
            Bit::X
        } else if self.val & m != 0 {
            Bit::One
        } else {
            Bit::Zero
        }
    }

    /// 同一位置上是否有内容差异（脏判断用）
    #[inline]
    pub fn differs(self, other: NetValue, w: Width) -> bool {
        let m = width_mask(w);
        ((self.val ^ other.val) | (self.unk ^ other.unk)) & m != 0
    }
}

/// 合并两个驱动源到同一 NET（多驱动 / 三态总线）。
///
/// 同位都被驱动且值相反 → 该位标记为 X（冲突）。
/// 常规多驱动在编辑期就被 DRC 拦截（§9.5），这里是运行期兜底。
#[inline]
pub fn merge_drivers(a: NetValue, b: NetValue, w: Width) -> NetValue {
    let m = width_mask(w);
    let both_known = !(a.unk | b.unk) & m;
    let conflict = both_known & (a.val ^ b.val);
    NetValue {
        // 冲突位取值 0，仅靠 unk 标记
        val: (a.val | b.val) & m & !conflict,
        unk: (a.unk | b.unk | conflict) & m,
    }
}

/// 把 `src` 合并进 `acc`（就地，避免分配）
#[inline]
pub fn merge_into(acc: &mut NetValue, src: NetValue, w: Width) {
    let m = width_mask(w);
    let both_known = !(acc.unk | src.unk) & m;
    let conflict = both_known & (acc.val ^ src.val);
    acc.val = (acc.val | src.val) & m & !conflict;
    acc.unk = (acc.unk | src.unk | conflict) & m;
}

// ---------------------------------------------------------------------------
// 算术（一律 wrap 补码回绕，ADR-4）
// ---------------------------------------------------------------------------

/// 无符号加，返回 (和, 进位)
#[inline]
pub fn add_wrap(a: u32, b: u32, cin: u32, w: Width) -> (u32, u32) {
    let m = width_mask(w);
    let s = (a & m) as u64 + (b & m) as u64 + (cin & 1) as u64;
    ((s as u32) & m, ((s >> w) & 1) as u32)
}

/// 无符号减，返回 (差, 借位)
#[inline]
pub fn sub_wrap(a: u32, b: u32, bin: u32, w: Width) -> (u32, u32) {
    let m = width_mask(w);
    let d = (a & m) as i64 - (b & m) as i64 - (bin & 1) as i64;
    ((d as u32) & m, if d < 0 { 1 } else { 0 })
}

/// 无符号乘（取低 w 位）
#[inline]
pub fn mul_wrap(a: u32, b: u32, w: Width) -> u32 {
    let m = width_mask(w);
    (((a & m) as u64 * (b & m) as u64) as u32) & m
}

/// 符号扩展成 i64（有符号比较 / 算术右移用）
#[inline]
pub fn to_signed(v: u32, w: Width) -> i64 {
    let m = width_mask(w);
    let x = (v & m) as i64;
    let sign = 1u32 << (w - 1);
    if v & sign != 0 {
        x - (m as i64) - 1
    } else {
        x
    }
}

/// 逻辑右移
#[inline]
pub fn shr(v: u32, sh: u32, w: Width) -> u32 {
    let m = width_mask(w);
    if sh >= w as u32 {
        0
    } else {
        (v & m) >> sh
    }
}

/// 算术右移
#[inline]
pub fn sar(v: u32, sh: u32, w: Width) -> u32 {
    let m = width_mask(w);
    if sh >= w as u32 {
        if to_signed(v, w) < 0 {
            m
        } else {
            0
        }
    } else {
        ((to_signed(v, w) >> sh) as u32) & m
    }
}

/// 左移
#[inline]
pub fn shl(v: u32, sh: u32, w: Width) -> u32 {
    let m = width_mask(w);
    if sh >= w as u32 {
        0
    } else {
        ((v & m) << sh) & m
    }
}

// ---------------------------------------------------------------------------
// 显示（仅 UI / CLI 用，不进热路径）
// ---------------------------------------------------------------------------

/// 按位宽 / 符号性格式化
pub fn format_value(v: NetValue, w: Width, signed: bool) -> String {
    if v.unk & width_mask(w) != 0 {
        let mut s = String::with_capacity(w as usize);
        for i in (0..w).rev() {
            s.push(match v.bit(i) {
                Bit::Zero => '0',
                Bit::One => '1',
                Bit::X => 'x',
            });
        }
        return s;
    }
    let raw = v.get(w);
    if w == 1 {
        return if raw != 0 { "1".into() } else { "0".into() };
    }
    if signed {
        format!("{}", to_signed(raw, w))
    } else {
        format!("{}", raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_normalization() {
        assert_eq!(normalize_width(0), 1);
        assert_eq!(normalize_width(1), 1);
        assert_eq!(normalize_width(2), 4);
        assert_eq!(normalize_width(5), 8);
        assert_eq!(normalize_width(9), 16);
        assert_eq!(normalize_width(17), 32);
        assert_eq!(normalize_width(64), 32);
    }

    #[test]
    fn mask_at_32_bits_does_not_overflow() {
        assert_eq!(width_mask(1), 1);
        assert_eq!(width_mask(8), 0xFF);
        assert_eq!(width_mask(32), u32::MAX);
    }

    #[test]
    fn merge_detects_conflict_per_bit() {
        let a = NetValue::from_u32(0b1010, 4);
        let b = NetValue::from_u32(0b1000, 4);
        let m = merge_drivers(a, b, 4);
        assert_eq!(m.bit(0), Bit::Zero);
        assert_eq!(m.bit(1), Bit::X);
        assert_eq!(m.bit(2), Bit::Zero);
        assert_eq!(m.bit(3), Bit::One);
    }

    #[test]
    fn merge_same_value_is_not_conflict() {
        let a = NetValue::from_u32(0b1111, 4);
        let m = merge_drivers(a, a, 4);
        assert!(!m.has_unknown());
        assert_eq!(m.get(4), 0b1111);
    }

    #[test]
    fn wrap_arithmetic() {
        let (s, c) = add_wrap(15, 1, 0, 4);
        assert_eq!((s, c), (0, 1));
        let (s, c) = add_wrap(0xFF, 0x01, 0, 8);
        assert_eq!((s, c), (0x00, 1));
        let (s, c) = add_wrap(u32::MAX, 1, 0, 32);
        assert_eq!((s, c), (0, 1));
    }

    #[test]
    fn subtract_borrow() {
        assert_eq!(sub_wrap(3, 5, 0, 8), (0xFE, 1));
        assert_eq!(sub_wrap(5, 3, 0, 8), (2, 0));
    }

    #[test]
    fn signed_roundtrip() {
        assert_eq!(to_signed(0b1111, 4), -1);
        assert_eq!(to_signed(0b1000, 4), -8);
        assert_eq!(to_signed(0b0111, 4), 7);
        assert_eq!(to_signed(0xFFFFFFFF, 32), -1);
    }

    #[test]
    fn shifts() {
        assert_eq!(shr(0b1000_0000, 4, 8), 0b0000_1000);
        assert_eq!(sar(0b1000_0000, 4, 8), 0b1111_1000);
        assert_eq!(shl(0b0000_0001, 4, 8), 0b0001_0000);
        assert_eq!(shr(1, 8, 8), 0);
        assert_eq!(sar(0x80, 8, 8), 0xFF);
    }

    #[test]
    fn bits_needed_matches_encoding() {
        assert_eq!(bits_needed(1), 1);
        assert_eq!(bits_needed(2), 1);
        assert_eq!(bits_needed(3), 2);
        assert_eq!(bits_needed(8), 3);
        assert_eq!(bits_needed(16), 4);
    }

    #[test]
    fn format_reports_unknown_bits() {
        let v = NetValue::new(0b0000, 0b0010);
        assert_eq!(format_value(v, 4, false), "00x0");
        assert_eq!(format_value(NetValue::from_u32(5, 4), 4, false), "5");
        assert_eq!(format_value(NetValue::from_u32(0b1111, 4), 4, true), "-1");
        assert_eq!(format_value(NetValue::ONE, 1, false), "1");
    }
}
