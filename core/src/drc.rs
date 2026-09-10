//! 设计规则检查（v3 §9.5）
//!
//! 规格要求 DRC **必须先于 Verilog 导出**落地：可综合电路的约束（无组合环、
//! 时钟规范）要早一天定进数据模型，导出器才能少一天痛苦。
//!
//! 检查项与规格一致：
//!   - 多驱动（Error）—— 两个输出接同一网络
//!   - 位宽不匹配（Error）—— ADR-6：禁止隐式拆合，必须走 Splitter / Merger
//!   - 悬空输入（Warning）—— ADR-5：读作 0，但应当提示
//!   - 组合环（Error）—— 反馈必须经过时序元件
//!
//! 注意：因为所有组件输出延迟 1 tick（ADR-1），组合环在仿真里不会卡死，
//! 但它对应真实硬件里的振荡，属于设计错误，所以按 Error 报。

use crate::board::{Board, Netlist, NO_NET};
use crate::defs::Dir;

/// 问题严重程度
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    /// 提示性问题，电路仍可仿真
    Warning,
    /// 设计错误，应当修掉（Verilog 导出会被它拦下）
    Error,
}

impl Severity {
    pub fn code(self) -> i64 {
        match self {
            Severity::Warning => 0,
            Severity::Error => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Severity::Warning => "警告",
            Severity::Error => "错误",
        }
    }
}

/// 问题类别
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IssueKind {
    MultiDriver,
    WidthMismatch,
    DanglingInput,
    CombinationalLoop,
}

impl IssueKind {
    pub fn code(self) -> i64 {
        match self {
            IssueKind::MultiDriver => 0,
            IssueKind::WidthMismatch => 1,
            IssueKind::DanglingInput => 2,
            IssueKind::CombinationalLoop => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            IssueKind::MultiDriver => "多驱动",
            IssueKind::WidthMismatch => "位宽不匹配",
            IssueKind::DanglingInput => "悬空输入",
            IssueKind::CombinationalLoop => "组合环",
        }
    }
}

/// 一条 DRC 结果
#[derive(Clone, Debug)]
pub struct Issue {
    pub kind: IssueKind,
    pub severity: Severity,
    /// 相关网络（NO_NET 表示与网络无关）
    pub net: u32,
    /// 相关元件（u32::MAX 表示与单个元件无关）
    pub inst: u32,
    pub detail: String,
}

/// 跑一遍全部检查
pub fn check(board: &Board) -> Vec<Issue> {
    let nl = board.compile();
    let mut issues = Vec::new();
    check_nets(board, &nl, &mut issues);
    check_combinational_loops(board, &nl, &mut issues);
    issues
}

/// 网络层面的检查：多驱动 / 位宽 / 悬空
fn check_nets(board: &Board, nl: &Netlist, out: &mut Vec<Issue>) {
    let nets = nl.net_count as usize;
    let mut driver_count = vec![0u16; nets];
    let mut driver_width = vec![0u8; nets];
    let mut receivers: Vec<Vec<(u32, u8)>> = vec![Vec::new(); nets];

    for (ii, inst) in board.instances.iter().enumerate() {
        let base = nl.pin_start[ii] as usize;
        for (slot, p) in inst.pins.iter().enumerate() {
            let net = nl.pin_net[base + slot];
            if net == NO_NET {
                // 压根没有连接的输入（ADR-5：读作 0，但应当提示出来）
                if p.dir == Dir::In {
                    out.push(Issue {
                        kind: IssueKind::DanglingInput,
                        severity: Severity::Warning,
                        net: NO_NET,
                        inst: ii as u32,
                        detail: format!("输入 {} 未连接（按 ADR-5 读作 0）", p.name),
                    });
                }
                continue;
            }
            let n = net as usize;
            if p.dir == Dir::Out {
                if driver_count[n] == 0 {
                    driver_width[n] = p.width;
                }
                driver_count[n] = driver_count[n].saturating_add(1);
            } else {
                receivers[n].push((ii as u32, p.width));
            }
        }
    }

    for n in 0..nets {
        if driver_count[n] > 1 {
            out.push(Issue {
                kind: IssueKind::MultiDriver,
                severity: Severity::Error,
                net: n as u32,
                inst: u32::MAX,
                detail: format!("{} 个输出同时驱动同一网络", driver_count[n]),
            });
        }
        if driver_count[n] > 0 {
            let dw = driver_width[n];
            for &(inst, w) in &receivers[n] {
                if w != dw {
                    out.push(Issue {
                        kind: IssueKind::WidthMismatch,
                        severity: Severity::Error,
                        net: n as u32,
                        inst,
                        detail: format!("驱动 {dw} 位，接收 {w} 位（须经 Splitter / Merger）"),
                    });
                }
            }
        } else {
            for &(inst, _) in &receivers[n] {
                out.push(Issue {
                    kind: IssueKind::DanglingInput,
                    severity: Severity::Warning,
                    net: n as u32,
                    inst,
                    detail: "该网络没有任何驱动（按 ADR-5 读作 0）".to_string(),
                });
            }
        }
    }
}

/// 组合环：对组件级有向图求强连通分量，分量内若**全部是组合组件**，就是组合环。
/// （反馈本身合法，只要环上有时序元件——这正是锁存器与寄存器的用法。）
fn check_combinational_loops(board: &Board, nl: &Netlist, out: &mut Vec<Issue>) {
    let n_inst = board.instances.len();
    if n_inst == 0 {
        return;
    }
    let nets = nl.net_count as usize;

    // 网络 → 读它的组件
    let mut net_rx: Vec<Vec<u32>> = vec![Vec::new(); nets];
    for (ii, inst) in board.instances.iter().enumerate() {
        if inst.def.is_sink() {
            continue;
        }
        let base = nl.pin_start[ii] as usize;
        for (slot, p) in inst.pins.iter().enumerate() {
            if p.dir != Dir::In {
                continue;
            }
            let net = nl.pin_net[base + slot];
            if net != NO_NET {
                net_rx[net as usize].push(ii as u32);
            }
        }
    }

    // 组件级邻接表
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); n_inst];
    for (ii, inst) in board.instances.iter().enumerate() {
        if inst.def.is_sink() {
            continue;
        }
        let base = nl.pin_start[ii] as usize;
        for (slot, p) in inst.pins.iter().enumerate() {
            if p.dir != Dir::Out {
                continue;
            }
            let net = nl.pin_net[base + slot];
            if net == NO_NET {
                continue;
            }
            for &rx in &net_rx[net as usize] {
                if rx != ii as u32 {
                    adj[ii].push(rx);
                }
            }
        }
    }

    let is_seq: Vec<bool> = board
        .instances
        .iter()
        .map(|i| i.def.is_sequential())
        .collect();

    for comp in tarjan_scc(&adj) {
        let self_loop = comp.len() == 1 && adj[comp[0] as usize].contains(&comp[0]);
        if comp.len() < 2 && !self_loop {
            continue;
        }
        if comp.iter().all(|&i| !is_seq[i as usize]) {
            out.push(Issue {
                kind: IssueKind::CombinationalLoop,
                severity: Severity::Error,
                net: NO_NET,
                inst: comp[0],
                detail: format!(
                    "{} 个组合组件构成反馈环（反馈必须经过时序元件）",
                    comp.len()
                ),
            });
        }
    }
}

/// Tarjan 强连通分量
///
/// 用递归实现，深度与电路级数同阶；万级长链在默认栈下仍然安全。
fn tarjan_scc(adj: &[Vec<u32>]) -> Vec<Vec<u32>> {
    struct State<'a> {
        adj: &'a [Vec<u32>],
        index: Vec<i32>,
        low: Vec<i32>,
        on_stack: Vec<bool>,
        stack: Vec<u32>,
        next: i32,
        comps: Vec<Vec<u32>>,
    }

    impl State<'_> {
        fn visit(&mut self, v: u32) {
            let vi = v as usize;
            self.index[vi] = self.next;
            self.low[vi] = self.next;
            self.next += 1;
            self.stack.push(v);
            self.on_stack[vi] = true;

            for k in 0..self.adj[vi].len() {
                let w = self.adj[vi][k];
                let wi = w as usize;
                if self.index[wi] < 0 {
                    self.visit(w);
                    let lw = self.low[wi];
                    if lw < self.low[vi] {
                        self.low[vi] = lw;
                    }
                } else if self.on_stack[wi] && self.index[wi] < self.low[vi] {
                    self.low[vi] = self.index[wi];
                }
            }

            if self.low[vi] == self.index[vi] {
                let mut comp = Vec::new();
                while let Some(w) = self.stack.pop() {
                    self.on_stack[w as usize] = false;
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                self.comps.push(comp);
            }
        }
    }

    let n = adj.len();
    let mut st = State {
        adj,
        index: vec![-1; n],
        low: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        next: 0,
        comps: Vec::new(),
    };
    for v in 0..n as u32 {
        if st.index[v as usize] < 0 {
            st.visit(v);
        }
    }
    st.comps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::{DefId, Params, OPT_CARRY};

    #[test]
    fn clean_circuit_has_no_errors() {
        let mut b = Board::new();
        let sw = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
        let g = b.add_instance(DefId::And, Params::default().width(1).inputs(2), 6, 0);
        let led = b.add_instance(DefId::Led, Params::default().width(1), 12, 0);
        assert!(b.connect_pins((sw, 0), (g, 0)));
        assert!(b.connect_pins((g, 2), (led, 0)));
        let issues = check(&b);
        let errors: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "不该有错误: {errors:?}");
        // AND 的第二个输入悬空 → 应当有悬空警告
        assert!(issues.iter().any(|i| i.kind == IssueKind::DanglingInput));
    }

    #[test]
    fn width_mismatch_is_an_error() {
        let mut b = Board::new();
        let c8 = b.add_instance(DefId::Constant, Params::default().width(8), 0, 0);
        let g = b.add_instance(DefId::Not, Params::default().width(1), 8, 0);
        assert!(b.connect_pins((c8, 0), (g, 0)));
        let issues = check(&b);
        assert!(
            issues
                .iter()
                .any(|i| i.kind == IssueKind::WidthMismatch && i.severity == Severity::Error),
            "8 位接 1 位必须被拦下"
        );
    }

    #[test]
    fn multi_driver_is_an_error() {
        let mut b = Board::new();
        let a = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
        let c = b.add_instance(DefId::Switch, Params::default().width(1), 0, 6);
        let led = b.add_instance(DefId::Led, Params::default().width(1), 12, 0);
        assert!(b.connect_pins((a, 0), (led, 0)));
        assert!(b.connect_pins((c, 0), (led, 0)));
        let issues = check(&b);
        assert!(issues.iter().any(|i| i.kind == IssueKind::MultiDriver));
    }

    #[test]
    fn pure_combinational_loop_is_flagged() {
        // 三级反相器首尾相接：纯组合环
        let mut b = Board::new();
        let p = Params::default().width(1);
        let g1 = b.add_instance(DefId::Not, p, 0, 0);
        let g2 = b.add_instance(DefId::Not, p, 6, 0);
        let g3 = b.add_instance(DefId::Not, p, 12, 0);
        assert!(b.connect_pins((g1, 1), (g2, 0)));
        assert!(b.connect_pins((g2, 1), (g3, 0)));
        assert!(b.connect_pins((g3, 1), (g1, 0)));
        let issues = check(&b);
        assert!(
            issues.iter().any(|i| i.kind == IssueKind::CombinationalLoop),
            "纯组合环必须被报出来"
        );
    }

    #[test]
    fn feedback_through_register_is_legal() {
        // 寄存器把输出带回输入：环上有时序元件 → 合法
        let mut b = Board::new();
        let p = Params::default().width(1).opts(0);
        let r = b.add_instance(DefId::Register, p, 0, 0);
        let g = b.add_instance(DefId::Not, Params::default().width(1), 8, 0);
        assert!(b.connect_pins((r, 2), (g, 0)));
        assert!(b.connect_pins((g, 1), (r, 0)));
        let issues = check(&b);
        assert!(
            !issues.iter().any(|i| i.kind == IssueKind::CombinationalLoop),
            "经过寄存器的反馈是合法设计"
        );
    }

    #[test]
    fn adder_chain_is_clean() {
        let mut b = Board::new();
        let a = b.add_instance(DefId::Constant, Params::default().width(8), 0, 0);
        let c = b.add_instance(DefId::Constant, Params::default().width(8), 0, 4);
        let sum = b.add_instance(DefId::Adder, Params::default().width(8).opts(OPT_CARRY), 8, 0);
        assert!(b.connect_pins((a, 0), (sum, 0)));
        assert!(b.connect_pins((c, 0), (sum, 1)));
        let issues = check(&b);
        assert!(!issues.iter().any(|i| i.severity == Severity::Error));
    }
}
