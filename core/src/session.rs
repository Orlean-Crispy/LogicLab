//! 编辑与仿真的统一门面（v3 §3.3 的 driver 层 + v4 ADR-9 / ADR-23 / ADR-28）
//!
//! 存在的理由：
//!   1. **ADR-23 编辑即重置**：任何编辑生效后仿真整体回到上电态。收在门面里，
//!      壳就不可能漏（漏掉的表现是 undo 后寄存器值未定义，属于必然踩的坑）。
//!   2. **ADR-9 撤销/重做**：历史天然属于这里，壳只调 undo/redo。
//!   3. **ADR-28 层次**：图纸导航、封装提取、接口同步都只在这里发生，
//!      core 的其它模块看不到「层次」这个概念。
//!
//! 所有编辑都走 `edit()` 这一个入口，它负责「存快照 → 改图纸 → 重建仿真」。
//! 新增编辑操作只要写一行，不会再漏掉任何收尾工作。

use std::collections::{HashMap, VecDeque};

use crate::board::{Board, Netlist, Point, NO_NET, NO_SUB};
use crate::defs::{DefId, Dir, Params, PinDef};
use crate::elaborate::{self, Flat};
use crate::engine::Engine;
use crate::examples;
use crate::save::{Project, SaveError};
use crate::values::{NetValue, Width};
use crate::view::{build_view_with, CircuitView, NetMap};

/// 波形记录上限（拍）
pub const WAVE_LIMIT: usize = 4096;
/// 同时观察的网络数上限
pub const WAVE_WATCH_LIMIT: usize = 64;

/// 一条被观察的网络波形
#[derive(Clone, Debug)]
pub struct WaveTrace {
    pub net: u32,
    pub name: String,
    pub width: Width,
    pub samples: Vec<u64>,
}

/// 撤销快照。
///
/// 存的是**整个工程的图纸集合**而不是单张图纸：封装会把元件从一张图纸搬到
/// 另一张，新建/删除图纸也会动到别的图纸的引用，只存当前图纸根本回不去。
#[derive(Clone)]
struct Snapshot {
    boards: Vec<Board>,
    path: Vec<u32>,
}

/// 编辑会话：工程 + 引擎 + 视图 + 命令历史
pub struct Session {
    project: Project,
    engine: Engine,
    flat: Flat,
    nls: Vec<Netlist>,
    view: CircuitView,
    /// 当前编辑的图纸索引。可以直接打开图纸库里的任意一张——新画的子电路
    /// 还没被任何实例引用，只靠 enter_sub 是进不去的。
    active: u32,
    /// 从根到 active 的实例路径（面包屑用；图纸未被实例化时为空）
    path: Vec<u32>,
    /// 每次编辑 +1；壳据此判断几何是否变了
    revision: u64,
    undo: VecDeque<Snapshot>,
    redo: Vec<Snapshot>,
    wave: Vec<WaveTrace>,
    wave_buf: Vec<u64>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        let mut project = Project::new("未命名工程");
        let mut b = Board::new();
        b.name = "主图纸".to_string();
        project.add_board(b);
        Self::from_project(project)
    }

    pub fn from_project(mut project: Project) -> Self {
        if project.boards.is_empty() {
            project.add_board(Board::new());
        }
        if project.main_board >= project.boards.len() {
            project.main_board = 0;
        }
        for (i, b) in project.boards.iter_mut().enumerate() {
            if b.name.is_empty() {
                b.name = if i == project.main_board {
                    "主图纸".to_string()
                } else {
                    format!("图纸{}", i + 1)
                };
            }
        }
        let root = project.main_board as u32;
        let mut s = Self {
            project,
            engine: Engine::new(),
            flat: Flat::default(),
            nls: Vec::new(),
            view: CircuitView::default(),
            active: root,
            path: Vec::new(),
            revision: 1,
            undo: VecDeque::new(),
            redo: Vec::new(),
            wave: Vec::new(),
            wave_buf: Vec::new(),
        };
        s.rebuild();
        s
    }

    // -----------------------------------------------------------------------
    // 读取
    // -----------------------------------------------------------------------

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// 当前编辑的图纸索引
    pub fn board_index(&self) -> u32 {
        self.current_board()
    }

    pub fn board(&self) -> &Board {
        &self.project.boards[self.current_board() as usize]
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn view(&self) -> &CircuitView {
        &self.view
    }

    pub fn tick(&self) -> u64 {
        self.engine.tick_count()
    }

    /// 编辑版本号（每次编辑 +1）
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 层级调试路径（ADR-25）：图纸名 / 实例名 / 引脚名
    pub fn hierarchy_path(&self, inst: u32, pin: &str) -> String {
        let mut parts = self.breadcrumb(0);
        parts.push(self.board().instance_name(inst).unwrap_or("?").to_string());
        parts.push(pin.to_string());
        parts.join("/")
    }

    /// 沿 path 解析出每一层的图纸索引（含根）；遇到失效引用就停在最后一层。
    ///
    /// 面包屑、跳层都要走这条链，逻辑只写一遍。
    fn path_boards(&self) -> Vec<u32> {
        let n = self.project.boards.len();
        let mut b = self.project.main_board as u32;
        let mut v = vec![b];
        for &i in &self.path {
            let Some(inst) = self.project.boards[b as usize].instances.get(i as usize) else {
                break;
            };
            if inst.sub == NO_SUB || inst.sub as usize >= n {
                break;
            }
            b = inst.sub;
            v.push(b);
        }
        v
    }

    /// 从根到当前图纸的路径（图纸名）
    pub fn breadcrumb(&self, lang: u32) -> Vec<String> {
        self.path_boards()
            .iter()
            .map(|&b| self.board_label(b, lang))
            .collect()
    }

    /// 进入子电路的层数（0 = 根图纸）
    pub fn depth(&self) -> usize {
        self.path.len()
    }

    /// 图纸显示名：英文模式下优先用 name_en，没有就回落原名
    pub fn board_label(&self, idx: u32, lang: u32) -> String {
        let Some(b) = self.project.boards.get(idx as usize) else {
            return String::new();
        };
        if lang == 0 {
            return b.name.clone();
        }
        if !b.name_en.is_empty() {
            return b.name_en.clone();
        }
        // 程序自己补的默认名也给一份英文；用户起过的名字原样返回——
        // 界面语言不该改写用户的数据。
        match b.name.as_str() {
            "主图纸" => "Main".to_string(),
            "子电路" => "Sub-circuit".to_string(),
            other => match other.strip_prefix("图纸") {
                Some(n) => format!("Sheet {n}"),
                None => b.name.clone(),
            },
        }
    }

    /// 全部图纸名（图纸库用）
    pub fn board_names(&self, lang: u32) -> Vec<String> {
        (0..self.project.boards.len() as u32)
            .map(|i| self.board_label(i, lang))
            .collect()
    }

    /// 某张图纸被引用了几次
    pub fn board_refs(&self, idx: u32) -> usize {
        self.project
            .boards
            .iter()
            .map(|b| b.instances.iter().filter(|i| i.sub == idx).count())
            .sum()
    }

    /// 读某个实例的某个引脚当前值。
    ///
    /// 刻意读**引脚**而不是「实例输出」——探针这类纯观察组件不参与求值，
    /// 没有输出可读，但它的输入引脚一样有值。
    pub fn pin_value(&self, inst: u32, slot: usize) -> NetValue {
        let b = self.current_board();
        let gi = self.flat.global_inst(b, inst);
        if gi == u32::MAX {
            return NetValue::ZERO;
        }
        let base = self.flat.pin_start.get(gi as usize).copied().unwrap_or(0);
        self.engine.pin_value(base + slot as u32)
    }

    /// 当前图纸上某实例的存储器内容（RAM / ROM / 显示屏帧缓冲）。
    ///
    /// 显示屏的画面就活在这里：状态归 core 所有，壳只读不写，
    /// 于是「编辑即重置」「撤销」「读档」自动把画面一并复位。
    pub fn instance_mem(&self, inst: u32) -> &[u32] {
        let gi = self.flat.global_inst(self.current_board(), inst);
        if gi == u32::MAX {
            return &[];
        }
        self.engine.instance_mem(gi)
    }

    pub fn net_value(&self, net: u32) -> NetValue {
        if net == NO_NET {
            NetValue::ZERO
        } else {
            self.engine.net_value(net)
        }
    }

    // -----------------------------------------------------------------------
    // 编辑（全部经由 edit()，自动获得撤销 + 重置）
    // -----------------------------------------------------------------------

    pub fn add_component(&mut self, def: DefId, x: i32, y: i32) -> u32 {
        self.edit(|b| b.add_instance(def, def.default_params(), x, y))
    }

    pub fn remove_component(&mut self, id: u32) -> bool {
        self.edit(|b| b.remove_instance(id))
    }

    pub fn move_component(&mut self, id: u32, x: i32, y: i32) {
        self.edit(|b| b.set_pos(id, x, y));
    }

    pub fn rotate_component(&mut self, id: u32) {
        self.edit(|b| b.rotate(id));
    }

    pub fn set_params(&mut self, id: u32, params: Params) {
        self.edit(|b| b.set_params(id, params));
    }

    /// 按名改一个参数（键名见 DefId::params）。
    ///
    /// 未知键返回 false；**值没有实际变化时不产生编辑**（否则撤销栈里全是空操作）。
    pub fn set_param(&mut self, id: u32, key: &str, value: i64) -> bool {
        let Some(old) = self.board().instance(id).map(|i| i.params) else {
            return false;
        };
        let mut next = old;
        if !next.set_named(key, value) {
            return false;
        }
        if next == old {
            return true;
        }
        self.set_params(id, next);
        true
    }

    pub fn set_display_name(&mut self, id: u32, name: &str) {
        self.edit(|b| {
            if let Some(inst) = b.instance_mut(id) {
                inst.display_name = name.to_string();
            }
        });
    }

    pub fn connect_pins(&mut self, from: (u32, usize), to: (u32, usize)) -> bool {
        self.edit(|b| b.connect_pins(from, to))
    }

    pub fn remove_wire(&mut self, index: u32) -> bool {
        self.edit(|b| b.remove_wire(index))
    }

    pub fn add_annotation(&mut self, x: i32, y: i32, text: &str) -> u32 {
        self.edit(|b| b.add_annotation(x, y, text))
    }

    pub fn remove_annotation(&mut self, id: u32) -> bool {
        self.edit(|b| b.remove_annotation(id))
    }

    pub fn set_annotation_text(&mut self, id: u32, text: &str) {
        self.edit(|b| {
            if let Some(a) = b.annotations.get_mut(id as usize) {
                a.text = text.to_string();
            }
        });
    }

    pub fn move_annotation(&mut self, id: u32, x: i32, y: i32) {
        self.edit(|b| {
            if let Some(a) = b.annotations.get_mut(id as usize) {
                a.x = x;
                a.y = y;
            }
        });
    }

    /// 网络标签（v4 §8）：同名即相连
    pub fn add_label(&mut self, x: i32, y: i32, name: &str) -> u32 {
        self.edit(|b| b.add_label(x, y, name))
    }

    pub fn remove_label(&mut self, id: u32) -> bool {
        self.edit(|b| b.remove_label(id))
    }

    pub fn set_label_name(&mut self, id: u32, name: &str) {
        self.edit(|b| {
            if let Some(l) = b.labels.get_mut(id as usize) {
                l.name = name.to_string();
            }
        });
    }

    /// 批量编辑：在同一个闭包里做多次改动，**只重建一次仿真**。
    ///
    /// 构造电路（载入示例、粘贴、脚本生成）必须走这里——逐个调用编辑方法会让
    /// 每次改动都触发一次网表推导与引擎重建，万级电路下是平方级的浪费。
    pub fn edit_batch<R>(&mut self, f: impl FnOnce(&mut Board) -> R) -> R {
        self.edit(f)
    }

    /// 用内置示例替换当前工程（层次示例会带进来多张图纸）
    pub fn load_example(&mut self, id: &str) -> bool {
        let Some(p) = examples::build(id) else {
            return false;
        };
        self.path.clear();
        self.project.main_board = p.main_board;
        self.active = p.main_board as u32;
        self.edit_project(|bs| *bs = p.boards);
        true
    }

    // -----------------------------------------------------------------------
    // 层次（ADR-28）
    // -----------------------------------------------------------------------

    /// 新建一张空图纸，返回其索引
    pub fn add_board(&mut self, name: &str) -> u32 {
        let n = self.project.boards.len() as u32;
        let name = if name.trim().is_empty() { format!("子电路{n}") } else { name.to_string() };
        self.edit_project(|bs| {
            let mut b = Board::new();
            b.name = name;
            bs.push(b);
        });
        n
    }

    pub fn set_board_name(&mut self, idx: u32, name: &str) -> bool {
        if idx as usize >= self.project.boards.len() || name.trim().is_empty() {
            return false;
        }
        let name = name.to_string();
        self.edit_project(|bs| bs[idx as usize].name = name);
        true
    }

    /// 删除图纸。删除后原先引用它的实例一并消失，其余引用索引前移。
    pub fn remove_board(&mut self, idx: u32) -> bool {
        if self.project.boards.len() <= 1 || idx as usize >= self.project.boards.len() {
            return false;
        }
        self.edit_project(|bs| {
            // 走 remove_instance 而不是 retain：后者只抹实例，会把导线留成孤儿
            for b in bs.iter_mut() {
                let victims: Vec<u32> = b
                    .instances
                    .iter()
                    .enumerate()
                    .filter(|(_, inst)| inst.sub == idx)
                    .map(|(i, _)| i as u32)
                    .collect();
                for &v in victims.iter().rev() {
                    b.remove_instance(v);
                }
            }
            bs.remove(idx as usize);
            for b in bs.iter_mut() {
                for inst in b.instances.iter_mut() {
                    if inst.sub != NO_SUB && inst.sub > idx {
                        inst.sub -= 1;
                    }
                }
            }
        });
        true
    }

    /// 在当前图纸放一个子电路实例
    pub fn add_sub_instance(&mut self, sub: u32, x: i32, y: i32) -> Option<u32> {
        if sub as usize >= self.project.boards.len() || sub == self.current_board() {
            return None;
        }
        let name = self.project.boards[sub as usize].name.clone();
        Some(self.edit(|b| b.add_sub_instance(sub, &name, x, y)))
    }

    /// 双击进入子电路：path 追加一层
    pub fn enter_sub(&mut self, inst: u32) -> bool {
        let Some(i) = self.board().instance(inst) else {
            return false;
        };
        if i.sub == NO_SUB || i.sub as usize >= self.project.boards.len() {
            return false;
        }
        self.active = i.sub;
        self.path.push(inst);
        self.refresh_view();
        self.revision += 1;
        true
    }

    /// 退回第 depth 层（0 = 根图纸）
    pub fn goto_depth(&mut self, depth: usize) -> bool {
        if depth > self.path.len() {
            return false;
        }
        self.path.truncate(depth);
        let root = self.project.main_board as u32;
        self.active = self.path_boards().last().copied().unwrap_or(root);
        self.refresh_view();
        self.revision += 1;
        true
    }

    /// 直接打开一张图纸（图纸库）。新图纸还没被实例化时只能这样进。
    pub fn open_board(&mut self, idx: u32) -> bool {
        if idx as usize >= self.project.boards.len() {
            return false;
        }
        self.active = idx;
        self.recompute_path();
        self.refresh_view();
        self.revision += 1;
        true
    }

    /// 把选中的元件提取成一张新图纸，原位置换成一个子电路实例。
    ///
    /// 这是 TC 的核心操作：先在根图纸把电路搭出来、跑通，再整体封装成一个元件，
    /// 之后就能像内置元件一样反复使用。跨边界的连线自动转成接口并重新接回。
    pub fn extract_to_sub(&mut self, sel: &[u32], name: &str) -> Option<u32> {
        let cur = self.current_board() as usize;
        let mut sel: Vec<u32> = sel.to_vec();
        sel.sort_unstable();
        sel.dedup();
        let total = self.project.boards[cur].instances.len() as u32;
        sel.retain(|&i| i < total);
        if sel.is_empty() {
            return None;
        }
        self.edit_project(|bs| extract(bs, cur, &sel, name))
    }

    /// 全工程 DRC：返回 (图纸索引, 问题)
    pub fn drc_all(&self) -> Vec<(u32, crate::drc::Issue)> {
        crate::drc::check_project(&self.project.boards)
    }

    /// 层次展开是否被截断（自引用 / 超过深度上限）
    pub fn hierarchy_truncated(&self) -> bool {
        self.flat.truncated
    }

    // -----------------------------------------------------------------------
    // 撤销 / 重做（ADR-9）
    // -----------------------------------------------------------------------

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop_back() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.project.boards = prev.boards;
        self.path = prev.path;
        self.after_edit();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push_back(self.snapshot());
        self.project.boards = next.boards;
        self.path = next.path;
        self.after_edit();
        true
    }

    // -----------------------------------------------------------------------
    // 仿真
    // -----------------------------------------------------------------------

    pub fn sim_tick(&mut self) {
        self.engine.tick();
        self.record_wave();
    }

    pub fn sim_run_for(&mut self, n: u64) {
        for _ in 0..n {
            self.engine.tick();
            self.record_wave();
        }
    }

    /// 拨开关：**不算编辑**，不重置仿真（它本就是运行期交互）
    pub fn set_input(&mut self, id: u32, value: u32) {
        let Some(p) = self.board().instance(id).map(|i| i.params) else {
            return;
        };
        self.board_mut().set_params(id, Params { value, ..p });
        let gi = self.flat.global_inst(self.current_board(), id);
        if gi != u32::MAX {
            self.engine.set_input(gi, value);
        }
        // 只刷值，不重建几何：拨开关不改变任何拓扑
        crate::view::refresh_values(&mut self.view, &self.engine);
    }

    pub fn toggle_input(&mut self, id: u32) {
        let Some(p) = self.board().instance(id).map(|i| i.params) else {
            return;
        };
        let next = if p.value != 0 { 0 } else { 1 };
        self.set_input(id, next);
    }

    /// 手动复位（效果与「编辑即重置」一致，但由用户主动触发）
    pub fn reset_sim(&mut self) {
        self.rebuild();
    }

    // -----------------------------------------------------------------------
    // 波形（§9.1）：观察若干网络，逐拍采样
    // -----------------------------------------------------------------------

    /// 观察一个网络。返回 false 表示网络号非法或观察数已满。
    pub fn wave_watch(&mut self, net: u32, name: &str) -> bool {
        if net == NO_NET || net as usize >= self.engine.net_count() {
            return false;
        }
        if self.wave.iter().any(|t| t.net == net) {
            return true;
        }
        if self.wave.len() >= WAVE_WATCH_LIMIT {
            return false;
        }
        let width = self.engine.net_width(net);
        let name = if name.trim().is_empty() { format!("net{net}") } else { name.to_string() };
        // 已经跑过的拍数先补齐，避免新旧波形错位
        let done = self.engine.tick_count() as usize;
        self.wave.push(WaveTrace { net, name, width, samples: vec![0; done] });
        true
    }

    pub fn wave_unwatch(&mut self, net: u32) {
        self.wave.retain(|t| t.net != net);
    }

    pub fn wave_clear(&mut self) {
        self.wave.clear();
    }

    pub fn wave_len(&self) -> usize {
        self.wave.len()
    }

    pub fn wave_trace(&self, i: usize) -> Option<&WaveTrace> {
        self.wave.get(i)
    }

    pub fn wave_saturated(&self) -> bool {
        self.wave.first().map(|t| t.samples.len() >= WAVE_LIMIT).unwrap_or(false)
    }

    /// VCD 导出（§9.1）：可直接喂给 GTKWave
    pub fn wave_vcd(&self) -> String {
        let mut s = String::from("$timescale 1ns $end\n$scope module logiclab $end\n");
        for (i, t) in self.wave.iter().enumerate() {
            s.push_str(&format!("$var wire {} v{} {} $end\n", t.width, i, t.name));
        }
        s.push_str("$upscope $end\n$enddefinitions $end\n");
        let n = self.wave.first().map(|t| t.samples.len()).unwrap_or(0);
        for tick in 0..n {
            s.push_str(&format!("#{tick}\n"));
            for (i, t) in self.wave.iter().enumerate() {
                let v = t.samples[tick];
                if t.width == 1 {
                    s.push_str(&format!("{}v{}\n", if v != 0 { 1 } else { 0 }, i));
                } else {
                    s.push_str(&format!("b{:b} v{}\n", v, i));
                }
            }
        }
        s
    }

    fn record_wave(&mut self) {
        if self.wave.is_empty() || self.wave[0].samples.len() >= WAVE_LIMIT {
            return;
        }
        self.wave_buf.clear();
        self.wave_buf.extend(self.wave.iter().map(|t| self.engine.net_value(t.net).val as u64));
        for (t, v) in self.wave.iter_mut().zip(self.wave_buf.iter()) {
            t.samples.push(*v);
        }
    }

    // -----------------------------------------------------------------------
    // 存档
    // -----------------------------------------------------------------------

    pub fn to_json_pretty(&self) -> Result<String, SaveError> {
        self.project.to_json_pretty()
    }

    pub fn to_json(&self) -> Result<String, SaveError> {
        self.project.to_json()
    }

    /// 载入工程（历史清空；等价于一次编辑）
    pub fn load_json(&mut self, s: &str) -> Result<(), SaveError> {
        let p = Project::from_json(s)?;
        *self = Self::from_project(p);
        Ok(())
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }

    // -----------------------------------------------------------------------
    // 内部
    // -----------------------------------------------------------------------

    fn snapshot(&self) -> Snapshot {
        Snapshot { boards: self.project.boards.clone(), path: self.path.clone() }
    }

    /// 当前编辑的图纸（索引越界时退回根图纸）
    fn current_board(&self) -> u32 {
        if (self.active as usize) < self.project.boards.len() {
            self.active
        } else {
            self.project.main_board as u32
        }
    }

    fn board_mut(&mut self) -> &mut Board {
        let b = self.current_board();
        &mut self.project.boards[b as usize]
    }

    /// **唯一的编辑入口**：存快照 → 改图纸 → 重建仿真（ADR-23 + ADR-9）
    fn edit<R>(&mut self, f: impl FnOnce(&mut Board) -> R) -> R {
        let cur = self.current_board() as usize;
        self.edit_project(|bs| f(&mut bs[cur]))
    }

    fn edit_project<R>(&mut self, f: impl FnOnce(&mut Vec<Board>) -> R) -> R {
        let before = self.snapshot();
        let result = f(&mut self.project.boards);
        self.push_history(before);
        self.after_edit();
        result
    }

    /// 撤销栈深度按电路规模自适应，避免大电路把内存吃满。
    fn push_history(&mut self, before: Snapshot) {
        let n: usize = before.boards.iter().map(|b| b.instances.len() + b.wires.len()).sum();
        let limit = if n > 2000 {
            16
        } else if n > 500 {
            48
        } else {
            128
        };
        self.undo.push_back(before);
        while self.undo.len() > limit {
            self.undo.pop_front();
        }
        // 新的编辑让「重做」失效
        self.redo.clear();
    }

    fn after_edit(&mut self) {
        self.fix_path();
        self.rebuild();
        self.revision += 1;
    }

    /// 图纸可能被删掉，active 先落到合法位置；再从根重算一次面包屑路径。
    fn fix_path(&mut self) {
        if self.active as usize >= self.project.boards.len() {
            self.active = self.project.main_board as u32;
        }
        self.recompute_path();
    }

    /// 从根 DFS 找一条到 active 的实例路径；找不到（图纸还没被实例化）就清空。
    fn recompute_path(&mut self) {
        let target = self.current_board();
        let root = self.project.main_board as u32;
        if target == root {
            self.path.clear();
            return;
        }
        let n = self.project.boards.len();
        let mut seen = vec![false; n];
        let mut stack: Vec<(u32, Vec<u32>)> = vec![(root, Vec::new())];
        while let Some((b, p)) = stack.pop() {
            let bi = b as usize;
            if bi >= n || seen[bi] {
                continue;
            }
            seen[bi] = true;
            for (k, inst) in self.project.boards[bi].instances.iter().enumerate() {
                if inst.sub == NO_SUB || inst.sub as usize >= n {
                    continue;
                }
                let mut np = p.clone();
                np.push(k as u32);
                if inst.sub == target {
                    self.path = np;
                    return;
                }
                stack.push((inst.sub, np));
            }
        }
        self.path.clear();
    }

    /// 把每张图纸的接口同步到引用它的实例上（ADR-28）。
    ///
    /// 一个电路里没有子电路实例时直接返回——普通电路零开销。
    fn sync_shapes(&mut self) {
        let n = self.project.boards.len();
        if !self.project.boards.iter().any(|b| b.instances.iter().any(|i| i.sub != NO_SUB)) {
            return;
        }
        let shapes: Vec<Vec<PinDef>> =
            self.project.boards.iter().map(elaborate::custom_pins).collect();
        for b in 0..n {
            let subs: Vec<(usize, u32)> = self.project.boards[b]
                .instances
                .iter()
                .enumerate()
                .filter(|(_, i)| i.sub != NO_SUB && (i.sub as usize) < n)
                .map(|(k, i)| (k, i.sub))
                .collect();
            for (k, sub) in subs {
                let pins = shapes[sub as usize].clone();
                let inst = &mut self.project.boards[b].instances[k];
                if !same_pins(&inst.pins, &pins) {
                    inst.pins = pins;
                }
            }
        }
    }

    /// 由当前工程重建引擎与视图。
    ///
    /// 展开只做一次：网表同时喂给引擎与视图，万级电路下重复推导就是白扔几毫秒。
    fn rebuild(&mut self) {
        self.sync_shapes();
        let root = self.project.main_board as u32;
        let (mut flat, nls) = elaborate::build(&self.project.boards, root);
        self.engine = Engine::new();
        self.engine.load_flat(&mut flat);
        self.flat = flat;
        self.nls = nls;
        self.refresh_view();
    }

    fn refresh_view(&mut self) {
        let b = self.current_board() as usize;
        if b >= self.nls.len() {
            self.view = CircuitView::default();
            return;
        }
        let map = self.flat.net_map.get(b).map(|v| v.as_slice());
        self.view = build_view_with(&self.project.boards[b], &self.nls[b], &NetMap { map }, &self.engine);
    }
}

/// 引脚布局是否等价（避免每次重建都重新分配一堆字符串）
fn same_pins(a: &[PinDef], b: &[PinDef]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.dx == y.dx && x.dy == y.dy && x.width == y.width && x.dir == y.dir && x.name == y.name
        })
}

/// 封装：把选中的元件搬进一张新图纸，原位置留下一个子电路实例。
///
/// 顺序很讲究——外部连线必须在删除选中元件**之前**接好，因为 connect_pins 走的是
/// 实例索引，而删除会让索引位移。导线本身是坐标折线，不受索引位移影响。
fn extract(bs: &mut Vec<Board>, cur: usize, sel: &[u32], name: &str) -> Option<u32> {
    // 引脚坐标 → (实例, 槽位)
    let mut sel_pin: HashMap<Point, (u32, usize)> = HashMap::new();
    let mut all_pin: HashMap<Point, (u32, usize)> = HashMap::new();
    for (ii, inst) in bs[cur].instances.iter().enumerate() {
        for (slot, p) in inst.pins.iter().enumerate() {
            let q = Point::new(inst.x + p.dx, inst.y + p.dy);
            all_pin.insert(q, (ii as u32, slot));
            if sel.binary_search(&(ii as u32)).is_ok() {
                sel_pin.insert(q, (ii as u32, slot));
            }
        }
    }
    if sel_pin.is_empty() {
        return None;
    }
    let (mut min_x, mut min_y) = (i32::MAX, i32::MAX);
    for &i in sel {
        let inst = &bs[cur].instances[i as usize];
        min_x = min_x.min(inst.x);
        min_y = min_y.min(inst.y);
    }

    // 导线分类：内部连接搬进子图纸，跨界的转成接口
    let mut inner: Vec<((u32, usize), (u32, usize))> = Vec::new();
    let mut cross: HashMap<(u32, usize), Vec<(u32, usize)>> = HashMap::new();
    let mut drop_wires: Vec<usize> = Vec::new();
    for (wi, w) in bs[cur].wires.iter().enumerate() {
        let (Some(a), Some(b)) = (w.points.first().copied(), w.points.last().copied()) else {
            continue;
        };
        let pa = sel_pin.get(&a).copied();
        let pb = sel_pin.get(&b).copied();
        match (pa, pb) {
            (Some(x), Some(y)) => {
                if x != y {
                    inner.push((x, y));
                }
                drop_wires.push(wi);
            }
            (Some(x), None) => {
                if let Some(&o) = all_pin.get(&b) {
                    cross.entry(x).or_default().push(o);
                    drop_wires.push(wi);
                }
            }
            (None, Some(y)) => {
                if let Some(&o) = all_pin.get(&a) {
                    cross.entry(y).or_default().push(o);
                    drop_wires.push(wi);
                }
            }
            (None, None) => {}
        }
    }
    drop_wires.sort_unstable();

    // ---- 新图纸 ----
    let mut nb = Board::new();
    nb.name = if name.trim().is_empty() { "子电路".to_string() } else { name.to_string() };
    let mut map: HashMap<u32, u32> = HashMap::new();
    for &i in sel {
        let src = &bs[cur].instances[i as usize];
        let nid = nb.add_instance(src.def, src.params, src.x - min_x, src.y - min_y);
        let dst = &mut nb.instances[nid as usize];
        dst.display_name = src.display_name.clone();
        dst.mem_init = src.mem_init.clone();
        dst.rot = src.rot;
        if src.sub != NO_SUB {
            // 嵌套的子电路实例：引脚缓存必须跟着搬，rebuild_pins 推不出来
            dst.sub = src.sub;
            dst.pins = src.pins.clone();
        } else {
            dst.rebuild_pins();
        }
        map.insert(i, nid);
    }
    for (x, y) in &inner {
        nb.connect_pins((map[&x.0], x.1), (map[&y.0], y.1));
    }

    // ---- 接口元件 ----
    let mut iface: HashMap<(u32, usize), u32> = HashMap::new();
    let mut keys: Vec<(u32, usize)> = cross.keys().copied().collect();
    keys.sort_unstable();
    let (mut nin, mut nout) = (0usize, 0usize);
    for k in keys {
        let src = &bs[cur].instances[k.0 as usize];
        let is_out = src.pins[k.1].dir == Dir::Out;
        let width = src.pins[k.1].width;
        let iname = if is_out {
            let s = format!("out{nout}");
            nout += 1;
            s
        } else {
            let s = format!("in{nin}");
            nin += 1;
            s
        };
        let def = if is_out { DefId::OutputPin } else { DefId::InputPin };
        let pi = nb.add_instance(def, Params::default().width(width as u32), -4, nin as i32 + nout as i32);
        nb.instances[pi as usize].display_name = iname;
        nb.connect_pins((pi, 0), (map[&k.0], k.1));
        iface.insert(k, pi);
    }

    // ---- 原位置放实例 ----
    bs.push(nb);
    let sub_idx = (bs.len() - 1) as u32;
    let label = if name.trim().is_empty() { "子电路".to_string() } else { name.to_string() };
    let inst = bs[cur].add_sub_instance(sub_idx, &label, min_x, min_y);
    let pins = elaborate::custom_pins(&bs[sub_idx as usize]);
    bs[cur].instances[inst as usize].pins = pins;

    // ---- 外部连线（此时选中元件还在，索引仍有效）----
    let sh = elaborate::shape(&bs[sub_idx as usize]);
    let n_in = sh.inputs.len();
    let mut conns: Vec<(usize, (u32, usize))> = Vec::new();
    for (k, outer) in &cross {
        let Some(&pi) = iface.get(k) else {
            continue;
        };
        let is_out = bs[cur].instances[k.0 as usize].pins[k.1].dir == Dir::Out;
        let slot = if is_out {
            sh.outputs.iter().position(|&x| x == pi).map(|p| p + n_in)
        } else {
            sh.inputs.iter().position(|&x| x == pi)
        };
        let Some(slot) = slot else {
            continue;
        };
        for &o in outer {
            conns.push((slot, o));
        }
    }
    for (slot, o) in conns {
        bs[cur].connect_pins((inst, slot), o);
    }

    // ---- 清掉原地的元件与导线 ----
    let mut wi = 0usize;
    bs[cur].wires.retain(|_| {
        let keep = drop_wires.binary_search(&wi).is_err();
        wi += 1;
        keep
    });
    let mut kept = Vec::with_capacity(bs[cur].instances.len());
    for (ii, it) in bs[cur].instances.drain(..).enumerate() {
        if sel.binary_search(&(ii as u32)).is_err() {
            kept.push(it);
        }
    }
    bs[cur].instances = kept;
    Some(sub_idx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::IssueKind;

    fn two_pin_circuit() -> Session {
        let mut s = Session::new();
        let sw = s.add_component(DefId::Switch, 0, 0);
        let led = s.add_component(DefId::Led, 8, 0);
        assert!(s.connect_pins((sw, 0), (led, 0)));
        s
    }

    #[test]
    fn edit_resets_simulation_to_power_on() {
        let mut s = two_pin_circuit();
        s.set_input(0, 1);
        s.sim_run_for(5);
        assert_eq!(s.pin_value(1, 0).get(1), 1);
        assert_eq!(s.tick(), 5);

        s.add_component(DefId::Not, 20, 0);
        assert_eq!(s.tick(), 0, "编辑必须把 tick 归零");
        assert_eq!(s.pin_value(1, 0).get(1), 0, "编辑后回到上电态");
    }

    #[test]
    fn toggle_input_does_not_reset_simulation() {
        let mut s = two_pin_circuit();
        s.sim_run_for(3);
        s.toggle_input(0);
        s.sim_run_for(2);
        assert_eq!(s.tick(), 5, "拨开关不算编辑，不该重置");
    }

    #[test]
    fn undo_redo_restores_board_exactly() {
        let mut s = Session::new();
        let initial = s.to_json().unwrap();
        assert!(!s.can_undo());

        s.add_component(DefId::And, 0, 0);
        s.add_component(DefId::Or, 4, 0);
        s.add_annotation(1, 1, "注释");
        s.add_label(0, 0, "CLK");
        let after = s.to_json().unwrap();
        assert_ne!(initial, after);

        for _ in 0..4 {
            assert!(s.undo());
        }
        assert!(!s.can_undo());
        assert_eq!(s.to_json().unwrap(), initial, "全量 undo 应回到初始态");
        for _ in 0..4 {
            assert!(s.redo());
        }
        assert!(!s.can_redo());
        assert_eq!(s.to_json().unwrap(), after, "重做应回到编辑后的状态");
    }

    #[test]
    fn undo_also_resets_simulation() {
        let mut s = two_pin_circuit();
        s.sim_run_for(4);
        s.add_component(DefId::Not, 20, 0);
        s.sim_run_for(3);
        assert!(s.undo());
        assert_eq!(s.tick(), 0, "撤销同样是编辑，必须重置仿真（ADR-23）");
    }

    #[test]
    fn new_edit_clears_redo_stack() {
        let mut s = Session::new();
        s.add_component(DefId::And, 0, 0);
        assert!(s.undo());
        assert!(s.can_redo());
        s.add_component(DefId::Or, 4, 0);
        assert!(!s.can_redo(), "新编辑应让重做失效");
    }

    #[test]
    fn unchanged_param_is_not_an_edit() {
        let mut s = Session::new();
        let id = s.add_component(DefId::And, 0, 0);
        let depth = undo_depth(&mut s);
        assert!(s.set_param(id, "width", 1), "写入与当前相同的值应返回成功");
        assert_eq!(undo_depth(&mut s), depth, "空操作不该增加历史深度");
        assert!(!s.set_param(id, "不存在的键", 1), "未知键应被拒绝");
    }

    fn undo_depth(s: &mut Session) -> usize {
        let mut n = 0;
        while s.undo() {
            n += 1;
        }
        while s.redo() {}
        n
    }

    #[test]
    fn instance_names_are_auto_assigned() {
        let mut s = Session::new();
        let a = s.add_component(DefId::Not, 0, 0);
        s.set_display_name(a, "我的反相器");
        assert_eq!(s.hierarchy_path(a, "Q"), "主图纸/我的反相器/Q");
    }

    #[test]
    fn net_labels_join_by_name() {
        let mut s = Session::new();
        // 开关输出引脚落在第一段导线的端点上，LED 输入引脚落在第二段的端点上
        let sw = s.add_component(DefId::Switch, 0, 6);
        let led = s.edit(|b| b.add_instance(DefId::Led, Params::default().width(1), 28, 6));
        s.edit(|b| {
            b.add_wire(vec![Point::new(2, 6), Point::new(10, 6)]);
            b.add_wire(vec![Point::new(20, 6), Point::new(28, 6)]);
        });
        s.add_label(6, 6, "BUS");
        s.add_label(24, 6, "BUS");
        let nl = s.board().compile();
        let a = nl.pin_net[nl.pin_start[sw as usize] as usize];
        let b = nl.pin_net[nl.pin_start[led as usize] as usize];
        assert_ne!(a, NO_NET);
        assert_eq!(a, b, "同名标签应把两段导线并成一个网络");
    }

    #[test]
    fn annotations_and_labels_survive_round_trip() {
        let mut s = Session::new();
        s.add_component(DefId::And, 0, 0);
        s.add_annotation(3, 5, "关键路径");
        s.add_label(1, 1, "CLK");
        let json = s.to_json_pretty().unwrap();

        let mut s2 = Session::new();
        s2.load_json(&json).unwrap();
        assert_eq!(s2.board().annotations[0].text, "关键路径");
        assert_eq!(s2.board().labels[0].name, "CLK");
        assert_eq!(s2.tick(), 0);
        assert!(!s2.can_undo(), "载入工程后历史应清空");
    }

    // -----------------------------------------------------------------------
    // 层次（ADR-28）
    // -----------------------------------------------------------------------

    /// 手工搭一个反相器子电路，并在根图纸用它驱动 LED
    fn inverter_session() -> (Session, u32) {
        let mut s = Session::new();
        let sub = s.add_board("反相器");
        assert!(s.open_board(sub));
        let a = s.add_component(DefId::InputPin, 0, 0);
        let n = s.add_component(DefId::Not, 6, 0);
        let y = s.add_component(DefId::OutputPin, 12, 0);
        s.set_display_name(a, "A");
        s.set_display_name(y, "Y");
        assert!(s.connect_pins((a, 0), (n, 0)));
        assert!(s.connect_pins((n, 1), (y, 0)));

        assert!(s.open_board(0));
        let sw = s.add_component(DefId::Switch, 0, 0);
        s.add_component(DefId::Led, 24, 0);
        let inst = s.add_sub_instance(sub, 10, 0).expect("放不下子电路实例");
        assert!(s.connect_pins((sw, 0), (inst, 0)));
        assert!(s.connect_pins((inst, 1), (1, 0)));
        (s, inst)
    }

    #[test]
    fn sub_circuit_shapes_follow_drawing_interface() {
        let (s, inst) = inverter_session();
        let it = s.board().instance(inst).unwrap();
        assert_eq!(it.def, DefId::Custom);
        assert_eq!(it.pins.len(), 2, "一个输入一个输出");
        assert_eq!(it.pins[0].dir, Dir::In);
        assert_eq!(it.pins[0].name, "A");
        assert_eq!(it.pins[1].dir, Dir::Out);
        assert_eq!(it.pins[1].name, "Y");
    }

    #[test]
    fn sub_circuit_drives_through_hierarchy() {
        let (mut s, _inst) = inverter_session();
        s.set_input(0, 1);
        s.sim_run_for(4);
        assert_eq!(s.pin_value(1, 0).get(1), 0, "开关 1 → 反相器 → LED 0");
        s.set_input(0, 0);
        s.sim_run_for(4);
        assert_eq!(s.pin_value(1, 0).get(1), 1, "开关 0 → LED 1");
    }

    #[test]
    fn hierarchy_is_visible_in_breadcrumb() {
        let (mut s, inst) = inverter_session();
        assert_eq!(s.depth(), 0);
        assert!(s.enter_sub(inst));
        assert_eq!(s.depth(), 1);
        assert_eq!(s.breadcrumb(0), vec!["主图纸", "反相器"]);
        assert_eq!(s.board().name, "反相器");
        assert!(s.goto_depth(0));
        assert_eq!(s.board().name, "主图纸");
    }

    #[test]
    fn a_drawing_cannot_contain_itself() {
        let mut s = Session::new();
        let sub = s.add_board("环");
        assert!(s.open_board(sub));
        assert!(s.add_sub_instance(sub, 0, 0).is_none(), "自引用必须被挡下");
        // 绕过门面强行造一个自引用，DRC 必须报出来
        s.edit_batch(|b| b.add_sub_instance(sub, "环", 0, 0));
        assert!(s
            .drc_all()
            .iter()
            .any(|(_, i)| i.kind == IssueKind::CircularReference));
    }

    #[test]
    fn extract_wraps_circuit_and_keeps_behaviour() {
        let mut s = Session::new();
        let sw = s.add_component(DefId::Switch, 0, 0);
        let n1 = s.add_component(DefId::Not, 8, 0);
        let n2 = s.add_component(DefId::Not, 16, 0);
        let led = s.add_component(DefId::Led, 24, 0);
        assert!(s.connect_pins((sw, 0), (n1, 0)));
        assert!(s.connect_pins((n1, 1), (n2, 0)));
        assert!(s.connect_pins((n2, 1), (led, 0)));
        s.set_input(sw, 1);
        s.sim_run_for(4);
        assert_eq!(s.pin_value(led, 0).get(1), 1);

        let sub = s.extract_to_sub(&[n1, n2], "双反相器").expect("封装失败");
        assert_eq!(s.project().boards.len(), 2);
        assert_eq!(s.board().instances.len(), 3, "开关 + 实例 + LED");
        let inner = &s.project().boards[sub as usize];
        assert_eq!(inner.name, "双反相器");
        assert_eq!(inner.instances.len(), 4, "两个反相器 + 两个接口");

        // 行为不变：开关 → 双反相器 → LED
        s.set_input(0, 1);
        s.sim_run_for(6);
        assert_eq!(s.pin_value(1, 0).get(1), 1, "两级反相抵消");
        s.set_input(0, 0);
        s.sim_run_for(6);
        assert_eq!(s.pin_value(1, 0).get(1), 0);
    }

    #[test]
    fn removing_a_drawing_drops_its_instances() {
        let (mut s, _) = inverter_session();
        assert_eq!(s.project().boards.len(), 2);
        assert!(s.remove_board(1));
        assert_eq!(s.project().boards.len(), 1);
        assert_eq!(s.board().instances.len(), 2, "引用它的实例一并消失");
    }

    #[test]
    fn hierarchy_survives_save_and_load() {
        let (mut s, _) = inverter_session();
        s.set_input(0, 1);
        s.sim_run_for(4);
        let json = s.to_json_pretty().unwrap();

        let mut s2 = Session::new();
        s2.load_json(&json).unwrap();
        assert_eq!(s2.project().boards.len(), 2);
        assert_eq!(s2.board().instances.len(), 3);
        assert_eq!(s2.board().instance(2).unwrap().pins.len(), 2, "引脚缓存要重建");
        s2.set_input(0, 1);
        s2.sim_run_for(4);
        assert_eq!(s2.pin_value(1, 0).get(1), 0, "读档后层次照样跑");
    }

    // -----------------------------------------------------------------------
    // 波形（§9.1）
    // -----------------------------------------------------------------------

    #[test]
    fn wave_records_samples_and_exports_vcd() {
        let mut s = two_pin_circuit();
        let net = s.view().wires[0].net;
        assert_ne!(net, NO_NET);
        assert!(s.wave_watch(net, "sig"));
        s.sim_run_for(4);
        s.set_input(0, 1);
        s.sim_run_for(4);

        let t = s.wave_trace(0).unwrap();
        assert_eq!(t.samples.len(), 8, "逐拍采样");
        assert_eq!(&t.samples[..4], &[0, 0, 0, 0]);
        assert_eq!(t.samples[7], 1, "拨开关后一拍内翻高并保持");

        let vcd = s.wave_vcd();
        assert!(vcd.contains("$var wire 1 v0 sig $end"));
        assert!(vcd.contains("#7"));
    }

    #[test]
    fn wave_watch_rejects_unknown_net() {
        let mut s = two_pin_circuit();
        assert!(!s.wave_watch(NO_NET, "x"));
        assert!(!s.wave_watch(9999, "x"));
        assert_eq!(s.wave_len(), 0);
    }

    #[test]
    fn removing_component_also_removes_its_wires() {
        let mut s = Session::new();
        let sw = s.add_component(DefId::Switch, 0, 0);
        let led = s.add_component(DefId::Led, 8, 0);
        assert!(s.connect_pins((sw, 0), (led, 0)));
        assert_eq!(s.board().wires.len(), 1);

        s.remove_component(sw);
        assert_eq!(s.board().instances.len(), 1);
        assert_eq!(s.board().wires.len(), 0, "删元件必须连带删掉接在它引脚上的导线");

        assert!(s.undo());
        assert_eq!(s.board().wires.len(), 1, "撤销要把线还回来");
    }

    /// 只是从导线中段路过的元件被删掉时，那条线两端都还在，不该跟着消失
    #[test]
    fn removing_component_keeps_pass_through_wires() {
        let mut s = Session::new();
        s.add_component(DefId::Buffer, 0, 0); // 输出脚落在 (2,0)
        s.edit(|b| {
            b.add_wire(vec![Point::new(2, 0), Point::new(30, 0)]);
        });
        let led = s.edit(|b| b.add_instance(DefId::Led, Params::default().width(1), 10, 0));
        assert_eq!(s.board().wires.len(), 1);

        s.remove_component(led);
        assert_eq!(s.board().wires.len(), 1, "路过引脚的导线不该被剪断");
    }

    #[test]
    fn removing_drawing_cleans_dependent_wires() {
        let mut s = Session::new();
        let sub = s.add_board("子");
        assert!(s.open_board(sub));
        let a = s.add_component(DefId::InputPin, 0, 0);
        let y = s.add_component(DefId::OutputPin, 8, 0);
        assert!(s.connect_pins((a, 0), (y, 0)));

        assert!(s.open_board(0));
        let sw = s.add_component(DefId::Switch, 0, 0);
        let inst = s.add_sub_instance(sub, 10, 0).expect("放不下实例");
        assert!(s.connect_pins((sw, 0), (inst, 0)));
        assert_eq!(s.board().wires.len(), 1);

        assert!(s.remove_board(sub));
        assert_eq!(s.board().instances.len(), 1, "引用它的实例一并消失");
        assert_eq!(s.board().wires.len(), 0, "接在实例上的导线也要消失");
    }

    #[test]
    fn batch_removal_cleans_wires_too() {
        let mut s = Session::new();
        let a = s.add_component(DefId::Not, 0, 0);
        let b = s.add_component(DefId::Not, 8, 0);
        let c = s.add_component(DefId::Not, 16, 0);
        assert!(s.connect_pins((a, 1), (b, 0)));
        assert!(s.connect_pins((b, 1), (c, 0)));
        assert_eq!(s.board().wires.len(), 2);

        // 桥接层的批量删除就是这么做的：降序逐个删
        s.edit_batch(|bd| {
            bd.remove_instance(1);
            bd.remove_instance(0);
        });
        assert_eq!(s.board().instances.len(), 1);
        assert_eq!(s.board().wires.len(), 0, "批量删除同样不留孤儿导线");
    }
}

