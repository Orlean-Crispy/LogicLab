//! LogicLab 的 Godot 桥（v3 §3.2 的 godot-shell）
//!
//! 这一层刻意做薄：所有逻辑都在纯 Rust 的 core 里，这里只做两件事——
//!   1. 把 GDScript 的调用翻译成 core 的编辑 / 仿真命令
//!   2. 把 core 的 CircuitView 摊平成 Godot 的扁平数组（PackedInt64Array）
//!
//! 换壳时只需重写这一层与渲染器，core 一行不改。

use godot::prelude::*;

use logiclab_core::board::{Board, NO_NET};
use logiclab_core::defs::{DefId, ParamKind, Params};
use logiclab_core::drc;
use logiclab_core::examples;
use logiclab_core::engine::Engine;
use logiclab_core::save::Project;
use logiclab_core::values::normalize_width;
use logiclab_core::view::{build_view, dir_code, CircuitView};

/// 元件库在 DefId::ALL 里的下标（GDScript 用它去查名字）
fn def_index(def: DefId) -> i64 {
    DefId::ALL.iter().position(|&d| d == def).unwrap_or(0) as i64
}

#[derive(GodotClass)]
#[class(base = RefCounted)]
struct LogicLab {
    project: Project,
    engine: Engine,
    view: CircuitView,
    /// 每次改动几何就 +1，GDScript 据此决定是否重建顶点缓冲
    revision: i64,
}

#[godot_api]
impl IRefCounted for LogicLab {
    fn init(_base: Base<RefCounted>) -> Self {
        let mut project = Project::new("未命名工程");
        project.add_board(Board::new());
        let mut engine = Engine::new();
        engine.load_board(project.main().expect("刚加的图纸"));
        let view = build_view(project.main().expect("刚加的图纸"), &engine);
        Self { project, engine, view, revision: 1 }
    }
}

#[godot_api]
impl LogicLab {
    // -----------------------------------------------------------------------
    // 内部
    // -----------------------------------------------------------------------

    #[func]
    fn _rebuild(&mut self) {
        let main = self.project.main_board;
        let board = &self.project.boards[main];
        self.engine.load_board(board);
        self.view = build_view(board, &self.engine);
        self.revision += 1;
    }

    fn board(&self) -> &Board {
        &self.project.boards[self.project.main_board]
    }

    fn board_mut(&mut self) -> &mut Board {
        let main = self.project.main_board;
        &mut self.project.boards[main]
    }

    // -----------------------------------------------------------------------
    // 元件库（唯一数据源是 core 的 DefId::ALL）
    // -----------------------------------------------------------------------

    #[func]
    fn library_ids(&self) -> PackedStringArray {
        DefId::ALL.iter().map(|d| GString::from(d.id())).collect()
    }

    #[func]
    fn library_labels(&self) -> PackedStringArray {
        DefId::ALL.iter().map(|d| GString::from(d.label())).collect()
    }

    #[func]
    fn library_categories(&self) -> PackedStringArray {
        DefId::ALL
            .iter()
            .map(|d| GString::from(d.category().label()))
            .collect()
    }

    /// 每类一个紧凑编码，供壳查颜色表
    #[func]
    fn library_category_codes(&self) -> PackedInt64Array {
        DefId::ALL.iter().map(|d| d.category().code()).collect()
    }

    // -----------------------------------------------------------------------
    // 编辑
    // -----------------------------------------------------------------------

    /// 放置元件，返回实例 id（-1 表示未知类型）
    #[func]
    fn add_component(&mut self, def_id: GString, x: i32, y: i32) -> i32 {
        let Some(def) = DefId::from_id(&def_id.to_string()) else {
            return -1;
        };
        let id = self.board_mut().add_instance(def, def.default_params(), x, y);
        self._rebuild();
        id as i32
    }

    #[func]
    fn remove_component(&mut self, id: i32) -> bool {
        let ok = self.board_mut().remove_instance(id as u32);
        if ok {
            self._rebuild();
        }
        ok
    }

    #[func]
    fn move_component(&mut self, id: i32, x: i32, y: i32) {
        self.board_mut().set_pos(id as u32, x, y);
        self._rebuild();
    }

    #[func]
    fn rotate_component(&mut self, id: i32) {
        self.board_mut().rotate(id as u32);
        self._rebuild();
    }

    /// 改位宽（会重建引脚布局）
    #[func]
    fn set_width(&mut self, id: i32, width: i32) {
        if let Some(p) = self.board().instance(id as u32).map(|i| i.params) {
            let np = Params { width: normalize_width(width.max(0) as u32), ..p };
            self.board_mut().set_params(id as u32, np);
            self._rebuild();
        }
    }

    /// 改门的输入数
    #[func]
    fn set_inputs(&mut self, id: i32, n: i32) {
        if let Some(p) = self.board().instance(id as u32).map(|i| i.params) {
            let np = Params { inputs: n.clamp(2, 8) as u8, ..p };
            self.board_mut().set_params(id as u32, np);
            self._rebuild();
        }
    }

    /// 改常量值（只影响值，不动几何）
    #[func]
    fn set_value(&mut self, id: i32, value: i32) {
        if let Some(p) = self.board().instance(id as u32).map(|i| i.params) {
            let np = Params { value: value.max(0) as u32, ..p };
            self.board_mut().set_params(id as u32, np);
            self.engine.set_input(id as u32, value.max(0) as u32);
        }
    }

    /// 从引脚拉到引脚。返回 false 表示没能找到不误连的路径。
    #[func]
    fn connect_pins(&mut self, ai: i32, a_slot: i32, bi: i32, b_slot: i32) -> bool {
        let ok = self
            .board_mut()
            .connect_pins((ai as u32, a_slot as usize), (bi as u32, b_slot as usize));
        self._rebuild();
        ok
    }

    #[func]
    fn remove_wire(&mut self, index: i32) -> bool {
        let ok = self.board_mut().remove_wire(index as u32);
        if ok {
            self._rebuild();
        }
        ok
    }

    /// 剪断某点附近的导线
    #[func]
    fn remove_wires_at(&mut self, x: i32, y: i32, radius: i32) -> bool {
        let hit = self.board().pick_wire(x, y, radius.max(0));
        match hit {
            Some(i) => {
                self.board_mut().remove_wire(i);
                self._rebuild();
                true
            }
            None => false,
        }
    }

    // -----------------------------------------------------------------------
    // 仿真
    // -----------------------------------------------------------------------

    /// 开关 / 按钮 / 常量：写入新值（下一拍生效）
    #[func]
    fn set_input(&mut self, id: i32, value: i32) {
        if let Some(p) = self.board().instance(id as u32).map(|i| i.params) {
            let np = Params { value: value.max(0) as u32, ..p };
            self.board_mut().set_params(id as u32, np);
            self.engine.set_input(id as u32, value.max(0) as u32);
        }
    }

    /// 开关翻转（0 与 1 互换）
    #[func]
    fn toggle_input(&mut self, id: i32) {
        let Some(p) = self.board().instance(id as u32).map(|i| i.params) else {
            return;
        };
        let next = if p.value != 0 { 0 } else { 1 };
        self.set_input(id, next as i32);
    }

    #[func]
    fn tick(&mut self) {
        self.engine.tick();
    }

    #[func]
    fn run_for(&mut self, n: i32) {
        self.engine.run_for(n.max(0) as u64);
    }

    #[func]
    fn reset_sim(&mut self) {
        self.engine.reset();
    }

    #[func]
    fn tick_count(&self) -> i64 {
        self.engine.tick_count() as i64
    }

    // -----------------------------------------------------------------------
    // 视图（扁平数组，避免每帧构造上千个对象）
    // -----------------------------------------------------------------------

    #[func]
    fn revision(&self) -> i64 {
        self.revision
    }

    /// [inst, def_idx, x, y, w, h, rot] × N
    #[func]
    fn components(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for c in &self.view.components {
            out.push(c.inst as i64);
            out.push(def_index(c.def));
            out.push(c.x as i64);
            out.push(c.y as i64);
            out.push(c.w as i64);
            out.push(c.h as i64);
            out.push(c.rot as i64);
        }
        out
    }

    /// [inst, slot, x, y, dir, width, net] × M
    #[func]
    fn pins(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for p in &self.view.pins {
            out.push(p.inst as i64);
            out.push(p.slot as i64);
            out.push(p.x as i64);
            out.push(p.y as i64);
            out.push(dir_code(p.dir));
            out.push(p.width as i64);
            out.push(p.net as i64);
        }
        out
    }

    /// [start, len, net, status, width] × W（start / len 指向 wire_points）
    #[func]
    fn wire_ranges(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        let mut start = 0i64;
        for w in &self.view.wires {
            let len = w.points.len() as i64;
            out.push(start);
            out.push(len);
            out.push(w.net as i64);
            out.push(w.status.code());
            out.push(w.width as i64);
            start += len * 2;
        }
        out
    }

    /// [x, y] × P（所有导线的点按顺序摊平）
    #[func]
    fn wire_points(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for w in &self.view.wires {
            for p in &w.points {
                out.push(p.x as i64);
                out.push(p.y as i64);
            }
        }
        out
    }

    /// [val, unk] × net_count —— 只在编辑（revision 变化）后取一次
    #[func]
    fn net_values(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for n in 0..self.view.net_count {
            let v = self.engine.net_value(n);
            out.push(v.val as i64);
            out.push(v.unk as i64);
        }
        out
    }

    /// [net, val, unk] × C —— 本拍变化的网络（增量刷新，通常很短）
    #[func]
    fn take_changed(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for &n in self.engine.changed_nets() {
            let v = self.engine.net_value(n);
            out.push(n as i64);
            out.push(v.val as i64);
            out.push(v.unk as i64);
        }
        out
    }

    #[func]
    fn net_count(&self) -> i64 {
        self.view.net_count as i64
    }

    /// [min_x, min_y, max_x, max_y]（空电路返回空数组）
    #[func]
    fn content_bounds(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        if let Some((min, max)) = self.view.bounds() {
            out.push(min.x as i64);
            out.push(min.y as i64);
            out.push(max.x as i64);
            out.push(max.y as i64);
        }
        out
    }

    #[func]
    fn net_value(&self, net: i32) -> i32 {
        self.engine.net_value(net.max(0) as u32).val as i32
    }

    // -----------------------------------------------------------------------
    // 命中测试（坐标是格坐标）
    // -----------------------------------------------------------------------

    /// [inst, slot]，未命中返回空
    #[func]
    fn pick_pin(&self, x: i32, y: i32, radius: i32) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        if let Some((inst, slot)) = self.board().pick_pin(x, y, radius.max(0)) {
            out.push(inst as i64);
            out.push(slot as i64);
        }
        out
    }

    #[func]
    fn pick_component(&self, x: i32, y: i32) -> i32 {
        self.board().pick_component(x, y).map_or(-1, |i| i as i32)
    }

    #[func]
    fn pick_wire(&self, x: i32, y: i32, radius: i32) -> i32 {
        self.board().pick_wire(x, y, radius.max(0)).map_or(-1, |i| i as i32)
    }

    // -----------------------------------------------------------------------
    // 元件信息 / 诊断
    // -----------------------------------------------------------------------

    /// [def_idx, width, inputs, value, opts, x, y, rot, in_count, out_count]
    #[func]
    fn component_info(&self, id: i32) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        let Some(inst) = self.board().instance(id as u32) else {
            return out;
        };
        let p = inst.params;
        out.push(def_index(inst.def));
        out.push(p.width as i64);
        out.push(p.inputs as i64);
        out.push(p.value as i64);
        out.push(p.opts as i64);
        out.push(inst.x as i64);
        out.push(inst.y as i64);
        out.push(inst.rot as i64);
        out.push(inst.def.in_count(&p) as i64);
        out.push(inst.def.out_count(&p) as i64);
        out
    }

    /// [supports_width, has_input_count, is_sequential, is_source]
    #[func]
    fn param_flags(&self, def_idx: i32) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        let def = DefId::ALL
            .get(def_idx.max(0) as usize)
            .copied()
            .unwrap_or(DefId::Not);
        out.push(i64::from(def.supports_width()));
        out.push(i64::from(def.has_input_count()));
        out.push(i64::from(def.is_sequential()));
        out.push(i64::from(def.is_source()));
        out
    }

    // -----------------------------------------------------------------------
    // 参数编辑（控件由 core 的参数描述驱动，壳里不硬编码组件知识）
    // -----------------------------------------------------------------------

    /// [choice / int / bool] 的编码见下方 param_kinds
    fn def_of(def_idx: i32) -> DefId {
        DefId::ALL
            .get(def_idx.max(0) as usize)
            .copied()
            .unwrap_or(DefId::Not)
    }

    /// [kind code...]，0 = 档位选择，1 = 整数范围，2 = 布尔开关
    #[func]
    fn param_kinds(&self, def_idx: i32) -> PackedInt64Array {
        DefId::params(Self::def_of(def_idx))
            .iter()
            .map(|p| match p.kind {
                ParamKind::Choice(_) => 0i64,
                ParamKind::Int { .. } => 1,
                ParamKind::Bool { .. } => 2,
            })
            .collect()
    }

    #[func]
    fn param_keys(&self, def_idx: i32) -> PackedStringArray {
        DefId::params(Self::def_of(def_idx))
            .iter()
            .map(|p| GString::from(p.key))
            .collect()
    }

    #[func]
    fn param_labels(&self, def_idx: i32) -> PackedStringArray {
        DefId::params(Self::def_of(def_idx))
            .iter()
            .map(|p| GString::from(p.label))
            .collect()
    }

    /// 整数与布尔的取值范围（档位选择项填 0）
    #[func]
    fn param_lo(&self, def_idx: i32) -> PackedInt64Array {
        DefId::params(Self::def_of(def_idx))
            .iter()
            .map(|p| match p.kind {
                ParamKind::Choice(_) => 0,
                ParamKind::Int { min, .. } => min,
                ParamKind::Bool { .. } => 0,
            })
            .collect()
    }

    #[func]
    fn param_hi(&self, def_idx: i32) -> PackedInt64Array {
        DefId::params(Self::def_of(def_idx))
            .iter()
            .map(|p| match p.kind {
                ParamKind::Choice(_) => 0,
                ParamKind::Int { max, .. } => max,
                ParamKind::Bool { .. } => 1,
            })
            .collect()
    }

    /// 档位选择的可用值，逗号分隔；非档位项为空串
    #[func]
    fn param_choices(&self, def_idx: i32) -> PackedStringArray {
        DefId::params(Self::def_of(def_idx))
            .iter()
            .map(|p| match p.kind {
                ParamKind::Choice(v) => {
                    GString::from(&v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","))
                }
                _ => GString::from(""),
            })
            .collect()
    }

    /// 某实例各参数的当前值（顺序与 param_keys 一致）
    #[func]
    fn param_values(&self, id: i32) -> PackedInt64Array {
        let Some(inst) = self.board().instance(id as u32) else {
            return PackedInt64Array::new();
        };
        DefId::params(inst.def)
            .iter()
            .map(|p| inst.params.get_named(p.key))
            .collect()
    }

    /// 改一个参数（键名来自 param_keys）
    #[func]
    fn set_param(&mut self, id: i32, key: GString, value: i32) -> bool {
        let Some(mut p) = self.board().instance(id as u32).map(|i| i.params) else {
            return false;
        };
        if !p.set_named(&key.to_string(), value as i64) {
            return false;
        }
        self.board_mut().set_params(id as u32, p);
        self._rebuild();
        true
    }

    // -----------------------------------------------------------------------
    // 内置示例
    // -----------------------------------------------------------------------

    #[func]
    fn example_ids(&self) -> PackedStringArray {
        examples::ALL.iter().map(|e| GString::from(e.id)).collect()
    }

    #[func]
    fn example_names(&self) -> PackedStringArray {
        examples::ALL.iter().map(|e| GString::from(e.name)).collect()
    }

    #[func]
    fn example_notes(&self) -> PackedStringArray {
        examples::ALL.iter().map(|e| GString::from(e.note)).collect()
    }

    /// 载入示例（替换当前图纸）
    #[func]
    fn load_example(&mut self, id: GString) -> bool {
        let Some(board) = examples::build(&id.to_string()) else {
            return false;
        };
        let main = self.project.main_board;
        self.project.boards[main] = board;
        self.engine = Engine::new();
        self._rebuild();
        true
    }

    // -----------------------------------------------------------------------
    // 设计规则检查
    // -----------------------------------------------------------------------

    /// [kind, severity, net, inst] × N（net / inst 为 -1 表示不适用）
    #[func]
    fn drc_issues(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for i in drc::check(self.board()) {
            out.push(i.kind.code());
            out.push(i.severity.code());
            out.push(if i.net == NO_NET { -1 } else { i.net as i64 });
            out.push(if i.inst == u32::MAX { -1 } else { i.inst as i64 });
        }
        out
    }

    #[func]
    fn drc_details(&self) -> PackedStringArray {
        drc::check(self.board())
            .iter()
            .map(|i| GString::from(&format!("[{}] {}", i.kind.label(), i.detail)))
            .collect()
    }

    /// [错误数, 警告数]
    #[func]
    fn drc_summary(&self) -> PackedInt64Array {
        let issues = drc::check(self.board());
        let errors = issues
            .iter()
            .filter(|i| i.severity == drc::Severity::Error)
            .count() as i64;
        let warnings = issues.len() as i64 - errors;
        PackedInt64Array::from(&[errors, warnings][..])
    }

    /// [instances, components, nets, pins, wires, tick, last_eval, revision]
    #[func]
    fn stats(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        out.push(self.project.main().map_or(0, |b| b.instances.len()) as i64);
        out.push(self.engine.component_count() as i64);
        out.push(self.engine.net_count() as i64);
        out.push(self.engine.pin_count() as i64);
        out.push(self.project.main().map_or(0, |b| b.wires.len()) as i64);
        out.push(self.engine.tick_count() as i64);
        out.push(self.engine.last_eval_count() as i64);
        out.push(self.revision);
        out
    }

    // -----------------------------------------------------------------------
    // 存取（文件读写归壳层；这里只做字符串进出）
    // -----------------------------------------------------------------------

    #[func]
    fn save_data(&self) -> GString {
        GString::from(self.project.to_json_pretty().unwrap_or_default().as_str())
    }

    #[func]
    fn load_data(&mut self, data: GString) -> bool {
        match Project::from_json(&data.to_string()) {
            Ok(mut p) => {
                if p.boards.is_empty() {
                    p.add_board(Board::new());
                }
                self.project = p;
                self.engine = Engine::new();
                self._rebuild();
                true
            }
            Err(_) => false,
        }
    }

    /// 新建一个空工程
    #[func]
    fn clear(&mut self) {
        let mut p = Project::new("未命名工程");
        p.add_board(Board::new());
        self.project = p;
        self.engine = Engine::new();
        self._rebuild();
    }
}

#[gdextension(entry_symbol = logiclab_entry)]
unsafe impl ExtensionLibrary for LogicLab {}
