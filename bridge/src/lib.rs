//! LogicLab 的 Godot 桥（v3 §3.2 的 godot-shell）
//!
//! 这一层刻意做薄：所有逻辑都在纯 Rust 的 core 里。这里只做两件事——
//!   1. 把 GDScript 的调用翻译成 core 的编辑 / 仿真命令
//!   2. 把 core 的 CircuitView 摊平成 Godot 的扁平数组（PackedInt64Array）
//!
//! "编辑即重置仿真"（ADR-23）由 core 的 Session 统一保证，这里不需要记得做任何事。
//! 换壳时只需重写这一层与渲染器，core 一行不改。

use godot::prelude::*;

use logiclab_core::board::NO_NET;
use logiclab_core::defs::{DefId, ParamKind};
use logiclab_core::drc;
use logiclab_core::examples;
use logiclab_core::session::Session;

/// 元件库在 DefId::ALL 里的下标（GDScript 用它去查名字）
fn def_index(def: DefId) -> i64 {
    DefId::ALL.iter().position(|&d| d == def).unwrap_or(0) as i64
}

/// 逗号拼接（UI 侧按逗号切分）
fn join_i64(values: &[i64]) -> String {
    values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")
}

fn net_code(net: u32) -> i64 {
    if net == NO_NET {
        -1
    } else {
        net as i64
    }
}

#[derive(GodotClass)]
#[class(base = RefCounted)]
struct LogicLab {
    session: Session,
}

#[godot_api]
impl IRefCounted for LogicLab {
    fn init(_base: Base<RefCounted>) -> Self {
        Self { session: Session::new() }
    }
}

#[godot_api]
impl LogicLab {
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

    #[func]
    fn library_category_codes(&self) -> PackedInt64Array {
        DefId::ALL.iter().map(|d| d.category().code()).collect()
    }

    // -----------------------------------------------------------------------
    // 编辑（Session 内部会按 ADR-23 重置仿真，壳无需关心）
    // -----------------------------------------------------------------------

    #[func]
    fn add_component(&mut self, def_id: GString, x: i32, y: i32) -> i32 {
        let Some(def) = DefId::from_id(&def_id.to_string()) else {
            return -1;
        };
        self.session.add_component(def, x, y) as i32
    }

    #[func]
    fn remove_component(&mut self, id: i32) -> bool {
        self.session.remove_component(id as u32)
    }

    #[func]
    fn move_component(&mut self, id: i32, x: i32, y: i32) {
        self.session.move_component(id as u32, x, y);
    }

    #[func]
    fn rotate_component(&mut self, id: i32) {
        self.session.rotate_component(id as u32);
    }

    #[func]
    fn set_display_name(&mut self, id: i32, name: GString) {
        self.session.set_display_name(id as u32, &name.to_string());
    }

    /// 改一个参数；键名来自 param_keys
    #[func]
    fn set_param(&mut self, id: i32, key: GString, value: i32) -> bool {
        self.session.set_param(id as u32, &key.to_string(), value as i64)
    }

    /// 从引脚拉到引脚。false 表示没能找到不误连的路径。
    #[func]
    fn connect_pins(&mut self, ai: i32, a_slot: i32, bi: i32, b_slot: i32) -> bool {
        self.session
            .connect_pins((ai as u32, a_slot as usize), (bi as u32, b_slot as usize))
    }

    #[func]
    fn remove_wire(&mut self, index: i32) -> bool {
        self.session.remove_wire(index as u32)
    }

    /// 剪断某点附近的导线
    #[func]
    fn remove_wires_at(&mut self, x: i32, y: i32, radius: i32) -> bool {
        match self.session.board().pick_wire(x, y, radius.max(0)) {
            Some(i) => self.session.remove_wire(i),
            None => false,
        }
    }

    /// 载入内置示例
    #[func]
    fn load_example(&mut self, id: GString) -> bool {
        self.session.load_example(&id.to_string())
    }

    // -----------------------------------------------------------------------
    // 撤销 / 重做（ADR-9）
    // -----------------------------------------------------------------------

    #[func]
    fn can_undo(&self) -> bool {
        self.session.can_undo()
    }

    #[func]
    fn can_redo(&self) -> bool {
        self.session.can_redo()
    }

    #[func]
    fn undo(&mut self) -> bool {
        self.session.undo()
    }

    #[func]
    fn redo(&mut self) -> bool {
        self.session.redo()
    }

    // -----------------------------------------------------------------------
    // 网络标签（v4 §8：同名即相连）
    // -----------------------------------------------------------------------

    #[func]
    fn add_label(&mut self, x: i32, y: i32, name: GString) -> i32 {
        self.session.add_label(x, y, &name.to_string()) as i32
    }

    #[func]
    fn remove_label(&mut self, id: i32) -> bool {
        self.session.remove_label(id as u32)
    }

    #[func]
    fn set_label_name(&mut self, id: i32, name: GString) {
        self.session.set_label_name(id as u32, &name.to_string());
    }

    #[func]
    fn pick_label(&self, x: i32, y: i32) -> i32 {
        self.session.board().pick_label(x, y).map_or(-1, |i| i as i32)
    }

    /// [x, y] × N（名字用 label_names 取）
    #[func]
    fn labels(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for l in &self.session.board().labels {
            out.push(l.x as i64);
            out.push(l.y as i64);
        }
        out
    }

    #[func]
    fn label_names(&self) -> PackedStringArray {
        self.session
            .board()
            .labels
            .iter()
            .map(|l| GString::from(&l.name))
            .collect()
    }

    // -----------------------------------------------------------------------
    // 文本注释（v4 §6.7）
    // -----------------------------------------------------------------------

    #[func]
    fn add_annotation(&mut self, x: i32, y: i32, text: GString) -> i32 {
        self.session.add_annotation(x, y, &text.to_string()) as i32
    }

    #[func]
    fn remove_annotation(&mut self, id: i32) -> bool {
        self.session.remove_annotation(id as u32)
    }

    #[func]
    fn set_annotation_text(&mut self, id: i32, text: GString) {
        self.session.set_annotation_text(id as u32, &text.to_string());
    }

    #[func]
    fn move_annotation(&mut self, id: i32, x: i32, y: i32) {
        self.session.move_annotation(id as u32, x, y);
    }

    /// [x, y, color] × N（文本用 annotation_texts 取）
    #[func]
    fn annotations(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for a in &self.session.view().annotations {
            out.push(a.x as i64);
            out.push(a.y as i64);
            out.push(0);
        }
        out
    }

    #[func]
    fn annotation_texts(&self) -> PackedStringArray {
        self.session
            .view()
            .annotations
            .iter()
            .map(|a| GString::from(&a.text))
            .collect()
    }

    // -----------------------------------------------------------------------
    // 参数描述（控件由 core 驱动，壳里不硬编码组件知识）
    // -----------------------------------------------------------------------

    fn def_of(def_idx: i32) -> DefId {
        DefId::ALL
            .get(def_idx.max(0) as usize)
            .copied()
            .unwrap_or(DefId::Not)
    }

    /// 0 = 档位选择，1 = 整数范围，2 = 布尔开关
    #[func]
    fn param_kinds(&self, def_idx: i32) -> PackedInt64Array {
        DefId::params(Self::def_of(def_idx))
            .iter()
            .map(|p| match p.kind {
                ParamKind::Choice { .. } => 0i64,
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

    #[func]
    fn param_lo(&self, def_idx: i32) -> PackedInt64Array {
        DefId::params(Self::def_of(def_idx))
            .iter()
            .map(|p| match p.kind {
                ParamKind::Choice { .. } => 0,
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
                ParamKind::Choice { .. } => 0,
                ParamKind::Int { max, .. } => max,
                ParamKind::Bool { .. } => 1,
            })
            .collect()
    }

    /// 档位选择的可选值，逗号分隔；非档位项为空串
    #[func]
    fn param_choices(&self, def_idx: i32) -> PackedStringArray {
        DefId::params(Self::def_of(def_idx))
            .iter()
            .map(|p| match p.kind {
                ParamKind::Choice { values, .. } => GString::from(&join_i64(values)),
                _ => GString::from(""),
            })
            .collect()
    }

    /// 档位的显示名（与 param_choices 一一对应）；无标签时回落到数字
    #[func]
    fn param_choice_labels(&self, def_idx: i32) -> PackedStringArray {
        DefId::params(Self::def_of(def_idx))
            .iter()
            .map(|p| match p.kind {
                ParamKind::Choice { values, labels } => {
                    if labels.is_empty() {
                        GString::from(&join_i64(values))
                    } else {
                        GString::from(&labels.join(","))
                    }
                }
                _ => GString::from(""),
            })
            .collect()
    }

    /// 某实例各参数的当前值（顺序与 param_keys 一致）
    #[func]
    fn param_values(&self, id: i32) -> PackedInt64Array {
        let Some(inst) = self.session.board().instance(id as u32) else {
            return PackedInt64Array::new();
        };
        DefId::params(inst.def)
            .iter()
            .map(|p| inst.params.get_named(p.key))
            .collect()
    }

    // -----------------------------------------------------------------------
    // 仿真
    // -----------------------------------------------------------------------

    #[func]
    fn set_input(&mut self, id: i32, value: i32) {
        self.session.set_input(id as u32, value.max(0) as u32);
    }

    #[func]
    fn toggle_input(&mut self, id: i32) {
        self.session.toggle_input(id as u32);
    }

    #[func]
    fn tick(&mut self) {
        self.session.sim_tick();
    }

    #[func]
    fn run_for(&mut self, n: i32) {
        self.session.sim_run_for(n.max(0) as u64);
    }

    #[func]
    fn reset_sim(&mut self) {
        self.session.reset_sim();
    }

    #[func]
    fn tick_count(&self) -> i64 {
        self.session.tick() as i64
    }

    #[func]
    fn revision(&self) -> i64 {
        self.session.revision() as i64
    }

    // -----------------------------------------------------------------------
    // 视图（扁平数组，避免每帧构造上千个对象）
    // -----------------------------------------------------------------------

    /// [inst, def_idx, x, y, w, h, rot] × N
    #[func]
    fn components(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for c in &self.session.view().components {
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
        for p in &self.session.view().pins {
            out.push(p.inst as i64);
            out.push(p.slot as i64);
            out.push(p.x as i64);
            out.push(p.y as i64);
            out.push(logiclab_core::view::dir_code(p.dir));
            out.push(p.width as i64);
            out.push(net_code(p.net));
        }
        out
    }

    /// [start, len, net, status, width] × W（start / len 指向 wire_points）
    #[func]
    fn wire_ranges(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        let mut start = 0i64;
        for w in &self.session.view().wires {
            let len = w.points.len() as i64;
            out.push(start);
            out.push(len);
            out.push(net_code(w.net));
            out.push(w.status.code());
            out.push(w.width as i64);
            start += len * 2;
        }
        out
    }

    /// [x, y] × P
    #[func]
    fn wire_points(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for w in &self.session.view().wires {
            for p in &w.points {
                out.push(p.x as i64);
                out.push(p.y as i64);
            }
        }
        out
    }

    /// [val, unk] × net_count（编辑后取一次全量）
    #[func]
    fn net_values(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for n in 0..self.session.view().net_count {
            let v = self.session.engine().net_value(n);
            out.push(v.val as i64);
            out.push(v.unk as i64);
        }
        out
    }

    /// [net, val, unk] × C —— 本拍变化的网络（增量刷新）
    #[func]
    fn take_changed(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for &n in self.session.engine().changed_nets() {
            let v = self.session.engine().net_value(n);
            out.push(n as i64);
            out.push(v.val as i64);
            out.push(v.unk as i64);
        }
        out
    }

    #[func]
    fn net_count(&self) -> i64 {
        self.session.view().net_count as i64
    }

    /// [min_x, min_y, max_x, max_y]（空电路返回空数组）
    #[func]
    fn content_bounds(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        if let Some((min, max)) = self.session.view().bounds() {
            out.push(min.x as i64);
            out.push(min.y as i64);
            out.push(max.x as i64);
            out.push(max.y as i64);
        }
        out
    }

    #[func]
    fn net_value(&self, net: i32) -> i32 {
        self.session.net_value(net.max(0) as u32).val as i32
    }

    // -----------------------------------------------------------------------
    // 命中测试（坐标是格坐标）
    // -----------------------------------------------------------------------

    /// [inst, slot]，未命中返回空
    #[func]
    fn pick_pin(&self, x: i32, y: i32, radius: i32) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        if let Some((inst, slot)) = self.session.board().pick_pin(x, y, radius.max(0)) {
            out.push(inst as i64);
            out.push(slot as i64);
        }
        out
    }

    #[func]
    fn pick_component(&self, x: i32, y: i32) -> i32 {
        self.session.board().pick_component(x, y).map_or(-1, |i| i as i32)
    }

    #[func]
    fn pick_wire(&self, x: i32, y: i32, radius: i32) -> i32 {
        self.session.board().pick_wire(x, y, radius.max(0)).map_or(-1, |i| i as i32)
    }

    #[func]
    fn pick_annotation(&self, x: i32, y: i32) -> i32 {
        self.session.board().pick_annotation(x, y).map_or(-1, |i| i as i32)
    }

    // -----------------------------------------------------------------------
    // 元件信息 / 诊断
    // -----------------------------------------------------------------------

    /// [def_idx, width, inputs, value, opts, x, y, rot, in_count, out_count]
    #[func]
    fn component_info(&self, id: i32) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        let Some(inst) = self.session.board().instance(id as u32) else {
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

    /// 实例名 inst_<n>（层级路径与导出命名，ADR-25）
    #[func]
    fn instance_name(&self, id: i32) -> GString {
        GString::from(self.session.board().instance_name(id as u32).unwrap_or(""))
    }

    /// [supports_width, has_input_count, is_sequential, is_source]
    #[func]
    fn param_flags(&self, def_idx: i32) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        let def = Self::def_of(def_idx);
        out.push(i64::from(def.supports_width()));
        out.push(i64::from(def.has_input_count()));
        out.push(i64::from(def.is_sequential()));
        out.push(i64::from(def.is_source()));
        out
    }

    /// [instances, components, nets, pins, wires, tick, last_eval, revision]
    #[func]
    fn stats(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        out.push(self.session.board().instances.len() as i64);
        out.push(self.session.engine().component_count() as i64);
        out.push(self.session.engine().net_count() as i64);
        out.push(self.session.engine().pin_count() as i64);
        out.push(self.session.board().wires.len() as i64);
        out.push(self.session.tick() as i64);
        out.push(self.session.engine().last_eval_count() as i64);
        out.push(self.session.revision() as i64);
        out
    }

    // -----------------------------------------------------------------------
    // 设计规则检查
    // -----------------------------------------------------------------------

    /// [kind, severity, net, inst] × N（net / inst 为 -1 表示不适用）
    #[func]
    fn drc_issues(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        for i in drc::check(self.session.board()) {
            out.push(i.kind.code());
            out.push(i.severity.code());
            out.push(net_code(i.net));
            out.push(if i.inst == u32::MAX { -1 } else { i.inst as i64 });
        }
        out
    }

    #[func]
    fn drc_details(&self) -> PackedStringArray {
        drc::check(self.session.board())
            .iter()
            .map(|i| GString::from(&format!("[{}] {}", i.kind.label(), i.detail)))
            .collect()
    }

    /// 关键路径：[ticks, inst, inst, …]（无环电路才有；有环返回空）
    #[func]
    fn critical_path(&self) -> PackedInt64Array {
        let mut out = PackedInt64Array::new();
        if let Some(cp) = drc::critical_path(self.session.board()) {
            out.push(cp.ticks as i64);
            for c in &cp.components {
                out.push(*c as i64);
            }
        }
        out
    }

    /// [错误数, 警告数]
    #[func]
    fn drc_summary(&self) -> PackedInt64Array {
        let issues = drc::check(self.session.board());
        let errors = issues
            .iter()
            .filter(|i| i.severity == drc::Severity::Error)
            .count() as i64;
        let warnings = issues.len() as i64 - errors;
        PackedInt64Array::from(&[errors, warnings][..])
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

    /// 某组件可改参数的个数（UI 判断是否显示参数区）
    #[func]
    fn param_count(&self, def_idx: i32) -> i64 {
        DefId::params(Self::def_of(def_idx)).len() as i64
    }

    // -----------------------------------------------------------------------
    // 存取（文件读写归壳层；这里只做字符串进出）
    // -----------------------------------------------------------------------

    #[func]
    fn save_data(&self) -> GString {
        GString::from(self.session.to_json_pretty().unwrap_or_default().as_str())
    }

    #[func]
    fn load_data(&mut self, data: GString) -> bool {
        self.session.load_json(&data.to_string()).is_ok()
    }

    #[func]
    fn clear(&mut self) {
        self.session.clear();
    }
}

#[gdextension(entry_symbol = logiclab_entry)]
unsafe impl ExtensionLibrary for LogicLab {}
