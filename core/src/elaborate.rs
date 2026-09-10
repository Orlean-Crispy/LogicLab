//! 层次展开：把多张图纸压成一张扁平网表（ADR-28）
//!
//! 仿真热路径只认 Flat：里面全是内置元件，没有层次、没有分支、没有指针追逐。
//! 层次只活在编辑期——展开一次，之后每拍都是线性扫描。
//!
//! 展开规则（钉死）：
//!   - 子电路外壳不产生运行时组件；它的引脚与图纸接口引脚并成同一个网络
//!   - 接口引脚按元件名排序，同名按实例索引（顺序稳定、可预测）
//!   - 同一张图纸实例化多次就展开多份，互不共享状态
//!   - 自引用 / 超过 MAX_DEPTH 一律截断并置 truncated，绝不递归爆栈

use crate::board::{Board, Netlist, UnionFind, NO_NET, NO_SUB};
use crate::defs::{CompState, DefId, Dir, PinDef};
use crate::values::Width;

/// 层次深度上限（自引用与误接的保护）
pub const MAX_DEPTH: usize = 32;

/// 图纸接口：子电路对外的引脚清单
#[derive(Clone, Debug, Default)]
pub struct Shape {
    pub inputs: Vec<u32>,
    pub outputs: Vec<u32>,
}

impl Shape {
    pub fn len(&self) -> usize {
        self.inputs.len() + self.outputs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty() && self.outputs.is_empty()
    }
}

/// 由图纸推导接口。按元件名排序，保证同一张图纸每次都得到同样的引脚顺序。
pub fn shape(board: &Board) -> Shape {
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for (i, inst) in board.instances.iter().enumerate() {
        match inst.def {
            DefId::InputPin => inputs.push(i as u32),
            DefId::OutputPin => outputs.push(i as u32),
            _ => {}
        }
    }
    for v in [&mut inputs, &mut outputs] {
        v.sort_by(|a, b| {
            let ka = &board.instances[*a as usize].display_name;
            let kb = &board.instances[*b as usize].display_name;
            ka.cmp(kb).then(a.cmp(b))
        });
    }
    Shape { inputs, outputs }
}

/// 自定义元件的引脚布局：输入贴左边自上而下，输出贴右边自上而下
pub fn custom_pins(board: &Board) -> Vec<PinDef> {
    let s = shape(board);
    let mut v = Vec::with_capacity(s.len());
    for (k, &i) in s.inputs.iter().enumerate() {
        let inst = &board.instances[i as usize];
        v.push(PinDef {
            name: inst.display_name.clone(),
            dx: 0,
            dy: k as i32,
            width: inst.params.width,
            dir: Dir::In,
        });
    }
    for (k, &i) in s.outputs.iter().enumerate() {
        let inst = &board.instances[i as usize];
        v.push(PinDef {
            name: inst.display_name.clone(),
            dx: 4,
            dy: k as i32,
            width: inst.params.width,
            dir: Dir::Out,
        });
    }
    v
}

/// 展开后的一个可求值组件
#[derive(Clone, Debug)]
pub struct FlatComp {
    pub def: DefId,
    pub params: crate::defs::Params,
    /// 所属图纸
    pub board: u32,
    /// 在该图纸内的实例索引
    pub inst: u32,
    /// 展开后的全局实例索引
    pub global_inst: u32,
    /// 全局引脚起点
    pub pin_start: u32,
    pub in_count: u16,
    pub out_count: u16,
}

/// 扁平网表：引擎唯一消费的形态
#[derive(Clone, Debug, Default)]
pub struct Flat {
    pub comps: Vec<FlatComp>,
    /// 与 comps 平行；load_flat 会把它取走（不复制）
    pub states: Vec<CompState>,
    pub pin_net: Vec<u32>,
    pub pin_width: Vec<Width>,
    pub pin_is_out: Vec<bool>,
    /// 全局实例 → 首个全局引脚
    pub pin_start: Vec<u32>,
    pub inst_count: u32,
    pub net_count: u32,
    /// 图纸 → 局部网络到全局网络的映射（取该图纸第一次实例化的那一份）
    pub net_map: Vec<Vec<u32>>,
    /// 图纸 → 局部实例到全局实例的映射（同上）
    pub inst_map: Vec<Vec<u32>>,
    /// 有引用被截断（自引用 / 超深 / 越界）
    pub truncated: bool,
}

impl Flat {
    /// 单张图纸的平凡展开：没有层次，网络编号原样不变
    pub fn single(board: &Board, nl: &Netlist) -> Self {
        let mut comps = Vec::new();
        let mut states = Vec::new();
        for (ii, inst) in board.instances.iter().enumerate() {
            if inst.def.is_passive() {
                continue;
            }
            let ins = inst.pins.iter().filter(|p| p.dir == Dir::In).count();
            comps.push(FlatComp {
                def: inst.def,
                params: inst.params,
                board: 0,
                inst: ii as u32,
                global_inst: ii as u32,
                pin_start: nl.pin_start[ii],
                in_count: ins as u16,
                out_count: (inst.pins.len() - ins) as u16,
            });
            states.push(inst.initial_state());
        }
        let nets = nl.net_count as usize;
        Self {
            comps,
            states,
            pin_net: nl.pin_net.clone(),
            pin_width: nl.pin_width.clone(),
            pin_is_out: nl.pin_is_out.clone(),
            pin_start: nl.pin_start.clone(),
            inst_count: board.instances.len() as u32,
            net_count: nl.net_count,
            net_map: vec![(0..nets as u32).collect()],
            inst_map: vec![(0..board.instances.len() as u32).collect()],
            truncated: false,
        }
    }

    /// 图纸局部网络 → 全局网络
    pub fn global_net(&self, board: u32, local: u32) -> u32 {
        if local == NO_NET {
            return NO_NET;
        }
        self.net_map
            .get(board as usize)
            .and_then(|m| m.get(local as usize))
            .copied()
            .unwrap_or(NO_NET)
    }

    /// 图纸局部实例 → 全局实例（超出返回 u32::MAX）
    pub fn global_inst(&self, board: u32, inst: u32) -> u32 {
        self.inst_map
            .get(board as usize)
            .and_then(|m| m.get(inst as usize))
            .copied()
            .unwrap_or(u32::MAX)
    }
}

// ---------------------------------------------------------------------------
// 展开
// ---------------------------------------------------------------------------

struct Builder<'a> {
    boards: &'a [Board],
    nls: &'a [Netlist],
    occ_board: Vec<u32>,
    occ_inst_base: Vec<u32>,
    occ_net_base: Vec<u32>,
    first_occ: Vec<u32>,
    pin_start: Vec<u32>,
    pin_occ: Vec<u32>,
    pin_local: Vec<u32>,
    pin_width: Vec<Width>,
    pin_is_out: Vec<bool>,
    unions: Vec<(u32, u32)>,
    inst_cursor: u32,
    net_cursor: u32,
    pin_cursor: u32,
    truncated: bool,
}

impl Builder<'_> {
    fn link(&mut self, na: u32, la: u32, nb: u32, lb: u32) {
        if la == NO_NET || lb == NO_NET {
            return;
        }
        self.unions.push((na + la, nb + lb));
    }

    fn walk(&mut self, occ: usize, path: &mut Vec<u32>) {
        // boards / nls 的生命周期独立于 self 的可变借用，先取出来避免借用冲突
        let boards = self.boards;
        let nls = self.nls;
        let b = self.occ_board[occ] as usize;
        let board = &boards[b];
        let nl = &nls[b];
        let net_base = self.occ_net_base[occ];
        let inst_base = self.occ_inst_base[occ];

        for (ii, inst) in board.instances.iter().enumerate() {
            let gi = inst_base + ii as u32;
            let base = self.pin_cursor;
            self.pin_start[gi as usize] = base;
            for (slot, pd) in inst.pins.iter().enumerate() {
                self.pin_occ.push(occ as u32);
                self.pin_local.push(nl.pin_net[nl.pin_start[ii] as usize + slot]);
                self.pin_width.push(pd.width);
                self.pin_is_out.push(pd.dir == Dir::Out);
                self.pin_cursor += 1;
            }

            if inst.sub == NO_SUB {
                continue;
            }
            let sub = inst.sub as usize;
            if sub >= boards.len() || sub == b || path.contains(&(sub as u32)) || path.len() >= MAX_DEPTH {
                self.truncated = true;
                continue;
            }

            let child = self.occ_board.len();
            let cb = &boards[sub];
            self.occ_board.push(sub as u32);
            self.occ_inst_base.push(self.inst_cursor);
            self.inst_cursor += cb.instances.len() as u32;
            self.pin_start.resize(self.inst_cursor as usize, 0);
            self.occ_net_base.push(self.net_cursor);
            self.net_cursor += nls[sub].net_count;
            if self.first_occ[sub] == u32::MAX {
                self.first_occ[sub] = child as u32;
            }

            // 外壳引脚与接口引脚并成同一个网络
            let cnet = self.occ_net_base[child];
            let cnl = &nls[sub];
            let sh = shape(cb);
            for (k, &pi) in sh.inputs.iter().chain(sh.outputs.iter()).enumerate() {
                let a = self.pin_local[(base + k as u32) as usize];
                let c = cnl.pin_net[cnl.pin_start[pi as usize] as usize];
                self.link(net_base, a, cnet, c);
            }

            path.push(sub as u32);
            self.walk(child, path);
            path.pop();
        }
    }
}

/// 展开整棵层次树。返回扁平网表与每张图纸的局部网表（后者的推导只做一次）。
pub fn build(boards: &[Board], root: u32) -> (Flat, Vec<Netlist>) {
    let nls: Vec<Netlist> = boards.iter().map(|b| b.compile()).collect();
    let n = boards.len();
    let mut flat = Flat {
        net_map: vec![Vec::new(); n],
        inst_map: vec![Vec::new(); n],
        ..Default::default()
    };
    if n == 0 || root as usize >= n {
        return (flat, nls);
    }

    let root_board = &boards[root as usize];
    let mut b = Builder {
        boards,
        nls: &nls,
        occ_board: vec![root],
        occ_inst_base: vec![0],
        occ_net_base: vec![0],
        first_occ: vec![u32::MAX; n],
        pin_start: vec![0; root_board.instances.len()],
        pin_occ: Vec::new(),
        pin_local: Vec::new(),
        pin_width: Vec::new(),
        pin_is_out: Vec::new(),
        unions: Vec::new(),
        inst_cursor: root_board.instances.len() as u32,
        net_cursor: nls[root as usize].net_count,
        pin_cursor: 0,
        truncated: false,
    };
    b.first_occ[root as usize] = 0;
    b.walk(0, &mut vec![root]);

    // ---- 网络合并与重新编号 ----
    let size = b.net_cursor.max(1) as usize;
    let mut uf = UnionFind::new(size);
    for &(x, y) in &b.unions {
        uf.union(x, y);
    }
    let mut remap = vec![0u32; size];
    let mut next = 0u32;
    for i in 0..size {
        if uf.find(i as u32) == i as u32 {
            remap[i] = next;
            next += 1;
        }
    }

    flat.net_count = next;
    for i in 0..b.pin_local.len() {
        let l = b.pin_local[i];
        if l == NO_NET {
            flat.pin_net.push(NO_NET);
        } else {
            let key = b.occ_net_base[b.pin_occ[i] as usize] + l;
            flat.pin_net.push(remap[uf.find(key) as usize]);
        }
    }

    for bi in 0..n {
        let fo = b.first_occ[bi];
        if fo == u32::MAX {
            continue;
        }
        let net_base = b.occ_net_base[fo as usize];
        flat.net_map[bi] = (0..nls[bi].net_count)
            .map(|l| remap[uf.find(net_base + l) as usize])
            .collect();
        let ib = b.occ_inst_base[fo as usize];
        flat.inst_map[bi] = (0..boards[bi].instances.len() as u32).map(|i| ib + i).collect();
    }

    for occ in 0..b.occ_board.len() {
        let bi = b.occ_board[occ] as usize;
        let ib = b.occ_inst_base[occ];
        for (ii, inst) in boards[bi].instances.iter().enumerate() {
            if inst.def.is_passive() {
                continue;
            }
            let ins = inst.pins.iter().filter(|p| p.dir == Dir::In).count();
            let gi = ib + ii as u32;
            flat.comps.push(FlatComp {
                def: inst.def,
                params: inst.params,
                board: bi as u32,
                inst: ii as u32,
                global_inst: gi,
                pin_start: b.pin_start[gi as usize],
                in_count: ins as u16,
                out_count: (inst.pins.len() - ins) as u16,
            });
            flat.states.push(inst.initial_state());
        }
    }

    flat.pin_start = b.pin_start;
    flat.pin_width = b.pin_width;
    flat.pin_is_out = b.pin_is_out;
    flat.inst_count = b.inst_cursor;
    flat.truncated = b.truncated;
    (flat, nls)
}
