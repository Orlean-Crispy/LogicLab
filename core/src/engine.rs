//! 仿真引擎（v3 §5.1 / ADR-1 / ADR-2 / ADR-8）
//!
//! # 语义（钉死）
//!
//! ```text
//! tick():
//!   阶段 A  对每个脏组件，用【本拍开始时的网值】求值 → 写入暂存区
//!   阶段 B  原子提交暂存区 → 网络值，记录变化的网络
//!   阶段 C  tick += 1（下一拍的脏集合由本拍变化的网络推出）
//! ```
//!
//! - 所有组件输出**一律延迟 1 tick**（含触发器：时钟沿在 t 被读到，输出在 t+1 出现）
//! - 导线零延迟、同一网络同值
//! - 未连接的输入读作 0（ADR-5）
//! - 确定性：脏集合由网络变化推出，与实例遍历顺序无关（ADR-8）
//!
//! # 性能
//!
//! - 热路径（`tick`）**零内存分配**：所有缓冲在 `load_board` 时按需分配
//! - 事件驱动 + 脏标记：输入未变的组件直接跳过求值
//! - 邻接表为 CSR 布局，缓存友好；不做指针追逐，不做 HashMap 查找

use crate::board::{Board, NO_NET};
use crate::defs::{eval, CompState, DefId, Params};
use crate::elaborate::Flat;
use crate::values::{NetValue, Width};

/// 实例无对应可求值组件（如探针）
pub const NO_COMP: u32 = u32::MAX;

/// 可求值组件的运行时描述（紧凑、无指针）
#[derive(Clone, Debug)]
struct CompRt {
    def: DefId,
    params: Params,
    /// 全局引脚偏移
    in_start: u32,
    in_count: u16,
    out_start: u32,
    out_count: u16,
    /// 自由运行（每拍必算），仅时钟源
    always: bool,
    /// 对应的板实例索引
    inst: u32,
}

/// 离散 tick 仿真引擎
pub struct Engine {
    comps: Vec<CompRt>,
    states: Vec<CompState>,
    /// 板实例索引 → 组件索引（NO_COMP = 无）
    comp_of_inst: Vec<u32>,
    /// 全局引脚 → 网络
    pin_net: Vec<u32>,
    net_val: Vec<NetValue>,
    net_width: Vec<Width>,
    // 接收端邻接（CSR）：网络 → 读取该网络的组件
    rx_off: Vec<u32>,
    rx: Vec<u32>,
    // 脏标记（时间戳法，避免每拍清空）
    stamp: u32,
    dirty_stamp: Vec<u32>,
    dirty: Vec<u32>,
    /// 外部（UI）强制标脏的组件
    forced: Vec<u32>,
    // 双缓冲暂存
    staging: Vec<NetValue>,
    staged: Vec<u32>,
    changed: Vec<u32>,
    // 热路径复用缓冲
    scratch_in: Vec<NetValue>,
    scratch_out: Vec<NetValue>,
    // 诊断
    last_eval: u32,
    total_eval: u64,
    primed: bool,
    tick: u64,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            comps: Vec::new(),
            states: Vec::new(),
            comp_of_inst: Vec::new(),
            pin_net: Vec::new(),
            net_val: Vec::new(),
            net_width: Vec::new(),
            rx_off: vec![0],
            rx: Vec::new(),
            stamp: 0,
            dirty_stamp: Vec::new(),
            dirty: Vec::new(),
            forced: Vec::new(),
            staging: Vec::new(),
            staged: Vec::new(),
            changed: Vec::new(),
            scratch_in: Vec::new(),
            scratch_out: Vec::new(),
            last_eval: 0,
            total_eval: 0,
            primed: false,
            tick: 0,
        }
    }

    // -----------------------------------------------------------------------
    // 装载
    // -----------------------------------------------------------------------

    /// 由电路板重建运行结构（单图纸，供单元测试与工具使用）
    pub fn load_board(&mut self, board: &Board) {
        let nl = board.compile();
        let mut flat = Flat::single(board, &nl);
        self.load_flat(&mut flat);
    }

    /// 由**已展开的**扁平网表重建运行结构。
    ///
    /// flat.states 会被取走而不是复制——内存型组件（RAM / 显示屏）的状态
    /// 可能有几十万个字，复制一遍纯属浪费。
    pub fn load_flat(&mut self, flat: &mut Flat) {
        self.comps.clear();
        self.states.clear();
        self.comp_of_inst = vec![NO_COMP; flat.inst_count as usize];

        for (ci, fc) in flat.comps.iter().enumerate() {
            self.comp_of_inst[fc.global_inst as usize] = ci as u32;
            self.comps.push(CompRt {
                def: fc.def,
                params: fc.params,
                in_start: fc.pin_start,
                in_count: fc.in_count,
                out_start: fc.pin_start + fc.in_count as u32,
                out_count: fc.out_count,
                always: fc.def == DefId::Clock,
                inst: fc.global_inst,
            });
        }
        self.states = std::mem::take(&mut flat.states);

        // ---- 网络值 / 位宽 ----
        let np = flat.pin_net.len();
        self.net_val.clear();
        self.net_val.resize(flat.net_count as usize, NetValue::ZERO);
        self.net_width.clear();
        self.net_width.resize(flat.net_count as usize, 1);
        for (p, &n) in flat.pin_net.iter().enumerate() {
            if n == NO_NET {
                continue;
            }
            let w = flat.pin_width[p];
            if w > self.net_width[n as usize] {
                self.net_width[n as usize] = w;
            }
        }
        self.pin_net = flat.pin_net.clone();

        // ---- 接收端 CSR ----
        let nets = flat.net_count as usize;
        let mut off = vec![0u32; nets + 1];
        for c in &self.comps {
            for k in 0..c.in_count as usize {
                let n = self.pin_net[c.in_start as usize + k];
                if n != NO_NET {
                    off[n as usize + 1] += 1;
                }
            }
        }
        for i in 0..nets {
            off[i + 1] += off[i];
        }
        let mut rx = vec![0u32; off[nets] as usize];
        let mut fill: Vec<u32> = off.clone();
        for (ci, c) in self.comps.iter().enumerate() {
            for k in 0..c.in_count as usize {
                let n = self.pin_net[c.in_start as usize + k];
                if n != NO_NET {
                    rx[fill[n as usize] as usize] = ci as u32;
                    fill[n as usize] += 1;
                }
            }
        }
        self.rx_off = off;
        self.rx = rx;

        // ---- 热路径缓冲（此后 tick 不再分配）----
        let max_in = self.comps.iter().map(|c| c.in_count as usize).max().unwrap_or(0);
        let max_out = self.comps.iter().map(|c| c.out_count as usize).max().unwrap_or(0);
        self.scratch_in = vec![NetValue::ZERO; max_in];
        self.scratch_out = vec![NetValue::ZERO; max_out];
        self.staging = vec![NetValue::ZERO; np];
        self.staged.clear();
        self.staged.reserve(np);
        self.changed.clear();
        self.changed.reserve(nets);
        self.dirty.clear();
        self.dirty.reserve(self.comps.len());
        self.dirty_stamp = vec![0u32; self.comps.len()];
        self.forced.clear();

        self.stamp = 0;
        self.primed = false;
        self.last_eval = 0;
    }

    // -----------------------------------------------------------------------
    // 仿真
    // -----------------------------------------------------------------------

    #[inline]
    fn mark_dirty(&mut self, ci: u32, stamp: u32) {
        if self.dirty_stamp[ci as usize] != stamp {
            self.dirty_stamp[ci as usize] = stamp;
            self.dirty.push(ci);
        }
    }

    /// 推进一拍
    pub fn tick(&mut self) {
        if self.comps.is_empty() {
            self.tick += 1;
            return;
        }

        // ---- 组装本拍脏集合 ----
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.dirty_stamp.fill(0);
            self.stamp = 1;
        }
        let stamp = self.stamp;
        self.dirty.clear();

        if self.primed {
            // 上一拍变化的网络 → 读它的组件
            for i in 0..self.changed.len() {
                let n = self.changed[i] as usize;
                for k in self.rx_off[n] as usize..self.rx_off[n + 1] as usize {
                    let ci = self.rx[k];
                    self.mark_dirty(ci, stamp);
                }
            }
            // 自由运行源
            for ci in 0..self.comps.len() {
                if self.comps[ci].always {
                    self.mark_dirty(ci as u32, stamp);
                }
            }
        } else {
            // 首个 tick：上电全 0，所有组件各算一次
            for ci in 0..self.comps.len() as u32 {
                self.mark_dirty(ci, stamp);
            }
            self.primed = true;
        }

        // UI 强制（开关 / 常量改动）
        for i in 0..self.forced.len() {
            let ci = self.forced[i];
            self.mark_dirty(ci, stamp);
        }
        self.forced.clear();

        // ---- 阶段 A：读旧值求值 ----
        self.staged.clear();
        self.changed.clear();

        {
            let comps = &self.comps;
            let pin_net = &self.pin_net;
            let net_val = &self.net_val;
            let states = &mut self.states;
            let scratch_in = &mut self.scratch_in;
            let scratch_out = &mut self.scratch_out;
            let staging = &mut self.staging;
            let staged = &mut self.staged;
            let dirty = &self.dirty;

            for di in 0..dirty.len() {
                let ci = dirty[di] as usize;
                let c = &comps[ci];
                let ni = c.in_count as usize;
                let no = c.out_count as usize;

                for k in 0..ni {
                    let n = pin_net[c.in_start as usize + k];
                    scratch_in[k] = if n == NO_NET { NetValue::ZERO } else { net_val[n as usize] };
                }
                eval(
                    c.def,
                    &c.params,
                    &scratch_in[..ni],
                    &mut states[ci],
                    &mut scratch_out[..no],
                );
                for k in 0..no {
                    let p = c.out_start as usize + k;
                    staging[p] = scratch_out[k];
                    staged.push(p as u32);
                }
            }
            self.last_eval = dirty.len() as u32;
            self.total_eval += dirty.len() as u64;
        }

        // ---- 阶段 B：原子提交 ----
        {
            let pin_net = &self.pin_net;
            let net_width = &self.net_width;
            let staging = &self.staging;
            let staged = &self.staged;

            for i in 0..staged.len() {
                let p = staged[i] as usize;
                let n = pin_net[p];
                if n == NO_NET {
                    continue;
                }
                let n = n as usize;
                let w = net_width[n];
                let v = staging[p].with_width(w);
                if self.net_val[n].differs(v, w) {
                    self.net_val[n] = v;
                    self.changed.push(n as u32);
                }
            }
        }

        self.tick += 1;
    }

    /// 快进 n 拍（M3+ 会挪到工作线程）
    pub fn run_for(&mut self, n: u64) {
        for _ in 0..n {
            self.tick();
        }
    }

    /// 改输入端（开关 / 按钮 / 常量），下一拍生效
    pub fn set_input(&mut self, inst: u32, value: u32) {
        let Some(&ci) = self.comp_of_inst.get(inst as usize) else {
            return;
        };
        if ci == NO_COMP {
            return;
        }
        let c = &mut self.comps[ci as usize];
        if !c.def.is_source() {
            return;
        }
        let v = value & crate::values::width_mask(c.params.width);
        if c.params.value != v {
            c.params.value = v;
            self.forced.push(ci);
        }
    }

    /// 复位仿真（清空状态与网络值，保留电路）
    pub fn reset(&mut self) {
        for st in &mut self.states {
            let keep = std::mem::take(&mut st.mem);
            *st = CompState::default();
            st.mem = keep;
            if !st.mem.is_empty() {
                st.mem.fill(0);
            }
        }
        self.net_val.fill(NetValue::ZERO);
        self.changed.clear();
        self.staged.clear();
        self.forced.clear();
        self.stamp = 0;
        self.dirty_stamp.fill(0);
        self.primed = false;
        self.tick = 0;
        self.last_eval = 0;
        self.total_eval = 0;
    }

    // -----------------------------------------------------------------------
    // 读取
    // -----------------------------------------------------------------------

    #[inline]
    pub fn net_value(&self, net: u32) -> NetValue {
        self.net_val.get(net as usize).copied().unwrap_or(NetValue::ZERO)
    }

    #[inline]
    pub fn net_width(&self, net: u32) -> Width {
        self.net_width.get(net as usize).copied().unwrap_or(1)
    }

    /// 引脚上的值（未连接 → 0）
    #[inline]
    pub fn pin_value(&self, pin: u32) -> NetValue {
        match self.pin_net.get(pin as usize) {
            Some(&n) if n != NO_NET => self.net_value(n),
            _ => NetValue::ZERO,
        }
    }

    /// 引脚所属网络
    #[inline]
    pub fn net_of_pin(&self, pin: u32) -> u32 {
        self.pin_net.get(pin as usize).copied().unwrap_or(NO_NET)
    }

    /// 本拍发生变化的网络（自上次 tick 起）
    pub fn changed_nets(&self) -> &[u32] {
        &self.changed
    }

    pub fn tick_count(&self) -> u64 {
        self.tick
    }

    pub fn component_count(&self) -> usize {
        self.comps.len()
    }

    pub fn net_count(&self) -> usize {
        self.net_val.len()
    }

    pub fn pin_count(&self) -> usize {
        self.pin_net.len()
    }

    /// 组件索引（引擎内部编号）→ 板实例索引
    pub fn inst_of_comp(&self, ci: usize) -> Option<u32> {
        self.comps.get(ci).map(|c| c.inst)
    }

    /// 上一拍实际求值的组件数（脏传播有效性诊断）
    pub fn last_eval_count(&self) -> u32 {
        self.last_eval
    }

    pub fn total_eval_count(&self) -> u64 {
        self.total_eval
    }

    /// 某实例的存储器内容（RAM / ROM / 显示屏帧缓冲）
    #[inline]
    pub fn instance_mem(&self, inst: u32) -> &[u32] {
        let Some(&ci) = self.comp_of_inst.get(inst as usize) else {
            return &[];
        };
        if ci == NO_COMP {
            return &[];
        }
        &self.states[ci as usize].mem
    }

    /// 某实例的输出引脚当前值（UI 探针用）
    pub fn instance_output(&self, inst: u32, slot: usize) -> NetValue {
        let Some(&ci) = self.comp_of_inst.get(inst as usize) else {
            return NetValue::ZERO;
        };
        if ci == NO_COMP {
            return NetValue::ZERO;
        }
        let c = &self.comps[ci as usize];
        if slot >= c.out_count as usize {
            return NetValue::ZERO;
        }
        self.pin_value(c.out_start + slot as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::{DefId, Params, OPT_CARRY};
    use crate::values::Bit;

    /// 连线：走 core 的自动避让（正交折线的拐点压到引脚会按 ADR-7 误连）
    fn connect(b: &mut Board, from: (u32, usize), to: (u32, usize)) {
        assert!(b.connect_pins(from, to), "连线失败: {from:?} -> {to:?}");
    }

    #[test]
    fn nand_gate_propagates_in_one_tick() {
        let mut b = Board::new();
        let a = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
        let c = b.add_instance(DefId::Switch, Params::default().width(1), 0, 2);
        let g = b.add_instance(DefId::Nand, Params::default().width(1).inputs(2), 4, 0);
        let out = b.add_instance(DefId::Led, Params::default().width(1), 8, 0);
        connect(&mut b, (a, 0), (g, 0));
        connect(&mut b, (c, 0), (g, 1));
        connect(&mut b, (g, 2), (out, 0));

        let mut e = Engine::new();
        e.load_board(&b);
        e.set_input(a, 1);
        e.set_input(c, 1);
        e.tick();
        // 首拍所有组件同时读"上电全 0"：NAND(0,0)=1，开关的同拍新值要下一拍才被读到
        assert_eq!(e.instance_output(g, 0).get(1), 1, "首拍 NAND 读到的是上电的 0");
        e.tick();
        assert_eq!(e.instance_output(g, 0).get(1), 0, "次拍读到 A=1,B=1 → 输出 0");
        e.set_input(c, 0);
        e.tick();
        assert_eq!(e.instance_output(g, 0).get(1), 0, "输入变化当拍不传播");
        e.tick();
        assert_eq!(e.instance_output(g, 0).get(1), 1, "1 tick 后 NAND 输出翻转");
    }

    /// 低有效 NAND 锁存器：反馈环路合法，且有确定的收敛路径。
    ///
    /// 注：NOR 交叉耦合锁存器上电全 0 时处于"两输出皆 0"的非法态，在 1 tick 门延迟
    /// 模型下会自持振荡——这是规格 §5.1 明确允许的行为（UI 应给警告而非阻止），
    /// 所以这里用可确定的低有效版本做断言。
    #[test]
    fn nand_sr_latch_holds_state() {
        let mut b = Board::new();
        let sbar = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
        let rbar = b.add_instance(DefId::Switch, Params::default().width(1), 0, 6);
        let p = Params::default().width(1).inputs(2);
        let g1 = b.add_instance(DefId::Nand, p, 6, 0);
        let g2 = b.add_instance(DefId::Nand, p, 6, 6);
        // Q = g1.Y = NAND(Sbar, Qbar)，Qbar = g2.Y = NAND(Rbar, Q)
        connect(&mut b, (sbar, 0), (g1, 0));
        connect(&mut b, (g2, 2), (g1, 1));
        connect(&mut b, (rbar, 0), (g2, 0));
        connect(&mut b, (g1, 2), (g2, 1));

        let mut e = Engine::new();
        e.load_board(&b);
        e.set_input(sbar, 0); // 置位有效
        e.set_input(rbar, 1); // 复位无效
        e.run_for(6);
        assert_eq!(e.instance_output(g1, 0).get(1), 1, "置位后 Q=1");
        assert_eq!(e.instance_output(g2, 0).get(1), 0, "互补输出 Qbar=0");

        // 松开置位 → 自锁保持
        e.set_input(sbar, 1);
        e.run_for(6);
        assert_eq!(e.instance_output(g1, 0).get(1), 1, "自锁保持 Q=1");
        assert_eq!(e.instance_output(g2, 0).get(1), 0, "自锁保持 Qbar=0");

        // 拉低复位
        e.set_input(rbar, 0);
        e.run_for(6);
        assert_eq!(e.instance_output(g1, 0).get(1), 0, "复位后 Q=0");
        assert_eq!(e.instance_output(g2, 0).get(1), 1, "复位后 Qbar=1");
    }

    /// 环形振荡器：3 个反相器自持振荡（§5.1 反馈环路合法）。
    ///
    /// 注意实测结果是**周期 2**而不是"信号沿环路走一圈"的周期 6：上电全 0 是对称初态，
    /// 双缓冲下三级同时翻转，全网同步振荡。这是模型的正确行为，也是 UI 该报警的情形。
    #[test]
    fn ring_oscillator_oscillates() {
        let mut b = Board::new();
        let g1 = b.add_instance(DefId::Not, Params::default().width(1), 0, 0);
        let g2 = b.add_instance(DefId::Not, Params::default().width(1), 4, 0);
        let g3 = b.add_instance(DefId::Not, Params::default().width(1), 8, 0);
        connect(&mut b, (g1, 1), (g2, 0));
        connect(&mut b, (g2, 1), (g3, 0));
        connect(&mut b, (g3, 1), (g1, 0));

        let mut e = Engine::new();
        e.load_board(&b);
        let mut seq = Vec::new();
        for _ in 0..8 {
            e.tick();
            seq.push(e.instance_output(g1, 0).get(1));
        }
        assert_eq!(seq, vec![1, 0, 1, 0, 1, 0, 1, 0], "对称初态下全网每拍同步翻转");
    }

    /// 移位寄存器电路：时钟 → 两级寄存器 → 开关喂数据
    fn shifter_board() -> Board {
        let mut b = Board::new();
        let clk = b.add_instance(DefId::Clock, Params::default().width(1), 0, 0);
        let d = b.add_instance(DefId::Switch, Params::default().width(1), 0, 4);
        let r1 = b.add_instance(DefId::Register, Params::default().width(1).opts(0), 4, 0);
        let r2 = b.add_instance(DefId::Register, Params::default().width(1).opts(0), 8, 0);
        connect(&mut b, (clk, 0), (r1, 1));
        connect(&mut b, (d, 0), (r1, 0));
        connect(&mut b, (clk, 0), (r2, 1));
        connect(&mut b, (r1, 2), (r2, 0));
        b
    }

    /// 按**坐标**取某个元件的输出——与实例在数组中的顺序无关
    fn out_at(b: &Board, e: &Engine, x: i32, y: i32, slot: usize) -> u32 {
        let idx = b
            .instances
            .iter()
            .position(|it| it.x == x && it.y == y)
            .expect("找不到该位置的元件");
        e.instance_output(idx as u32, slot).get(1)
    }

    /// 逐拍波形（按坐标采样，与实例顺序解耦）
    fn trace(b: &Board, shuffle: bool) -> Vec<(u32, u32)> {
        let mut board = shifter_board();
        if shuffle {
            // 打乱实例顺序：网络拓扑由坐标推导，语义必须不变（ADR-8）
            board.instances.reverse();
        }
        let mut e = Engine::new();
        e.load_board(&board);
        let (dx, dy) = (0, 4);
        let mut out = Vec::new();
        for i in 0..40 {
            if i == 5 {
                e.set_input(
                    board.instances.iter().position(|it| it.x == dx && it.y == dy).unwrap() as u32,
                    1,
                );
            }
            if i == 25 {
                e.set_input(
                    board.instances.iter().position(|it| it.x == dx && it.y == dy).unwrap() as u32,
                    0,
                );
            }
            e.tick();
            out.push((out_at(&board, &e, 4, 0, 0), out_at(&board, &e, 8, 0, 0)));
        }
        let _ = b;
        out
    }

    #[test]
    fn same_circuit_replays_identically() {
        let a = trace(&shifter_board(), false);
        let b = trace(&shifter_board(), false);
        assert_eq!(a, b, "同一电路同一输入序列必须逐拍可复现（ADR-8）");
        assert!(a.iter().any(|&(q1, _)| q1 == 1), "波形不应全 0");
    }

    #[test]
    fn instance_order_does_not_affect_behaviour() {
        let normal = trace(&shifter_board(), false);
        let shuffled = trace(&shifter_board(), true);
        assert_eq!(normal, shuffled, "实例遍历顺序不得影响结果（ADR-8）");
    }

    #[test]
    fn dirty_propagation_skips_idle_components() {
        let mut b = Board::new();
        let sw = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
        let g = b.add_instance(DefId::And, Params::default().width(1).inputs(2), 4, 0);
        connect(&mut b, (sw, 0), (g, 0));

        let mut e = Engine::new();
        e.load_board(&b);
        e.tick();
        assert_eq!(e.last_eval_count(), 2, "首拍全算");
        e.tick();
        e.tick();
        assert_eq!(e.last_eval_count(), 0, "无变化时应零求值");

        e.set_input(sw, 1);
        e.tick();
        assert_eq!(e.last_eval_count(), 1, "只有被改动的开关重算");
        e.tick();
        assert_eq!(e.last_eval_count(), 1, "变化传播到 AND");
        e.tick();
        assert_eq!(e.last_eval_count(), 0, "传播完成后重新静默");
    }

    #[test]
    fn clock_drives_register_chain() {
        let mut b = Board::new();
        let clk = b.add_instance(DefId::Clock, Params::default().width(1), 0, 0);
        let d = b.add_instance(DefId::Switch, Params::default().width(1), 0, 4);
        let r1 = b.add_instance(DefId::Register, Params::default().width(1).opts(0), 4, 0);
        let r2 = b.add_instance(DefId::Register, Params::default().width(1).opts(0), 8, 0);
        connect(&mut b, (clk, 0), (r1, 1));
        connect(&mut b, (d, 0), (r1, 0));
        connect(&mut b, (clk, 0), (r2, 1));
        connect(&mut b, (r1, 2), (r2, 0));

        let mut e = Engine::new();
        e.load_board(&b);
        e.set_input(d, 1);
        e.run_for(40);
        // D=1 持续输入，两个寄存器应被推成 1
        assert_eq!(e.instance_output(r1, 0).get(1), 1);
        assert_eq!(e.instance_output(r2, 0).get(1), 1);
    }

    #[test]
    fn multi_bit_adder_end_to_end() {
        let mut b = Board::new();
        let a = b.add_instance(DefId::Constant, Params { width: 8, value: 200, ..Default::default() }, 0, 0);
        let c = b.add_instance(DefId::Constant, Params { width: 8, value: 100, ..Default::default() }, 0, 4);
        let add = b.add_instance(DefId::Adder, Params::default().width(8).opts(OPT_CARRY), 4, 0);
        connect(&mut b, (a, 0), (add, 0));
        connect(&mut b, (c, 0), (add, 1));

        let mut e = Engine::new();
        e.load_board(&b);
        e.run_for(3);
        assert_eq!(e.instance_output(add, 0).get(8), 44, "200+100 回绕到 44");
        assert_eq!(e.instance_output(add, 1).get(1), 1, "进位为 1");
    }

    #[test]
    fn unconnected_input_reads_zero() {
        let mut b = Board::new();
        let g = b.add_instance(DefId::And, Params::default().width(1).inputs(2), 0, 0);
        let mut e = Engine::new();
        e.load_board(&b);
        e.run_for(3);
        assert_eq!(e.instance_output(g, 0).get(1), 0);
    }

    #[test]
    fn nets_are_derived_from_wires_and_values_flow() {
        let mut b = Board::new();
        let sw = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
        let buf = b.add_instance(DefId::Buffer, Params::default().width(1), 6, 0);
        connect(&mut b, (sw, 0), (buf, 0));
        let mut e = Engine::new();
        e.load_board(&b);
        assert_ne!(e.net_of_pin(0), NO_NET);
        assert_eq!(e.net_of_pin(0), e.net_of_pin(1), "开关输出与缓冲器输入必须同网络");
        e.set_input(sw, 1);
        e.run_for(3);
        assert_eq!(e.instance_output(buf, 0).get(1), 1);
    }

    #[test]
    fn reset_returns_to_power_on_state() {
        let mut b = Board::new();
        let sw = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
        let buf = b.add_instance(DefId::Buffer, Params::default().width(1), 6, 0);
        connect(&mut b, (sw, 0), (buf, 0));
        let mut e = Engine::new();
        e.load_board(&b);
        e.set_input(sw, 1);
        e.run_for(3);
        assert_eq!(e.instance_output(buf, 0).bit(0), Bit::One);
        e.reset();
        assert_eq!(e.tick_count(), 0);
        assert_eq!(e.instance_output(buf, 0).get(1), 0);
    }

    #[test]
    fn ram_round_trip_through_engine() {
        let mut b = Board::new();
        let addr = b.add_instance(DefId::Constant, Params { width: 4, value: 5, ..Default::default() }, 0, 0);
        let din = b.add_instance(DefId::Constant, Params { width: 8, value: 0x5A, ..Default::default() }, 0, 4);
        let we = b.add_instance(DefId::Switch, Params::default().width(1), 0, 8);
        let clk = b.add_instance(DefId::Clock, Params::default().width(1), 0, 12);
        let ram = b.add_instance(DefId::Ram, Params::default().width(8).depth(16), 6, 0);
        connect(&mut b, (addr, 0), (ram, 0));
        connect(&mut b, (din, 0), (ram, 1));
        connect(&mut b, (we, 0), (ram, 2));
        connect(&mut b, (clk, 0), (ram, 3));

        let mut e = Engine::new();
        e.load_board(&b);
        e.set_input(we, 1);
        e.run_for(10);
        e.set_input(we, 0);
        e.run_for(4);
        assert_eq!(e.instance_output(ram, 0).get(8), 0x5A);
    }
}
