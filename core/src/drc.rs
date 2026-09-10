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

use std::collections::HashMap;

use crate::board::{Board, Netlist, NO_NET, NO_SUB};
use crate::defs::{DefId, Dir};

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

    pub fn label_en(self) -> &'static str {
        match self {
            Severity::Warning => "Warning",
            Severity::Error => "Error",
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
    /// 门控时钟（警告）：时序元件的 clock 不是来自 Clock 组件
    GatedClock,
    /// 图纸引用了自己，或几张图纸互相引用成环（层次，ADR-28）
    CircularReference,
}

impl IssueKind {
    pub fn code(self) -> i64 {
        match self {
            IssueKind::MultiDriver => 0,
            IssueKind::WidthMismatch => 1,
            IssueKind::DanglingInput => 2,
            IssueKind::CombinationalLoop => 3,
            IssueKind::GatedClock => 4,
            IssueKind::CircularReference => 5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            IssueKind::MultiDriver => "多驱动",
            IssueKind::WidthMismatch => "位宽不匹配",
            IssueKind::DanglingInput => "悬空输入",
            IssueKind::CombinationalLoop => "组合环",
            IssueKind::GatedClock => "门控时钟",
            IssueKind::CircularReference => "循环引用",
        }
    }

    pub fn label_en(self) -> &'static str {
        match self {
            IssueKind::MultiDriver => "Multiple drivers",
            IssueKind::WidthMismatch => "Width mismatch",
            IssueKind::DanglingInput => "Dangling input",
            IssueKind::CombinationalLoop => "Combinational loop",
            IssueKind::GatedClock => "Gated clock",
            IssueKind::CircularReference => "Circular reference",
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
    check_gated_clocks(board, &nl, &mut issues);
    issues
}

/// 全工程检查：每张图纸各查一遍，再补上跨图纸的引用问题。
///
/// 返回 (图纸索引, 问题)。层次电路的每张图纸都是独立可检查的单元——
/// 子电路外壳的引脚方向来自图纸接口，所以父图纸的网络检查天然正确。
pub fn check_project(boards: &[Board]) -> Vec<(u32, Issue)> {
    let mut out = Vec::new();
    for (i, b) in boards.iter().enumerate() {
        for (k, inst) in b.instances.iter().enumerate() {
            if inst.sub != NO_SUB && inst.sub as usize == i {
                out.push((
                    i as u32,
                    Issue {
                        kind: IssueKind::CircularReference,
                        severity: Severity::Error,
                        net: NO_NET,
                        inst: k as u32,
                        detail: format!("图纸「{}」引用了自己", b.name),
                    },
                ));
            }
        }
        for issue in check(b) {
            out.push((i as u32, issue));
        }
        // 间接引用环：i → j → … → i
        for (k, inst) in b.instances.iter().enumerate() {
            if inst.sub == NO_SUB || inst.sub as usize >= boards.len() || inst.sub as usize == i {
                continue;
            }
            if reaches(boards, inst.sub, i as u32) {
                out.push((
                    i as u32,
                    Issue {
                        kind: IssueKind::CircularReference,
                        severity: Severity::Error,
                        net: NO_NET,
                        inst: k as u32,
                        detail: format!("图纸「{}」经引用链绕回自己", b.name),
                    },
                ));
            }
        }
    }
    out
}

/// 从 from 出发能否沿引用边到达 target
fn reaches(boards: &[Board], from: u32, target: u32) -> bool {
    let n = boards.len();
    let mut seen = vec![false; n];
    let mut stack = vec![from];
    while let Some(x) = stack.pop() {
        let Some(b) = boards.get(x as usize) else {
            continue;
        };
        for inst in &b.instances {
            if inst.sub == NO_SUB || inst.sub as usize >= n {
                continue;
            }
            if inst.sub == target {
                return true;
            }
            if !seen[inst.sub as usize] {
                seen[inst.sub as usize] = true;
                stack.push(inst.sub);
            }
        }
    }
    false
}

/// 组件级有向图：A → B 表示 A 的输出驱动了 B 的输入。
/// 组合环检测与关键路径共用它，避免两处各写一遍。
fn component_graph(board: &Board, nl: &Netlist) -> Vec<Vec<u32>> {
    let n = board.instances.len();
    let nets = nl.net_count as usize;
    if n == 0 || nets == 0 {
        return vec![Vec::new(); n];
    }

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

    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); n];
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
    adj
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
    let adj = component_graph(board, nl);
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

/// 门控时钟（v4 §9.5 警告项）：时序元件的 clock 不是由 Clock 组件驱动。
///
/// 允许经过 Buffer 链——时钟树上插缓冲器是正常做法；插逻辑门才叫门控时钟。
fn check_gated_clocks(board: &Board, nl: &Netlist, out: &mut Vec<Issue>) {
    // 网络 → 驱动它的组件
    let mut net_driver: HashMap<u32, u32> = HashMap::new();
    for (ii, inst) in board.instances.iter().enumerate() {
        let base = nl.pin_start[ii] as usize;
        for (slot, p) in inst.pins.iter().enumerate() {
            if p.dir != Dir::Out {
                continue;
            }
            let net = nl.pin_net[base + slot];
            if net != NO_NET {
                net_driver.insert(net, ii as u32);
            }
        }
    }

    for (ii, inst) in board.instances.iter().enumerate() {
        if !inst.def.is_sequential() {
            continue;
        }
        let Some(clk_slot) = inst.pins.iter().position(|p| p.name.eq_ignore_ascii_case("clk"))
        else {
            continue;
        };
        let base = nl.pin_start[ii] as usize;
        let net = nl.pin_net[base + clk_slot];
        if net == NO_NET || clock_from_source(board, nl, &net_driver, net) {
            continue;
        }
        out.push(Issue {
            kind: IssueKind::GatedClock,
            severity: Severity::Warning,
            net,
            inst: ii as u32,
            detail: "时钟不是由 Clock 组件驱动（门控时钟，综合时有风险）".to_string(),
        });
    }
}

/// 沿驱动器回溯，判断该网络上的时钟是否来自 Clock 组件（允许经过 Buffer 链）。
/// 回溯步数有上界，避免畸形电路把这里变成死循环。
fn clock_from_source(
    board: &Board,
    nl: &Netlist,
    net_driver: &HashMap<u32, u32>,
    start: u32,
) -> bool {
    let mut net = start;
    for _ in 0..16 {
        let Some(&d) = net_driver.get(&net) else {
            return false;
        };
        let Some(inst) = board.instances.get(d as usize) else {
            return false;
        };
        if inst.def == DefId::Clock {
            return true;
        }
        if inst.def != DefId::Buffer {
            return false;
        }
        // Buffer 的输入端（pins 布局保证输入在前）
        let base = nl.pin_start[d as usize] as usize;
        net = nl.pin_net[base];
        if net == NO_NET {
            return false;
        }
    }
    false
}

/// 关键路径：最长的组合逻辑路径（v4 §9.5）
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CriticalPath {
    /// 路径上的实例，从输入侧到输出侧
    pub components: Vec<u32>,
    /// 延迟，单位 tick（等于路径上的组件数）
    pub ticks: u32,
}

/// 求关键路径；电路含环时返回 None（此时 DRC 已报组合环，路径无意义）。
pub fn critical_path(board: &Board) -> Option<CriticalPath> {
    let n = board.instances.len();
    if n == 0 {
        return None;
    }
    let nl = board.compile();
    let adj = component_graph(board, &nl);

    // 拓扑排序（Kahn）；排不完说明有环
    let mut indeg = vec![0usize; n];
    for v in 0..n {
        for &w in &adj[v] {
            indeg[w as usize] += 1;
        }
    }
    let mut order = Vec::with_capacity(n);
    let mut stack: Vec<u32> = (0..n as u32).filter(|&v| indeg[v as usize] == 0).collect();
    while let Some(v) = stack.pop() {
        order.push(v);
        for &w in &adj[v as usize] {
            indeg[w as usize] -= 1;
            if indeg[w as usize] == 0 {
                stack.push(w);
            }
        }
    }
    if order.len() != n {
        return None;
    }

    // 最长路径 DP：best[v] = 以 v 结尾的最长链长度
    let mut best = vec![1u32; n];
    let mut prev = vec![u32::MAX; n];
    for &v in &order {
        for &w in &adj[v as usize] {
            if best[v as usize] + 1 > best[w as usize] {
                best[w as usize] = best[v as usize] + 1;
                prev[w as usize] = v;
            }
        }
    }

    let end = best
        .iter()
        .enumerate()
        .max_by_key(|(_, &d)| d)
        .map(|(i, _)| i)?;
    let mut path = Vec::new();
    let mut cur = end as u32;
    while cur != u32::MAX {
        path.push(cur);
        cur = prev[cur as usize];
    }
    path.reverse();
    Some(CriticalPath { ticks: best[end], components: path })
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
    fn gated_clock_is_warned() {
        let mut b = Board::new();
        let sw = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
        let g = b.add_instance(DefId::And, Params::default().width(1).inputs(2), 10, 0);
        let r = b.add_instance(DefId::Register, Params::default().width(1).opts(0), 20, 0);
        assert!(b.connect_pins((sw, 0), (g, 0)));
        assert!(b.connect_pins((g, 2), (r, 1)), "用逻辑门驱动时钟脚");
        let issues = check(&b);
        assert!(
            issues.iter().any(|i| i.kind == IssueKind::GatedClock),
            "门控时钟应当被警告: {issues:?}"
        );
    }

    #[test]
    fn clock_component_driving_register_is_fine() {
        let mut b = Board::new();
        let clk = b.add_instance(DefId::Clock, Params::default().width(1), 0, 0);
        let r = b.add_instance(DefId::Register, Params::default().width(1).opts(0), 10, 0);
        assert!(b.connect_pins((clk, 0), (r, 1)));
        assert!(!check(&b).iter().any(|i| i.kind == IssueKind::GatedClock));
    }

    /// 时钟树上插缓冲器是正常做法，不该被当成门控时钟
    #[test]
    fn buffer_chain_does_not_trigger_gated_clock() {
        let mut b = Board::new();
        let clk = b.add_instance(DefId::Clock, Params::default().width(1), 0, 0);
        let buf = b.add_instance(DefId::Buffer, Params::default().width(1), 10, 0);
        let r = b.add_instance(DefId::Register, Params::default().width(1).opts(0), 20, 0);
        assert!(b.connect_pins((clk, 0), (buf, 0)));
        assert!(b.connect_pins((buf, 1), (r, 1)));
        assert!(!check(&b).iter().any(|i| i.kind == IssueKind::GatedClock));
    }

    #[test]
    fn critical_path_finds_longest_chain() {
        let mut b = Board::new();
        let sw = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
        let n1 = b.add_instance(DefId::Not, Params::default().width(1), 10, 0);
        let n2 = b.add_instance(DefId::Not, Params::default().width(1), 20, 0);
        let n3 = b.add_instance(DefId::Not, Params::default().width(1), 30, 0);
        let led = b.add_instance(DefId::Led, Params::default().width(1), 40, 0);
        assert!(b.connect_pins((sw, 0), (n1, 0)));
        assert!(b.connect_pins((n1, 1), (n2, 0)));
        assert!(b.connect_pins((n2, 1), (n3, 0)));
        assert!(b.connect_pins((n3, 1), (led, 0)));

        let cp = critical_path(&b).expect("无环电路应能求出关键路径");
        assert_eq!(cp.ticks, 4, "开关 + 3 级 NOT，路径长 4");
        assert_eq!(cp.components.len(), 4);
        assert_eq!(cp.components[0], sw, "路径应从输入侧开始");
    }

    #[test]
    fn critical_path_returns_none_on_cycle() {
        let mut b = Board::new();
        let p = Params::default().width(1);
        let g1 = b.add_instance(DefId::Not, p, 0, 0);
        let g2 = b.add_instance(DefId::Not, p, 10, 0);
        assert!(b.connect_pins((g1, 1), (g2, 0)));
        assert!(b.connect_pins((g2, 1), (g1, 0)));
        assert!(critical_path(&b).is_none(), "有环时不该给出关键路径");
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
