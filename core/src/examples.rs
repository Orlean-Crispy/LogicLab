//! 内置示例电路
//!
//! 两个用途：用户在界面里一键载入当作起点；同时也是自测的固定夹具
//! （下面的测试会验证每个示例都能建成、都能自动走通、且没有 DRC 错误）。
//!
//! 示例返回的是**整个工程**而不只是主图纸——层次示例（全加器）需要多张图纸。

use crate::board::Board;
use crate::defs::{DefId, Params};
use crate::elaborate::custom_pins;
use crate::save::Project;

/// 一个内置示例
pub struct Example {
    pub id: &'static str,
    pub name: &'static str,
    /// 英文名与说明。内置内容属于「程序自带的东西」，跟着界面语言走；
    /// 用户自己改过名的图纸不受影响（Board::name_en 留空即回落原名）。
    pub name_en: &'static str,
    pub note: &'static str,
    pub note_en: &'static str,
    pub build: fn() -> Project,
}

/// 全部示例
pub const ALL: &[Example] = &[
    Example {
        id: "half_adder",
        name: "NAND 半加器",
        name_en: "NAND Half Adder",
        note: "5 个 NAND 搭出 S=A⊕B 与 C=A·B；拨动两个开关看真值表",
        note_en: "Five NANDs give S = A xor B and C = A and B; flip the two switches to walk the truth table",
        build: || one("NAND 半加器", "NAND Half Adder", half_adder()),
    },
    Example {
        id: "full_adder",
        name: "层次全加器",
        name_en: "Hierarchical Full Adder",
        note: "把半加器封装成自定义元件，再用两个半加器 + 一个 OR 搭出全加器（双击实例进入子电路）",
        note_en: "Wrap a half adder into a custom component, then build a full adder from two of them plus an OR (double-click an instance to look inside)",
        build: full_adder,
    },
    Example {
        id: "sr_latch",
        name: "NAND 锁存器",
        name_en: "NAND Latch",
        note: "低有效置位/复位，松开后自锁保持（组合反馈，导出 Verilog 前需改造）",
        note_en: "Active-low set/reset; release both and it latches (combinational feedback, needs rework before Verilog export)",
        build: || one("NAND 锁存器", "NAND Latch", sr_latch()),
    },
    Example {
        id: "shift_reg",
        name: "三级移位寄存器",
        name_en: "3-Stage Shift Register",
        note: "每个时钟上升沿把数据往后推一级",
        note_en: "Each rising clock edge pushes the data one stage further",
        build: || one("三级移位寄存器", "3-Stage Shift Register", shift_reg()),
    },
    Example {
        id: "counter4",
        name: "4 位计数器",
        name_en: "4-Bit Counter",
        note: "8 位计数器，经 Splitter 拆成高/低两个 4 位探针",
        note_en: "An 8-bit counter split by a Splitter into two 4-bit probes",
        build: || one("4 位计数器", "4-Bit Counter", counter4()),
    },
    Example {
        id: "counter_display",
        name: "计数器 + 外设",
        name_en: "Counter + Peripherals",
        note: "同一个计数器同时驱动数码管与 8×8 点阵屏（地址 + 数据 + 写使能）",
        note_en: "One counter driving both a 7-segment display and an 8x8 dot-matrix screen (address + data + write enable)",
        build: || one("计数器 + 外设", "Counter + Peripherals", counter_display()),
    },
    Example {
        id: "mux2",
        name: "2 选 1 数据选择器",
        name_en: "2-to-1 Multiplexer",
        note: "MUX 用选择脚在两个字节间切换",
        note_en: "A MUX switches between two byte sources with one select pin",
        build: || one("2 选 1 数据选择器", "2-to-1 Multiplexer", mux2()),
    },
];

/// 按 id 构造；未知 id 返回 None
pub fn build(id: &str) -> Option<Project> {
    ALL.iter().find(|e| e.id == id).map(|e| (e.build)())
}

/// 全部 id（供界面列出）
pub fn ids() -> Vec<&'static str> {
    ALL.iter().map(|e| e.id).collect()
}

/// 单图纸工程的便捷构造
fn one(name: &str, name_en: &str, board: Board) -> Project {
    let mut p = Project::new(name);
    let mut b = board;
    b.name = name.to_string();
    b.name_en = name_en.to_string();
    p.add_board(b);
    p
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

/// 把上面那套 NAND 半加器加接口，做成可复用的子电路
fn half_adder_cell() -> Board {
    let mut b = Board::new();
    b.name = "半加器".to_string();
    b.name_en = "Half Adder".to_string();
    let a = b.add_instance(DefId::InputPin, sw1(), 0, 0);
    let bb = b.add_instance(DefId::InputPin, sw1(), 0, 6);
    b.instances[a as usize].display_name = "A".to_string();
    b.instances[bb as usize].display_name = "B".to_string();
    let n1 = b.add_instance(DefId::Nand, nand2(), 7, 0);
    let n2 = b.add_instance(DefId::Nand, nand2(), 7, 4);
    let n3 = b.add_instance(DefId::Nand, nand2(), 7, 8);
    let xor = b.add_instance(DefId::Nand, nand2(), 14, 3);
    let and_g = b.add_instance(DefId::Nand, nand2(), 14, 10);
    let s = b.add_instance(DefId::OutputPin, sw1(), 22, 3);
    let c = b.add_instance(DefId::OutputPin, sw1(), 22, 10);
    b.instances[s as usize].display_name = "S".to_string();
    b.instances[c as usize].display_name = "C".to_string();

    wire(&mut b, (a, 0), (n1, 0));
    wire(&mut b, (bb, 0), (n1, 1));
    wire(&mut b, (a, 0), (n2, 0));
    wire(&mut b, (n1, 2), (n2, 1));
    wire(&mut b, (bb, 0), (n3, 0));
    wire(&mut b, (n1, 2), (n3, 1));
    wire(&mut b, (n2, 2), (xor, 0));
    wire(&mut b, (n3, 2), (xor, 1));
    wire(&mut b, (n1, 2), (and_g, 0));
    wire(&mut b, (n1, 2), (and_g, 1));
    wire(&mut b, (xor, 2), (s, 0));
    wire(&mut b, (and_g, 2), (c, 0));
    b
}

/// 层次示例：全加器 = 两个半加器 + 一个 OR。
///
/// 接口引脚按名字排序：输入 A、B，输出 C、S —— 于是实例的槽位是 0=A 1=B 2=C 3=S。
fn full_adder() -> Project {
    let cell = half_adder_cell();
    let pins = custom_pins(&cell);

    let mut root = Board::new();
    root.name = "全加器".to_string();
    root.name_en = "Full Adder".to_string();
    let sa = root.add_instance(DefId::Switch, sw1(), 0, 0);
    let sb = root.add_instance(DefId::Switch, sw1(), 0, 8);
    let sc = root.add_instance(DefId::Switch, sw1(), 0, 16);
    let h1 = root.add_sub_instance(1, "半加器", 10, 0);
    root.instances[h1 as usize].pins = pins.clone();
    let h2 = root.add_sub_instance(1, "半加器", 24, 0);
    root.instances[h2 as usize].pins = pins;
    let or_g = root.add_instance(DefId::Or, nand2(), 38, 6);
    let led_s = root.add_instance(DefId::Led, led1(), 46, 0);
    let led_co = root.add_instance(DefId::Led, led1(), 46, 8);

    wire(&mut root, (sa, 0), (h1, 0));
    wire(&mut root, (sb, 0), (h1, 1));
    wire(&mut root, (h1, 3), (h2, 0));
    wire(&mut root, (sc, 0), (h2, 1));
    wire(&mut root, (h1, 2), (or_g, 0));
    wire(&mut root, (h2, 2), (or_g, 1));
    wire(&mut root, (h2, 3), (led_s, 0));
    wire(&mut root, (or_g, 2), (led_co, 0));

    let mut p = Project::new("层次全加器");
    p.add_board(root);
    p.add_board(cell);
    p
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
    let cnt = b.add_instance(DefId::Counter, Params::default().width(8).opts(0), 8, 0);
    // 8 位拆成低 4 位 + 高 4 位（v4 §6.3 的两段语义）
    let sp = b.add_instance(
        DefId::Splitter,
        Params { value: 4, ..DefId::Splitter.default_params().width(8) },
        16,
        0,
    );
    let lo = b.add_instance(DefId::Led, Params::default().width(4), 24, 0);
    let hi = b.add_instance(DefId::Led, Params::default().width(4), 24, 4);

    wire(&mut b, (clk, 0), (cnt, 0));
    wire(&mut b, (cnt, 1), (sp, 0));
    wire(&mut b, (sp, 1), (lo, 0));
    wire(&mut b, (sp, 2), (hi, 0));
    b
}

/// 外设示例：计数器同时驱动数码管与点阵屏。
///
/// 显示屏是内存映射的：ADDR 选行、DATA 写像素、WE 使能、CLK 上升沿提交。
fn counter_display() -> Board {
    let mut b = Board::new();
    let clk = b.add_instance(DefId::Clock, sw1(), 0, 0);
    let cnt = b.add_instance(DefId::Counter, Params::default().width(8).opts(0), 8, 0);
    let seg = b.add_instance(DefId::SevenSeg, Params::default().width(8), 16, 12);
    // 8 位里低 3 位做行地址（8 行），高 5 位悬空即可
    let sp = b.add_instance(
        DefId::Splitter,
        Params { value: 3, ..DefId::Splitter.default_params().width(8) },
        16,
        0,
    );
    let one = b.add_instance(
        DefId::Constant,
        Params { width: 1, value: 1, ..DefId::Constant.default_params() },
        24,
        8,
    );
    let disp = b.add_instance(DefId::Display, Params::default().width(8).depth(8), 32, 0);

    wire(&mut b, (clk, 0), (cnt, 0));
    wire(&mut b, (cnt, 1), (seg, 0));
    wire(&mut b, (cnt, 1), (sp, 0));
    wire(&mut b, (sp, 1), (disp, 1));
    wire(&mut b, (cnt, 1), (disp, 0));
    wire(&mut b, (one, 0), (disp, 2));
    wire(&mut b, (clk, 0), (disp, 3));
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
    use crate::drc::{check, check_project, IssueKind, Severity};
    use crate::session::Session;

    #[test]
    fn every_example_builds_with_wires() {
        for ex in ALL {
            let p = (ex.build)();
            assert!(!p.boards.is_empty(), "{} 没有图纸", ex.id);
            let b = p.main().unwrap();
            assert!(!b.instances.is_empty(), "{} 没有元件", ex.id);
            assert!(!b.wires.is_empty(), "{} 没有导线", ex.id);
            for w in &b.wires {
                assert!(w.points.len() >= 2, "{} 出现退化导线", ex.id);
            }
        }
    }

    /// 刻意演示组合反馈的示例：仿真能跑，但导出 Verilog 前必须改造。
    /// DRC 对它们报组合环是**正确行为**，因此在这里显式豁免。
    const COMBINATIONAL_FEEDBACK_DEMOS: &[&'static str] = &["sr_latch"];

    #[test]
    fn every_example_is_drc_clean() {
        for ex in ALL {
            let p = (ex.build)();
            let errors: Vec<_> = check_project(&p.boards)
                .into_iter()
                .filter(|(_, i)| i.severity == Severity::Error)
                .filter(|(_, i)| {
                    !(i.kind == IssueKind::CombinationalLoop
                        && COMBINATIONAL_FEEDBACK_DEMOS.contains(&ex.id))
                })
                .collect();
            assert!(errors.is_empty(), "{} 有 DRC 错误: {errors:?}", ex.id);
        }
    }

    /// 反过来也要确认：DRC 确实能抓出 SR 锁存器的组合环（否则上面那条豁免就没意义）
    #[test]
    fn sr_latch_is_indeed_flagged_as_combinational_loop() {
        let p = build("sr_latch").unwrap();
        assert!(check(&p.boards[0]).iter().any(|i| i.kind == IssueKind::CombinationalLoop));
    }

    #[test]
    fn lookup_by_id_works() {
        assert!(build("half_adder").is_some());
        assert!(build("nope").is_none());
        assert_eq!(ids().len(), ALL.len());
    }

    /// 层次示例必须**算得对**：这是封装真的能用的证据，而不只是画得出来。
    #[test]
    fn full_adder_computes_through_sub_circuits() {
        let mut s = Session::from_project(build("full_adder").unwrap());
        assert_eq!(s.project().boards.len(), 2);
        // 根图纸：0=A 1=B 2=CI，6=S 7=CO
        for (a, b, ci, sum, co) in [
            (0, 0, 0, 0, 0),
            (1, 0, 0, 1, 0),
            (0, 1, 0, 1, 0),
            (1, 1, 0, 0, 1),
            (0, 0, 1, 1, 0),
            (1, 0, 1, 0, 1),
            (0, 1, 1, 0, 1),
            (1, 1, 1, 1, 1),
        ] {
            s.set_input(0, a);
            s.set_input(1, b);
            s.set_input(2, ci);
            // 两级半加器串起来有 6 层门延迟，再留出 OR 的一拍
            s.sim_run_for(10);
            assert_eq!(s.pin_value(6, 0).get(1) as u32, sum, "S: {a}+{b}+{ci}");
            assert_eq!(s.pin_value(7, 0).get(1) as u32, co, "CO: {a}+{b}+{ci}");
        }
    }

    /// 显示屏的状态必须活在 core 里：跑几拍后帧缓冲要真的有内容
    #[test]
    fn display_frame_buffer_fills_while_simulating() {
        let mut s = Session::from_project(build("counter_display").unwrap());
        s.sim_run_for(200);
        // 实例顺序：0=时钟 1=计数器 2=数码管 3=Splitter 4=常量 5=显示屏
        let fb = s.instance_mem(5);
        assert_eq!(fb.len(), 8, "8 行");
        assert!(fb.iter().any(|v| *v != 0), "跑了两百拍，画面不该还是全黑");
    }
}
