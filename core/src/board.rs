//! 编辑期数据模型与网表推导（v3 §4 / ADR-7）
//!
//! Board 是用户所见（元件 + 导线），Netlist 是引擎所需（引脚 → 网络）。
//! 两者分离：Board 变化时才重新推导一次，仿真热路径只碰 Netlist。
//!
//! 连接判定（ADR-7 钉死）：
//!   - 导线端点 / 拐点落在另一条导线上 → 连接
//!   - 引脚落在导线上 → 连接
//!   - 引脚与引脚重合 → 连接
//!   - **纯交叉不连接**（两条线中段十字穿过，各自独立）
//!   - 一条导线自身所有线段电气连通
//! 逻辑层禁浮点，全部整数格坐标（ADR-22）。

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::defs::{CompState, DefId, Dir, Params, PinDef};
use crate::values::Width;

/// 未连接标记
pub const NO_NET: u32 = u32::MAX;

/// 默认实例名（ADR-25：层级调试路径与导出命名的基准）
pub fn auto_name(id: u32) -> String {
    format!("inst_{id}")
}

/// 整数格坐标
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// 板上元件实例
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Instance {
    pub def: DefId,
    pub params: Params,
    pub x: i32,
    pub y: i32,
    /// 0 / 90 / 180 / 270
    pub rot: u16,
    /// 实例名，默认自动生成 inst_<n>；层级调试路径与 Verilog 导出命名都用它（ADR-25）
    #[serde(default)]
    pub display_name: String,
    /// 引脚布局缓存：不入档，读档后由 def + params + rot 重建（Board::fixup）
    #[serde(skip)]
    pub pins: Vec<PinDef>,
    /// 存储器初始内容（RAM / ROM）
    #[serde(default)]
    pub mem_init: Vec<u32>,
}

impl Instance {
    pub fn new(def: DefId, params: Params, x: i32, y: i32) -> Self {
        let mut it = Self {
            def,
            params,
            x,
            y,
            rot: 0,
            display_name: String::new(),
            pins: Vec::new(),
            mem_init: Vec::new(),
        };
        it.rebuild_pins();
        it
    }

    /// 依据当前参数与旋转重建引脚布局。
    ///
    /// 旋转后以**组件包围盒**的四角为基准平移回非负区，保证原点始终是左上角。
    /// 只按引脚取 min 会把"输出引脚在右侧"的布局（开关 / 常量 / 时钟）压回原点，
    /// 组件框会塌成一格——这是错的。
    pub fn rebuild_pins(&mut self) {
        let mut pins = self.def.pins(&self.params);
        let bw = pins.iter().map(|p| p.dx).max().unwrap_or(0) + 1;
        let bh = pins.iter().map(|p| p.dy).max().unwrap_or(0) + 1;
        let rot = self.rot % 360;
        let rotate = |x: i32, y: i32| -> (i32, i32) {
            match rot {
                90 => (-y, x),
                180 => (-x, -y),
                270 => (y, -x),
                _ => (x, y),
            }
        };
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        for (cx, cy) in [(0, 0), (bw - 1, 0), (0, bh - 1), (bw - 1, bh - 1)] {
            let (x, y) = rotate(cx, cy);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
        }
        for p in pins.iter_mut() {
            let (x, y) = rotate(p.dx, p.dy);
            p.dx = x - min_x;
            p.dy = y - min_y;
        }
        self.pins = pins;
    }

    /// 引脚在世界格坐标中的位置
    pub fn pin_world(&self, slot: usize) -> Option<Point> {
        self.pins.get(slot).map(|p| Point::new(self.x + p.dx, self.y + p.dy))
    }

    /// 引脚布局的包围盒（宽, 高），至少 1×1 格
    pub fn size(&self) -> (i32, i32) {
        let w = self.pins.iter().map(|p| p.dx).max().unwrap_or(0) + 1;
        let h = self.pins.iter().map(|p| p.dy).max().unwrap_or(0) + 1;
        (w.max(1), h.max(1))
    }

    /// 供仿真使用的初始状态（ADR-2：上电全 0，仅存储器可预置）
    pub fn initial_state(&self) -> CompState {
        if matches!(self.def, DefId::Ram | DefId::Rom) {
            CompState::with_mem(&self.mem_init, self.params.depth)
        } else {
            CompState::default()
        }
    }
}

/// 文本注释：无电气行为，随 Board 保存（v4 §6.7，教学软件刚需）
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Annotation {
    pub x: i32,
    pub y: i32,
    pub text: String,
    /// 0xRRGGBB；0 表示用默认色
    #[serde(default)]
    pub color: u32,
}

/// 网络标签：同名即相连，无需物理连线（v4 §8）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetLabel {
    pub x: i32,
    pub y: i32,
    pub name: String,
}

/// 导线：正交折线（≥2 个点，只保留端点与拐点）
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Wire {
    pub points: Vec<Point>,
}

impl Wire {
    pub fn new(points: Vec<Point>) -> Self {
        Self { points }
    }

    /// 点是否落在该导线的任意线段上（含端点）
    pub fn contains_point(&self, q: Point) -> bool {
        self.points
            .windows(2)
            .any(|seg| point_on_segment(q, seg[0], seg[1]))
    }
}

/// 点到正交线段的格距平方（允许线段是任意方向，取最近点的整数近似）
fn segment_distance2(p: Point, a: Point, b: Point) -> i32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 == 0 {
        let (ex, ey) = (p.x - a.x, p.y - a.y);
        return ex * ex + ey * ey;
    }
    // 把 p 投影到线段上（整数四舍五入），再夹到线段范围内
    let t_num = (p.x - a.x) * dx + (p.y - a.y) * dy;
    let t = t_num.clamp(0, len2);
    let px = a.x + (t * dx) / len2;
    let py = a.y + (t * dy) / len2;
    let (ex, ey) = (p.x - px, p.y - py);
    ex * ex + ey * ey
}

/// 点是否落在线段 ab 上（整数叉积，无浮点）
#[inline]
pub fn point_on_segment(p: Point, a: Point, b: Point) -> bool {
    let cross = (b.x - a.x) as i64 * (p.y - a.y) as i64 - (b.y - a.y) as i64 * (p.x - a.x) as i64;
    if cross != 0 {
        return false;
    }
    let (min_x, max_x) = if a.x < b.x { (a.x, b.x) } else { (b.x, a.x) };
    let (min_y, max_y) = if a.y < b.y { (a.y, b.y) } else { (b.y, a.y) };
    p.x >= min_x && p.x <= max_x && p.y >= min_y && p.y <= max_y
}

// ---------------------------------------------------------------------------
// 自动走线
// ---------------------------------------------------------------------------

/// 走线搜索区域在两端点包围盒外再外扩的格数
const ROUTE_PAD: i32 = 16;
/// 每转一次弯的额外代价：让走线尽量少拐弯，可读性优先
const TURN_COST: u32 = 8;

/// 区间列表是否完整覆盖 [lo, hi]（列表已合并为互不相交）
fn covers(spans: Option<&Vec<(i32, i32)>>, lo: i32, hi: i32) -> bool {
    let Some(spans) = spans else {
        return false;
    };
    let i = spans.partition_point(|s| s.0 <= lo);
    i > 0 && spans[i - 1].1 >= hi
}

/// 已有导线对各行 / 各列的占用，供寻路禁止"重叠边"。
///
/// 按 ADR-7，只有**真重叠**会误连；仅在端点相接（同一引脚的扇出）是正常走线，必须放行。
struct AvoidMap {
    rows: HashMap<i32, Vec<(i32, i32)>>,
    cols: HashMap<i32, Vec<(i32, i32)>>,
}

impl AvoidMap {
    fn build(wires: &[Wire]) -> Self {
        let mut rows: HashMap<i32, Vec<(i32, i32)>> = HashMap::new();
        let mut cols: HashMap<i32, Vec<(i32, i32)>> = HashMap::new();
        for w in wires {
            for seg in w.points.windows(2) {
                let (a, b) = (seg[0], seg[1]);
                if a.y == b.y {
                    rows.entry(a.y).or_default().push((a.x.min(b.x), a.x.max(b.x)));
                } else if a.x == b.x {
                    cols.entry(a.x).or_default().push((a.y.min(b.y), a.y.max(b.y)));
                }
            }
        }
        for group in [&mut rows, &mut cols] {
            for spans in group.values_mut() {
                spans.sort_unstable_by_key(|s| s.0);
                let mut merged: Vec<(i32, i32)> = Vec::with_capacity(spans.len());
                for &(lo, hi) in spans.iter() {
                    match merged.last_mut() {
                        Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
                        _ => merged.push((lo, hi)),
                    }
                }
                *spans = merged;
            }
        }
        Self { rows, cols }
    }

    /// 水平单位边 (x, y)-(x+1, y) 是否落在已有导线上
    fn h_blocked(&self, y: i32, x: i32) -> bool {
        covers(self.rows.get(&y), x, x + 1)
    }

    /// 垂直单位边 (x, y)-(x, y+1) 是否落在已有导线上
    fn v_blocked(&self, x: i32, y: i32) -> bool {
        covers(self.cols.get(&x), y, y + 1)
    }
}

/// 折线简化：只保留端点与拐点，去掉重复点与共线的中间点。
///
/// 这不只是为了紧凑，更是**正确性要求**：ADR-7 的"压到线上即连接"针对的是导线端点，
/// 若把路径上每个格点都当顶点，后续走线就会被前面线自己的路径格点堵死，
/// 最后只能退回到会误连的兜底折线。
fn simplify_path(pts: Vec<Point>) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(pts.len());
    for p in pts {
        if out.last() == Some(&p) {
            continue;
        }
        if out.len() >= 2 {
            let a = out[out.len() - 2];
            let b = out[out.len() - 1];
            let collinear = (a.x == b.x && b.x == p.x) || (a.y == b.y && b.y == p.y);
            if collinear {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// 两点间的正交走线：在受限网格上跑带转弯代价的 Dijkstra。
///
/// 状态是 (格点, 进入方向)，因此"少拐弯"能被显式优化，结果既短又干净。
/// 禁止通行的有两种：落入 `blocked`（引脚 / 已有导线端点与拐点）的格点，
/// 以及与 `avoid` 中导线真重叠的边（重叠必然误连）。
/// **十字穿越是允许的**（ADR-7：交叉不连接）。
///
/// 搜索区域限制在两端点包围盒外扩 `ROUTE_PAD` 格内，保证交互操作不被大电路拖慢。
/// 返回 `None` 表示确实无路可走——调用方应据实反馈，而不是硬画一条错线。
pub fn route_orthogonal(
    from: Point,
    to: Point,
    blocked: &[Point],
    avoid: &[Wire],
) -> Option<Vec<Point>> {
    if from == to {
        return None;
    }
    let x0 = from.x.min(to.x) - ROUTE_PAD;
    let x1 = from.x.max(to.x) + ROUTE_PAD;
    let y0 = from.y.min(to.y) - ROUTE_PAD;
    let y1 = from.y.max(to.y) + ROUTE_PAD;
    let w = (x1 - x0 + 1) as u32;

    let blocked_set: HashSet<(i32, i32)> = blocked.iter().map(|p| (p.x, p.y)).collect();
    let avoid_map = AvoidMap::build(avoid);

    // 状态 = 格点 × 3 个"进入方向"（0 = 起点、1 = 水平、2 = 垂直）
    let idx = |x: i32, y: i32, d: u32| ((y - y0) as u32 * w + (x - x0) as u32) * 3 + d;
    let decode = |i: u32| {
        let d = i % 3;
        let cell = i / 3;
        (x0 + (cell % w) as i32, y0 + (cell / w) as i32, d)
    };

    let n = ((x1 - x0 + 1) as u32 * (y1 - y0 + 1) as u32 * 3) as usize;
    let mut dist = vec![u32::MAX; n];
    let mut prev = vec![u32::MAX; n];
    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();

    let start = idx(from.x, from.y, 0);
    dist[start as usize] = 0;
    heap.push(Reverse((0, start)));

    let mut goal: Option<u32> = None;
    while let Some(Reverse((cost, cur))) = heap.pop() {
        if cost > dist[cur as usize] {
            continue;
        }
        let (x, y, d) = decode(cur);
        if x == to.x && y == to.y {
            goal = Some(cur);
            break;
        }
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let nx = x + dx;
            let ny = y + dy;
            if nx < x0 || nx > x1 || ny < y0 || ny > y1 {
                continue;
            }
            if blocked_set.contains(&(nx, ny)) {
                continue;
            }
            let nd = if dx != 0 { 1 } else { 2 };
            if nd == 1 {
                if avoid_map.h_blocked(y, x.min(nx)) {
                    continue;
                }
            } else if avoid_map.v_blocked(x, y.min(ny)) {
                continue;
            }
            let step = 1 + if d != 0 && d != nd { TURN_COST } else { 0 };
            let nc = cost + step;
            let ni = idx(nx, ny, nd);
            if nc < dist[ni as usize] {
                dist[ni as usize] = nc;
                prev[ni as usize] = cur;
                heap.push(Reverse((nc, ni)));
            }
        }
    }

    let mut cur = goal?;
    let mut pts: Vec<Point> = Vec::new();
    loop {
        let (x, y, _) = decode(cur);
        pts.push(Point::new(x, y));
        let p = prev[cur as usize];
        if p == u32::MAX {
            break;
        }
        cur = p;
    }
    pts.reverse();
    Some(simplify_path(pts))
}

// ---------------------------------------------------------------------------
// 电路板
// ---------------------------------------------------------------------------

/// 电路板（图纸）
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Board {
    pub instances: Vec<Instance>,
    pub wires: Vec<Wire>,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    #[serde(default)]
    pub labels: Vec<NetLabel>,
}

impl Board {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加元件，返回其索引（索引即稳定 id）
    pub fn add_instance(&mut self, def: DefId, params: Params, x: i32, y: i32) -> u32 {
        let id = self.instances.len() as u32;
        let mut inst = Instance::new(def, params, x, y);
        inst.display_name = auto_name(id);
        self.instances.push(inst);
        id
    }

    /// 按 id 取实例名（层级路径用，ADR-25）
    pub fn instance_name(&self, id: u32) -> Option<&str> {
        self.instances.get(id as usize).map(|i| i.display_name.as_str())
    }

    pub fn add_annotation(&mut self, x: i32, y: i32, text: &str) -> u32 {
        self.annotations.push(Annotation {
            x,
            y,
            text: text.to_string(),
            color: 0,
        });
        (self.annotations.len() - 1) as u32
    }

    pub fn remove_annotation(&mut self, id: u32) -> bool {
        if (id as usize) < self.annotations.len() {
            self.annotations.remove(id as usize);
            true
        } else {
            false
        }
    }

    pub fn add_label(&mut self, x: i32, y: i32, name: &str) -> u32 {
        self.labels.push(NetLabel { x, y, name: name.to_string() });
        (self.labels.len() - 1) as u32
    }

    pub fn remove_label(&mut self, id: u32) -> bool {
        if (id as usize) < self.labels.len() {
            self.labels.remove(id as usize);
            true
        } else {
            false
        }
    }

    /// 命中测试：落在该格点上的标签
    pub fn pick_label(&self, x: i32, y: i32) -> Option<u32> {
        self.labels
            .iter()
            .rposition(|l| l.x == x && l.y == y)
            .map(|i| i as u32)
    }

    /// 命中测试：返回落在该格点上的注释
    pub fn pick_annotation(&self, x: i32, y: i32) -> Option<u32> {
        for (i, a) in self.annotations.iter().enumerate().rev() {
            // 注释按字符数估算宽度，高度固定 1 格
            let w = (a.text.chars().count() as i32).max(1);
            if x >= a.x && x < a.x + w && y >= a.y && y < a.y + 1 {
                return Some(i as u32);
            }
        }
        None
    }

    /// 删除元件；导线需由调用方负责清理 / 重新推导
    pub fn remove_instance(&mut self, id: u32) -> bool {
        if (id as usize) < self.instances.len() {
            self.instances.remove(id as usize);
            true
        } else {
            false
        }
    }

    pub fn add_wire(&mut self, points: Vec<Point>) -> u32 {
        self.wires.push(Wire::new(points));
        (self.wires.len() - 1) as u32
    }

    pub fn instance(&self, id: u32) -> Option<&Instance> {
        self.instances.get(id as usize)
    }

    pub fn instance_mut(&mut self, id: u32) -> Option<&mut Instance> {
        self.instances.get_mut(id as usize)
    }

    /// 移动元件
    pub fn set_pos(&mut self, id: u32, x: i32, y: i32) {
        if let Some(it) = self.instances.get_mut(id as usize) {
            it.x = x;
            it.y = y;
        }
    }

    /// 旋转元件（顺时针 90°）
    pub fn rotate(&mut self, id: u32) {
        if let Some(it) = self.instances.get_mut(id as usize) {
            it.rot = (it.rot + 90) % 360;
            it.rebuild_pins();
        }
    }

    /// 改参数并同步引脚布局
    pub fn set_params(&mut self, id: u32, params: Params) {
        if let Some(it) = self.instances.get_mut(id as usize) {
            it.params = params;
            it.rebuild_pins();
        }
    }

    /// 命中测试：返回 (实例, 引脚槽位)，取半径内最近的一个（半径为格数）
    pub fn pick_pin(&self, x: i32, y: i32, radius: i32) -> Option<(u32, usize)> {
        let r2 = radius * radius;
        let mut best: Option<(u32, usize, i32)> = None;
        for (ii, inst) in self.instances.iter().enumerate() {
            for (slot, p) in inst.pins.iter().enumerate() {
                let dx = inst.x + p.dx - x;
                let dy = inst.y + p.dy - y;
                let d2 = dx * dx + dy * dy;
                if d2 <= r2 && best.map_or(true, |(_, _, bd)| d2 < bd) {
                    best = Some((ii as u32, slot, d2));
                }
            }
        }
        best.map(|(i, s, _)| (i, s))
    }

    /// 命中测试：返回落在该格点上的元件（取其包围盒）
    pub fn pick_component(&self, x: i32, y: i32) -> Option<u32> {
        // 逆序遍历：后放的元件在上层
        for (ii, inst) in self.instances.iter().enumerate().rev() {
            let (w, h) = inst.size();
            if x >= inst.x && x < inst.x + w && y >= inst.y && y < inst.y + h {
                return Some(ii as u32);
            }
        }
        None
    }

    /// 命中测试：返回该格点所在的导线索引
    pub fn pick_wire(&self, x: i32, y: i32, radius: i32) -> Option<u32> {
        let mut best: Option<(u32, i32)> = None;
        for (wi, w) in self.wires.iter().enumerate() {
            for seg in w.points.windows(2) {
                // 点到线段的格距上界（正交线段，直接取切比雪夫距离即可）
                let d = segment_distance2(Point::new(x, y), seg[0], seg[1]);
                if d <= radius * radius && best.map_or(true, |(_, bd)| d < bd) {
                    best = Some((wi as u32, d));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    /// 删除导线（索引会移动，调用方需自行刷新视图）
    pub fn remove_wire(&mut self, index: u32) -> bool {
        if (index as usize) < self.wires.len() {
            self.wires.remove(index as usize);
            true
        } else {
            false
        }
    }

    /// 全部引脚的世界坐标（走线避让用）
    pub fn all_pin_points(&self) -> Vec<Point> {
        let mut v = Vec::with_capacity(self.pin_total());
        for inst in &self.instances {
            for p in &inst.pins {
                v.push(Point::new(inst.x + p.dx, inst.y + p.dy));
            }
        }
        v
    }

    /// 从引脚拉到引脚：自动避让其它引脚与已有导线后落一条导线。
    ///
    /// 返回 false 表示**没有找到不会误连的路径**（此时仍会落一条兜底折线，供 UI 高亮提示）。
    pub fn connect_pins(&mut self, from: (u32, usize), to: (u32, usize)) -> bool {
        let Some(a) = self.instances.get(from.0 as usize).and_then(|i| i.pin_world(from.1))
        else {
            return false;
        };
        let Some(c) = self.instances.get(to.0 as usize).and_then(|i| i.pin_world(to.1)) else {
            return false;
        };
        // 避让对象：其它引脚 + 已有导线的端点与拐点；并禁止与已有导线真重叠
        let mut blocked = self.all_pin_points();
        for w in &self.wires {
            blocked.extend_from_slice(&w.points);
        }
        blocked.retain(|p| *p != a && *p != c);

        match route_orthogonal(a, c, &blocked, &self.wires) {
            Some(pts) => {
                self.add_wire(pts);
                true
            }
            None => {
                let fallback = simplify_path(vec![a, Point::new(c.x, a.y), c]);
                if fallback.len() >= 2 {
                    self.add_wire(fallback);
                }
                false
            }
        }
    }

    /// 全部引脚数
    pub fn pin_total(&self) -> usize {
        self.instances.iter().map(|i| i.pins.len()).sum()
    }

    /// 读档后的修复：引脚布局是缓存（按 def + params + rot 重建），
    /// 实例名缺失时补上 inst_<n>（老工程兼容，ADR-25）
    pub fn fixup(&mut self) {
        for (i, inst) in self.instances.iter_mut().enumerate() {
            inst.rebuild_pins();
            if inst.display_name.is_empty() {
                inst.display_name = auto_name(i as u32);
            }
        }
    }

    /// 从图推导网表
    pub fn compile(&self) -> Netlist {
        let mut n = Netlist::default();
        n.build(self);
        n
    }
}

// ---------------------------------------------------------------------------
// 网表推导
// ---------------------------------------------------------------------------

/// 引擎所需的网表视图
#[derive(Clone, Debug, Default)]
pub struct Netlist {
    /// 全局引脚 → 网络（NO_NET = 未连接）
    pub pin_net: Vec<u32>,
    /// 全局引脚 → 所属实例索引
    pub pin_owner: Vec<u32>,
    /// 全局引脚 → 实例内槽位
    pub pin_slot: Vec<u16>,
    /// 实例索引 → 该实例第一个全局引脚
    pub pin_start: Vec<u32>,
    /// 全局引脚位宽
    pub pin_width: Vec<Width>,
    /// 全局引脚是否属于输出端
    pub pin_is_out: Vec<bool>,
    /// 导线索引 → 网络（NO_NET = 悬空导线）
    pub wire_net: Vec<u32>,
    pub net_count: u32,
}

/// 并查集
struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self { parent: (0..n as u32).collect() }
    }

    fn find(&mut self, x: u32) -> u32 {
        let mut r = x;
        while self.parent[r as usize] != r {
            r = self.parent[r as usize];
        }
        let mut c = x;
        while self.parent[c as usize] != r {
            let next = self.parent[c as usize];
            self.parent[c as usize] = r;
            c = next;
        }
        r
    }

    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        if ra < rb {
            self.parent[rb as usize] = ra;
        } else {
            self.parent[ra as usize] = rb;
        }
    }
}

/// 正交线段索引。
///
/// 核心性质（ADR-7 的推论）：**同一直线上的两条正交线段，只要区间相交（含端点相接），
/// 就必然连接**——相交时必有一方的端点落在另一方上。于是可以先把相交区间合并成
/// 互不相交的集合，之后任何"点是否落在线段上"的查询只需一次二分。
///
/// 这让网表推导从 O(导线数²) 降到 O((导线数 + 引脚数)·log n)：
/// 无论是星形布线还是一根长总线挂上万个引脚，都不会退化。
#[derive(Default)]
struct OrthoIndex {
    /// y → 该行上互不相交的水平区间 (x0, x1, 代表导线)
    rows: HashMap<i32, Vec<(i32, i32, u32)>>,
    /// x → 该列上互不相交的垂直区间 (y0, y1, 代表导线)
    cols: HashMap<i32, Vec<(i32, i32, u32)>>,
}

impl OrthoIndex {
    fn build(wires: &[Wire], np: usize, uf: &mut UnionFind) -> Self {
        let mut rows: HashMap<i32, Vec<(i32, i32, u32)>> = HashMap::new();
        let mut cols: HashMap<i32, Vec<(i32, i32, u32)>> = HashMap::new();
        for (wi, w) in wires.iter().enumerate() {
            for seg in w.points.windows(2) {
                let (a, b) = (seg[0], seg[1]);
                if a.y == b.y {
                    rows.entry(a.y).or_default().push((a.x.min(b.x), a.x.max(b.x), wi as u32));
                } else if a.x == b.x {
                    cols.entry(a.x).or_default().push((a.y.min(b.y), a.y.max(b.y), wi as u32));
                }
                // 非正交线段不被支持（逻辑层只接受正交折线），直接忽略
            }
        }
        for group in [&mut rows, &mut cols] {
            for spans in group.values_mut() {
                spans.sort_unstable_by_key(|s| s.0);
                let mut merged: Vec<(i32, i32, u32)> = Vec::with_capacity(spans.len());
                for &(lo, hi, w) in spans.iter() {
                    match merged.last_mut() {
                        Some(last) if lo <= last.1 => {
                            uf.union((np + last.2 as usize) as u32, (np + w as usize) as u32);
                            last.1 = last.1.max(hi);
                        }
                        _ => merged.push((lo, hi, w)),
                    }
                }
                *spans = merged;
            }
        }
        Self { rows, cols }
    }

    /// 点命中的导线（水平、垂直方向各至多一条），写入 out
    fn hit(&self, p: Point, out: &mut Vec<u32>) {
        out.clear();
        if let Some(spans) = self.rows.get(&p.y) {
            let i = spans.partition_point(|s| s.0 <= p.x);
            if i > 0 && spans[i - 1].1 >= p.x {
                out.push(spans[i - 1].2);
            }
        }
        if let Some(spans) = self.cols.get(&p.x) {
            let i = spans.partition_point(|s| s.0 <= p.y);
            if i > 0 && spans[i - 1].1 >= p.y {
                out.push(spans[i - 1].2);
            }
        }
    }
}

impl Netlist {
    fn build(&mut self, board: &Board) {
        // ---- 1. 展平引脚 ----
        let mut pin_start = Vec::with_capacity(board.instances.len());
        let mut pin_owner = Vec::new();
        let mut pin_slot = Vec::new();
        let mut pin_width = Vec::new();
        let mut pin_is_out = Vec::new();
        let mut pin_pos = Vec::new();

        for (ii, inst) in board.instances.iter().enumerate() {
            pin_start.push(pin_owner.len() as u32);
            for (slot, pd) in inst.pins.iter().enumerate() {
                pin_owner.push(ii as u32);
                pin_slot.push(slot as u16);
                pin_width.push(pd.width);
                pin_is_out.push(pd.dir == Dir::Out);
                pin_pos.push(Point::new(inst.x + pd.dx, inst.y + pd.dy));
            }
        }

        let np = pin_owner.len();
        let nw = board.wires.len();

        // ---- 2. 并查集：节点 = 引脚 ∪ 导线 ----
        let mut uf = UnionFind::new(np + nw);
        let wire_node = |w: usize| (np + w) as u32;

        let index = OrthoIndex::build(&board.wires, np, &mut uf);
        let mut hits: Vec<u32> = Vec::with_capacity(2);

        // 导线端点 / 拐点落在另一条导线上 → 连接（ADR-7：纯交叉不连接）
        for (wi, w) in board.wires.iter().enumerate() {
            for &p in &w.points {
                index.hit(p, &mut hits);
                for &wj in &hits {
                    if wj as usize != wi {
                        uf.union(wire_node(wi), wire_node(wj as usize));
                    }
                }
            }
        }

        // 引脚落在导线上 → 连接
        for (pi, p) in pin_pos.iter().enumerate() {
            index.hit(*p, &mut hits);
            for &wj in &hits {
                uf.union(pi as u32, wire_node(wj as usize));
            }
        }

        // 网络标签：同名即相连（v4 §8）。标签落在导线上或引脚上都算。
        {
            let mut by_name: HashMap<&str, u32> = HashMap::new();
            for l in &board.labels {
                if l.name.is_empty() {
                    continue;
                }
                let at = Point::new(l.x, l.y);
                index.hit(at, &mut hits);
                let node = if let Some(&w) = hits.first() {
                    wire_node(w as usize)
                } else if let Some(pi) = pin_pos.iter().position(|p| *p == at) {
                    pi as u32
                } else {
                    continue;
                };
                match by_name.get(l.name.as_str()) {
                    Some(&first) => uf.union(node, first),
                    None => {
                        by_name.insert(l.name.as_str(), node);
                    }
                }
            }
        }

        // 引脚与引脚重合 → 连接（位置分桶，避免 O(P²)）
        {
            let mut buckets: HashMap<(i32, i32), u32> = HashMap::with_capacity(np);
            for (pi, p) in pin_pos.iter().enumerate() {
                match buckets.get(&(p.x, p.y)) {
                    Some(&first) => uf.union(pi as u32, first),
                    None => {
                        buckets.insert((p.x, p.y), pi as u32);
                    }
                }
            }
        }

        // ---- 3. 根 → 网络号 ----
        let mut root_pins = vec![0u32; np + nw];
        let mut root_wires = vec![0u32; np + nw];
        for pi in 0..np {
            root_pins[uf.find(pi as u32) as usize] += 1;
        }
        for wi in 0..nw {
            root_wires[uf.find(wire_node(wi)) as usize] += 1;
        }

        let mut net_of_root: Vec<u32> = vec![NO_NET; np + nw];
        let mut next_net = 0u32;
        let mut pin_net = vec![NO_NET; np];
        for pi in 0..np {
            let r = uf.find(pi as u32) as usize;
            // 孤立引脚（无导线、无重合引脚）视为未连接
            if root_wires[r] == 0 && root_pins[r] < 2 {
                continue;
            }
            if net_of_root[r] == NO_NET {
                net_of_root[r] = next_net;
                next_net += 1;
            }
            pin_net[pi] = net_of_root[r];
        }

        // 悬空的**输出**引脚也分配私有网络：它的值必须可读（探针、单测、波形都要）。
        // 悬空输入不需要（ADR-5：读作 0）。
        for pi in 0..np {
            if pin_net[pi] == NO_NET && pin_is_out[pi] {
                pin_net[pi] = next_net;
                next_net += 1;
            }
        }

        let mut wire_net = vec![NO_NET; nw];
        for (wi, w) in wire_net.iter_mut().enumerate() {
            *w = net_of_root[uf.find(wire_node(wi)) as usize];
        }

        self.pin_net = pin_net;
        self.wire_net = wire_net;
        self.pin_owner = pin_owner;
        self.pin_slot = pin_slot;
        self.pin_start = pin_start;
        self.pin_width = pin_width;
        self.pin_is_out = pin_is_out;
        self.net_count = next_net;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_hit_testing() {
        let a = Point::new(0, 0);
        let b = Point::new(10, 0);
        assert!(point_on_segment(Point::new(5, 0), a, b));
        assert!(point_on_segment(a, a, b));
        assert!(point_on_segment(b, a, b));
        assert!(!point_on_segment(Point::new(11, 0), a, b));
        assert!(!point_on_segment(Point::new(5, 1), a, b));
        let c = Point::new(0, 0);
        let d = Point::new(4, 4);
        assert!(point_on_segment(Point::new(2, 2), c, d));
        assert!(!point_on_segment(Point::new(2, 3), c, d));
    }

    #[test]
    fn path_simplify_keeps_only_corners() {
        let raw = vec![
            Point::new(0, 0),
            Point::new(1, 0),
            Point::new(2, 0),
            Point::new(2, 1),
            Point::new(2, 2),
            Point::new(2, 2),
            Point::new(1, 2),
        ];
        assert_eq!(
            simplify_path(raw),
            vec![
                Point::new(0, 0),
                Point::new(2, 0),
                Point::new(2, 2),
                Point::new(1, 2)
            ]
        );
    }

    #[test]
    fn rotation_keeps_origin_at_corner() {
        let mut inst = Instance::new(DefId::And, Params::default().width(1).inputs(2), 0, 0);
        let before: Vec<(i32, i32)> = inst.pins.iter().map(|p| (p.dx, p.dy)).collect();
        inst.rot = 180;
        inst.rebuild_pins();
        let min_x = inst.pins.iter().map(|p| p.dx).min().unwrap();
        let min_y = inst.pins.iter().map(|p| p.dy).min().unwrap();
        assert_eq!((min_x, min_y), (0, 0));
        assert_ne!(before, inst.pins.iter().map(|p| (p.dx, p.dy)).collect::<Vec<_>>());
        assert_eq!(inst.pins.len(), before.len());
    }

    #[test]
    fn switch_output_pin_sits_on_the_right_edge() {
        let it = Instance::new(DefId::Switch, Params::default().width(1), 5, 7);
        assert_eq!(it.size(), (3, 1), "开关的框是 3×1，输出在右边缘");
        assert_eq!(it.pin_world(0), Some(Point::new(7, 7)));
    }

    #[test]
    fn wire_connects_two_pins() {
        let mut b = Board::new();
        let sw = b.add_instance(DefId::Switch, Default::default(), 0, 0);
        let led = b.add_instance(DefId::Led, Default::default(), 10, 0);
        assert!(b.connect_pins((sw, 0), (led, 0)));
        let nl = b.compile();
        assert_ne!(nl.pin_net[0], NO_NET);
        assert_eq!(nl.pin_net[0], nl.pin_net[1]);
    }

    #[test]
    fn crossing_wires_do_not_connect() {
        let mut b = Board::new();
        let sw = b.add_instance(DefId::Switch, Default::default(), 0, 0);
        let led = b.add_instance(DefId::Led, Default::default(), 10, 6);
        // 水平线经 (5,0)，竖直线也经 (5,0)：纯交叉
        b.add_wire(vec![Point::new(2, 0), Point::new(12, 0)]);
        b.add_wire(vec![Point::new(5, -4), Point::new(5, 4)]);
        // 第三条线从竖直线的端点接出，接到探针
        b.add_wire(vec![Point::new(5, 4), Point::new(5, 6), Point::new(10, 6)]);
        let nl = b.compile();
        let a = nl.pin_net[nl.pin_start[sw as usize] as usize];
        let led_net = nl.pin_net[nl.pin_start[led as usize] as usize];
        assert_ne!(a, NO_NET);
        assert_ne!(led_net, NO_NET);
        assert_ne!(a, led_net, "中段交叉不得连接");
    }

    #[test]
    fn endpoint_touching_mid_segment_connects() {
        let mut b = Board::new();
        let sw = b.add_instance(DefId::Switch, Default::default(), 0, 0);
        let led = b.add_instance(DefId::Led, Default::default(), 6, 0);
        b.add_wire(vec![Point::new(2, 0), Point::new(12, 0)]);
        // 端点 (6,0) 落在上面那条线的中段 → 连接
        b.add_wire(vec![Point::new(6, 0), Point::new(6, 6)]);
        let nl = b.compile();
        let a = nl.pin_net[nl.pin_start[sw as usize] as usize];
        let led_net = nl.pin_net[nl.pin_start[led as usize] as usize];
        assert_ne!(a, NO_NET);
        assert_eq!(a, led_net);
    }

    #[test]
    fn dangling_pins_differ_by_direction() {
        let mut b = Board::new();
        b.add_instance(DefId::And, Params::default().width(1).inputs(2), 0, 0);
        let nl = b.compile();
        // 输入悬空 → 无网络（ADR-5 读作 0）；输出悬空 → 私有网络（值必须可读）
        assert_eq!(nl.pin_net[0], NO_NET);
        assert_eq!(nl.pin_net[1], NO_NET);
        assert_ne!(nl.pin_net[2], NO_NET);
    }

    #[test]
    fn coincident_pins_connect_without_wire() {
        let mut b = Board::new();
        // Switch 输出在 (2,0)，Led 输入在 (0,0) → 把 Led 放在 (2,0)
        b.add_instance(DefId::Switch, Default::default(), 0, 0);
        b.add_instance(DefId::Led, Default::default(), 2, 0);
        let nl = b.compile();
        assert_eq!(nl.net_count, 1);
    }

    #[test]
    fn autoroute_avoids_other_pins() {
        use crate::defs::OPT_CARRY;
        let mut b = Board::new();
        let c = b.add_instance(DefId::Constant, Default::default(), 0, 4);
        let add = b.add_instance(DefId::Adder, Params::default().width(8).opts(OPT_CARRY), 4, 0);
        assert!(b.connect_pins((c, 0), (add, 1)));
        let w = &b.wires[0];
        // 加法器引脚：A(4,0) B(4,1) CI(4,2)，走线只能碰 B
        assert!(!w.contains_point(Point::new(4, 0)), "不得压到 A");
        assert!(!w.contains_point(Point::new(4, 2)), "不得压到 CI");
        assert!(w.contains_point(Point::new(4, 1)), "必须接到 B");
    }

    /// 一个真实场景的回归：NAND 半加器里同一个引脚要扇出 4 条线，
    /// 自动走线必须全部成功、互不误连，且路径只含端点和拐点。
    #[test]
    fn fanout_routing_succeeds() {
        let mut b = Board::new();
        let sw_a = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
        let sw_b = b.add_instance(DefId::Switch, Params::default().width(1), 0, 6);
        let nand2 = || Params::default().width(1).inputs(2);
        let n1 = b.add_instance(DefId::Nand, nand2(), 6, 0);
        let n2 = b.add_instance(DefId::Nand, nand2(), 6, 3);
        let n3 = b.add_instance(DefId::Nand, nand2(), 6, 6);
        let xor = b.add_instance(DefId::Nand, nand2(), 12, 2);
        let and = b.add_instance(DefId::Nand, nand2(), 12, 8);

        let links = [
            ((sw_a, 0), (n1, 0)),
            ((sw_b, 0), (n1, 1)),
            ((sw_a, 0), (n2, 0)),
            ((n1, 2), (n2, 1)),
            ((sw_b, 0), (n3, 0)),
            ((n1, 2), (n3, 1)),
            ((n2, 2), (xor, 0)),
            ((n3, 2), (xor, 1)),
            ((n1, 2), (and, 0)),
            ((n1, 2), (and, 1)),
        ];
        for (i, (f, t)) in links.iter().enumerate() {
            assert!(b.connect_pins(*f, *t), "第 {i} 条连线失败: {f:?} -> {t:?}");
        }
        for w in &b.wires {
            assert!(w.points.len() >= 2);
            for seg in w.points.windows(2) {
                assert!(
                    seg[0].x == seg[1].x || seg[0].y == seg[1].y,
                    "走线必须是正交折线: {:?}",
                    w.points
                );
            }
        }
        // A / B / n1.Y 必须各是独立的网络，不得被短路
        let nl = b.compile();
        let net = |inst: u32, slot: usize| {
            let p = nl.pin_start[inst as usize] + slot as u32;
            nl.pin_net[p as usize]
        };
        let y1 = net(n1, 2);
        assert_ne!(net(sw_a, 0), net(sw_b, 0), "A / B 不得短接");
        assert_ne!(net(sw_a, 0), y1, "A 不得与 n1.Y 短接");
        assert_ne!(net(sw_b, 0), y1, "B 不得与 n1.Y 短接");
    }
}
