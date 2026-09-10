//! 内置示例电路
//!
//! 两个用途：用户在界面里一键载入当作起点；同时也是自测的固定夹具
//! （下面的测试会验证每个示例都能建成、都能自动走通、且没有 DRC 错误）。

use crate::board::Board;
use crate::defs::{DefId, Params};

/// 一个内置示例
pub struct Example {
    pub id: &'static str,
    pub name: &'static str,
    pub note: &'static str,
    pub build: fn() -> Board,
}

/// 全部示例
pub const ALL: &[Example] = &[
    Example {
        id: "half_adder",
        name: "NAND 半加器",
        note: "5 个 NAND 搭出 S=A⊕B 与 C=A·B；拨动两个开关看真值表",
        build: half_adder,
    },
    Example {
        id: "sr_latch",
        name: "NAND 锁存器",
        note: "低有效置位/复位，松开后自锁保持（组合反馈，导出 Verilog 前需改造）",
        build: sr_latch,
    },
    Example {
        id: "shift_reg",
        name: "三级移位寄存器",
        note: "每个时钟上升沿把数据往后推一级",
        build: shift_reg,
    },
    Example {
        id: "counter4",
        name: "4 位计数器",
        note: "计数器 + Splitter 拆位 + 四个探针",
        build: counter4,
    },
    Example {
        id: "mux2",
        name: "2 选 1 数据选择器",
        note: "MUX 用选择脚在两个字节间切换",
        build: mux2,
    },
];

/// 按 id 构造；未知 id 返回 None
pub fn build(id: &str) -> Option<Board> {
    ALL.iter().find(|e| e.id == id).map(|e| (e.build)())
}

/// 全部 id（供界面列出）
pub fn ids() -> Vec<&'static str> {
    ALL.iter().map(|e| e.id).collect()
}

/// 连线：示例电路必须能被自动走线走通，否则开发期就该炸出来
fn wire(b: &mut Board, from: (u32, usize), to: (u32, usize)) {
    let ok = b.connect_pins(from, to);
    debug_assert!(ok, "示例电路连线失败: {from:?} -> {to:?}");
}

fn sw1() -> Params {
    Params::default().width(1)
}

fn nand2() -> Params {
    Params::default().width(1).inputs(2)
}

fn led1() -> Params {
    Params::default().width(1)
}

/// 半加器：S = A⊕B（4 个 NAND），C = A·B（1 个 NAND）
fn half_adder() -> Board {
    let mut b = Board::new();
    let a = b.add_instance(DefId::Switch, sw1(), 0, 0);
    let c = b.add_instance(DefId::Switch, sw1(), 0, 8);
    let n1 = b.add_instance(DefId::Nand, nand2(), 7, 0);
    let n2 = b.add_instance(DefId::Nand, nand2(), 7, 4);
    let n3 = b.add_instance(DefId::Nand, nand2(), 7, 8);
    let xor = b.add_instance(DefId::Nand, nand2(), 14, 3);
    let and_g = b.add_instance(DefId::Nand, nand2(), 14, 10);
    let led_s = b.add_instance(DefId::Led, led1(), 22, 3);
    let led_c = b.add_instance(DefId::Led, led1(), 22, 10);

    wire(&mut b, (a, 0), (n1, 0));
    wire(&mut b, (c, 0), (n1, 1));
    wire(&mut b, (a, 0), (n2, 0));
    wire(&mut b, (n1, 2), (n2, 1));
    wire(&mut b, (c, 0), (n3, 0));
    wire(&mut b, (n1, 2), (n3, 1));
    wire(&mut b, (n2, 2), (xor, 0));
    wire(&mut b, (n3, 2), (xor, 1));
    wire(&mut b, (n1, 2), (and_g, 0));
    wire(&mut b, (n1, 2), (and_g, 1));
    wire(&mut b, (xor, 2), (led_s, 0));
    wire(&mut b, (and_g, 2), (led_c, 0));
    b
}

/// 低有效 NAND 锁存器：Q = NAND(Sbar, Qbar)，Qbar = NAND(Rbar, Q)
fn sr_latch() -> Board {
    let mut b = Board::new();
    let sbar = b.add_instance(DefId::Switch, sw1(), 0, 0);
    let rbar = b.add_instance(DefId::Switch, sw1(), 0, 8);
    let g1 = b.add_instance(DefId::Nand, nand2(), 7, 0);
    let g2 = b.add_instance(DefId::Nand, nand2(), 7, 8);
    let led_q = b.add_instance(DefId::Led, led1(), 16, 1);
    let led_qb = b.add_instance(DefId::Led, led1(), 16, 9);

    wire(&mut b, (sbar, 0), (g1, 0));
    wire(&mut b, (g2, 2), (g1, 1));
    wire(&mut b, (rbar, 0), (g2, 0));
    wire(&mut b, (g1, 2), (g2, 1));
    wire(&mut b, (g1, 2), (led_q, 0));
    wire(&mut b, (g2, 2), (led_qb, 0));
    b
}

/// 三级移位寄存器，共用一路时钟
fn shift_reg() -> Board {
    let mut b = Board::new();
    let clk = b.add_instance(DefId::Clock, sw1(), 0, 0);
    let d = b.add_instance(DefId::Switch, sw1(), 0, 8);
    let r1 = b.add_instance(DefId::Register, Params::default().width(1).opts(0), 8, 0);
    let r2 = b.add_instance(DefId::Register, Params::default().width(1).opts(0), 16, 0);
    let r3 = b.add_instance(DefId::Register, Params::default().width(1).opts(0), 24, 0);
    let out = b.add_instance(DefId::Led, led1(), 32, 1);

    for r in [r1, r2, r3] {
        wire(&mut b, (clk, 0), (r, 1));
    }
    wire(&mut b, (d, 0), (r1, 0));
    wire(&mut b, (r1, 2), (r2, 0));
    wire(&mut b, (r2, 2), (r3, 0));
    wire(&mut b, (r3, 2), (out, 0));
    b
}

/// 4 位计数器，输出经 Splitter 拆成 4 个探针
fn counter4() -> Board {
    let mut b = Board::new();
    let clk = b.add_instance(DefId::Clock, sw1(), 0, 0);
    let cnt = b.add_instance(DefId::Counter, Params::default().width(4).opts(0), 8, 0);
    let sp = b.add_instance(DefId::Splitter, Params::default().width(4), 16, 0);

    wire(&mut b, (clk, 0), (cnt, 0));
    wire(&mut b, (cnt, 1), (sp, 0));
    for i in 0..4 {
        let led = b.add_instance(DefId::Led, led1(), 24, i as i32 * 4);
        wire(&mut b, (sp, 1 + i as usize), (led, 0));
    }
    b
}

/// 2 选 1 MUX：两个字节源，一个选择脚
fn mux2() -> Board {
    let mut b = Board::new();
    let d0 = b.add_instance(DefId::Switch, Params::default().width(8), 0, 0);
    let d1 = b.add_instance(DefId::Switch, Params::default().width(8), 0, 6);
    let sel = b.add_instance(DefId::Switch, sw1(), 0, 12);
    let mux = b.add_instance(DefId::Mux, Params::default().width(8).inputs(2), 10, 0);
    let out = b.add_instance(DefId::Led, Params::default().width(8), 20, 0);

    wire(&mut b, (d0, 0), (mux, 0));
    wire(&mut b, (d1, 0), (mux, 1));
    wire(&mut b, (sel, 0), (mux, 2));
    wire(&mut b, (mux, 3), (out, 0));
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::{check, Severity};

    #[test]
    fn every_example_builds_with_wires() {
        for ex in ALL {
            let b = (ex.build)();
            assert!(!b.instances.is_empty(), "{} 没有元件", ex.id);
            assert!(!b.wires.is_empty(), "{} 没有导线", ex.id);
            for w in &b.wires {
                assert!(w.points.len() >= 2, "{} 出现退化导线", ex.id);
            }
        }
    }

    /// 刻意演示组合反馈的示例：仿真能跑，但导出 Verilog 前必须改造。
    /// DRC 对它们报组合环是**正确行为**，因此在这里显式豁免。
    const COMBINATIONAL_FEEDBACK_DEMOS: &[&str] = &["sr_latch"];

    #[test]
    fn every_example_is_drc_clean() {
        for ex in ALL {
            let b = (ex.build)();
            let errors: Vec<_> = check(&b)
                .into_iter()
                .filter(|i| i.severity == Severity::Error)
                .filter(|i| {
                    !(i.kind == crate::drc::IssueKind::CombinationalLoop
                        && COMBINATIONAL_FEEDBACK_DEMOS.contains(&ex.id))
                })
                .collect();
            assert!(errors.is_empty(), "{} 有 DRC 错误: {errors:?}", ex.id);
        }
    }

    /// 反过来也要确认：DNC 确实能抓出 SR 锁存器的组合环（否则上面那条豁免就没意义）
    #[test]
    fn sr_latch_is_indeed_flagged_as_combinational_loop() {
        let b = build("sr_latch").unwrap();
        assert!(check(&b)
            .iter()
            .any(|i| i.kind == crate::drc::IssueKind::CombinationalLoop));
    }

    #[test]
    fn lookup_by_id_works() {
        assert!(build("half_adder").is_some());
        assert!(build("nope").is_none());
        assert_eq!(ids().len(), ALL.len());
    }
}
