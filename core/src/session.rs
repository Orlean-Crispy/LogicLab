//! 编辑与仿真的统一门面（v3 §3.3 的 driver 层 + v4 ADR-23）
//!
//! 存在的理由有两个：
//!   1. **ADR-23 编辑即重置**：任何编辑命令生效后，仿真状态整体回到上电态。
//!      把这条规则收在门面里，壳就不可能漏掉（漏掉的表现是 undo 后寄存器值
//!      变成 undefined，属于必然踩的坑）。
//!   2. 将来加 undo/redo 时，命令栈天然挂在这里，壳一行不用改。
//!
//! 门面只做编排，所有真实逻辑仍在 board / engine / view 里。

use crate::board::{Board, NO_NET};
use crate::defs::{DefId, Params};
use crate::engine::Engine;
use crate::examples;
use crate::save::{Project, SaveError};
use crate::view::{build_view, CircuitView};

/// 一个编辑会话：工程 + 引擎 + 视图
pub struct Session {
    project: Project,
    engine: Engine,
    view: CircuitView,
    /// 实例索引 → 该实例第一个全局引脚的编号（读引脚值用，O(1)）
    pin_start: Vec<u32>,
    /// 每次编辑 +1；壳据此判断"几何变了，需要重建顶点缓冲"
    revision: u64,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        let mut project = Project::new("未命名工程");
        project.add_board(Board::new());
        Self::from_project(project)
    }

    pub fn from_project(mut project: Project) -> Self {
        if project.boards.is_empty() {
            project.add_board(Board::new());
        }
        let main = project.main_board;
        let board = &project.boards[main];
        let mut engine = Engine::new();
        engine.load_board(board);
        let view = build_view(board, &engine);
        let pin_start = board.compile().pin_start;
        Self { project, engine, view, pin_start, revision: 1 }
    }

    // -----------------------------------------------------------------------
    // 读取
    // -----------------------------------------------------------------------

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn board(&self) -> &Board {
        &self.project.boards[self.project.main_board]
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

    /// 层级调试路径（ADR-25）：board/inst_3/…
    pub fn hierarchy_path(&self, inst: u32, net_name: &str) -> String {
        let name = self.board().instance_name(inst).unwrap_or("?");
        format!("{name}/{net_name}")
    }

    // -----------------------------------------------------------------------
    // 编辑（每条都以"重置仿真"收尾，ADR-23）
    // -----------------------------------------------------------------------

    pub fn add_component(&mut self, def: DefId, x: i32, y: i32) -> u32 {
        let id = self.board_mut().add_instance(def, def.default_params(), x, y);
        self.after_edit();
        id
    }

    pub fn remove_component(&mut self, id: u32) -> bool {
        let ok = self.board_mut().remove_instance(id);
        if ok {
            self.after_edit();
        }
        ok
    }

    pub fn move_component(&mut self, id: u32, x: i32, y: i32) {
        self.board_mut().set_pos(id, x, y);
        self.after_edit();
    }

    pub fn rotate_component(&mut self, id: u32) {
        self.board_mut().rotate(id);
        self.after_edit();
    }

    pub fn set_params(&mut self, id: u32, params: Params) {
        self.board_mut().set_params(id, params);
        self.after_edit();
    }

    /// 按名改一个参数（键名见 DefId::params）
    pub fn set_param(&mut self, id: u32, key: &str, value: i64) -> bool {
        let Some(mut p) = self.board().instance(id).map(|i| i.params) else {
            return false;
        };
        if !p.set_named(key, value) {
            return false;
        }
        self.set_params(id, p);
        true
    }

    pub fn set_display_name(&mut self, id: u32, name: &str) {
        if let Some(inst) = self.board_mut().instance_mut(id) {
            inst.display_name = name.to_string();
        }
        self.after_edit();
    }

    pub fn connect_pins(&mut self, a: (u32, usize), b: (u32, usize)) -> bool {
        let ok = self.board_mut().connect_pins(a, b);
        self.after_edit();
        ok
    }

    pub fn remove_wire(&mut self, index: u32) -> bool {
        let ok = self.board_mut().remove_wire(index);
        if ok {
            self.after_edit();
        }
        ok
    }

    pub fn add_annotation(&mut self, x: i32, y: i32, text: &str) -> u32 {
        let id = self.board_mut().add_annotation(x, y, text);
        self.after_edit();
        id
    }

    pub fn remove_annotation(&mut self, id: u32) -> bool {
        let ok = self.board_mut().remove_annotation(id);
        if ok {
            self.after_edit();
        }
        ok
    }

    pub fn set_annotation_text(&mut self, id: u32, text: &str) {
        if let Some(a) = self.board_mut().annotations.get_mut(id as usize) {
            a.text = text.to_string();
        }
        self.after_edit();
    }

    pub fn move_annotation(&mut self, id: u32, x: i32, y: i32) {
        if let Some(a) = self.board_mut().annotations.get_mut(id as usize) {
            a.x = x;
            a.y = y;
        }
        self.after_edit();
    }

    /// 用内置示例替换当前图纸
    pub fn load_example(&mut self, id: &str) -> bool {
        let Some(board) = examples::build(id) else {
            return false;
        };
        let main = self.project.main_board;
        self.project.boards[main] = board;
        self.after_edit();
        true
    }

    // -----------------------------------------------------------------------
    // 仿真
    // -----------------------------------------------------------------------

    pub fn sim_tick(&mut self) {
        self.engine.tick();
    }

    pub fn sim_run_for(&mut self, n: u64) {
        self.engine.run_for(n);
    }

    /// 开关 / 按钮 / 常量：写值，下一 tick 生效（不算编辑，不重置）
    pub fn set_input(&mut self, id: u32, value: u32) {
        if let Some(p) = self.board().instance(id).map(|i| i.params) {
            let np = Params { value, ..p };
            self.board_mut().set_params(id, np);
            self.engine.set_input(id, value);
            self.refresh_values();
        }
    }

    pub fn toggle_input(&mut self, id: u32) {
        let Some(p) = self.board().instance(id).map(|i| i.params) else {
            return;
        };
        let next = if p.value != 0 { 0 } else { 1 };
        self.set_input(id, next);
    }

    /// 手动复位（与"编辑即重置"效果一致，但由用户主动触发）
    pub fn reset_sim(&mut self) {
        let main = self.project.main_board;
        let board = &self.project.boards[main];
        self.engine = Engine::new();
        self.engine.load_board(board);
        self.pin_start = board.compile().pin_start;
        self.refresh_values();
    }

    /// 读某个实例的某个引脚当前值。
    ///
    /// 注意这里刻意读的是**引脚**而不是"实例输出"——探针（LED）这类纯观察组件
    /// 不参与求值，没有输出可读，但它的输入引脚一样有值。
    pub fn pin_value(&self, inst: u32, slot: usize) -> crate::values::NetValue {
        let Some(&base) = self.pin_start.get(inst as usize) else {
            return crate::values::NetValue::ZERO;
        };
        self.engine.pin_value(base + slot as u32)
    }

    pub fn net_value(&self, net: u32) -> crate::values::NetValue {
        if net == NO_NET {
            crate::values::NetValue::ZERO
        } else {
            self.engine.net_value(net)
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

    /// 载入工程（等价于一次编辑：仿真回到上电态）
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

    fn board_mut(&mut self) -> &mut Board {
        let main = self.project.main_board;
        &mut self.project.boards[main]
    }

    /// 编辑之后的统一收尾：**整个仿真回到上电态**（ADR-23）。
    ///
    /// 重建引擎而不是"修补"，是因为编辑可能改变网表结构、引脚布局、位宽——
    /// 逐个修补既复杂又必然遗漏。万级电路的装载约 3 ms，编辑是低频操作，值得。
    fn after_edit(&mut self) {
        let main = self.project.main_board;
        let board = &self.project.boards[main];
        self.engine = Engine::new();
        self.engine.load_board(board);
        self.view = build_view(board, &self.engine);
        self.pin_start = board.compile().pin_start;
        self.revision += 1;
    }

    /// 只刷新视图里的值（不重建几何）
    fn refresh_values(&mut self) {
        crate::view::refresh_values(&mut self.view, &self.engine);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::DefId;

    #[test]
    fn edit_resets_simulation_to_power_on() {
        let mut s = Session::new();
        let sw = s.add_component(DefId::Switch, 0, 0);
        let led = s.add_component(DefId::Led, 8, 0);
        assert!(s.connect_pins((sw, 0), (led, 0)));

        s.set_input(sw, 1);
        s.sim_run_for(5);
        assert_eq!(s.pin_value(led, 0).get(1), 1);
        assert_eq!(s.tick(), 5);

        // 编辑之后：tick 归零、输出回到上电态（ADR-23）
        s.add_component(DefId::Not, 20, 0);
        assert_eq!(s.tick(), 0, "编辑必须把 tick 归零");
        assert_eq!(s.pin_value(led, 0).get(1), 0, "编辑后回到上电态");
    }

    #[test]
    fn input_toggle_does_not_reset_simulation() {
        let mut s = Session::new();
        let sw = s.add_component(DefId::Switch, 0, 0);
        let led = s.add_component(DefId::Led, 8, 0);
        assert!(s.connect_pins((sw, 0), (led, 0)));
        s.sim_run_for(3);
        let before = s.tick();
        s.toggle_input(sw);
        s.sim_run_for(2);
        assert_eq!(s.tick(), before + 2, "拨开关不算编辑，不该重置");
    }

    #[test]
    fn instance_names_are_auto_assigned() {
        let mut s = Session::new();
        let a = s.add_component(DefId::Not, 0, 0);
        let b = s.add_component(DefId::Or, 4, 0);
        assert_eq!(s.board().instance_name(a), Some("inst_0"));
        assert_eq!(s.board().instance_name(b), Some("inst_1"));
        s.set_display_name(a, "我的反相器");
        assert_eq!(s.board().instance_name(a), Some("我的反相器"));
        assert_eq!(s.hierarchy_path(a, "Q"), "我的反相器/Q");
    }

    #[test]
    fn annotations_survive_save_round_trip() {
        let mut s = Session::new();
        s.add_component(DefId::And, 0, 0);
        s.add_annotation(3, 5, "这里是关键路径");
        let json = s.to_json_pretty().unwrap();
        assert!(json.contains("关键路径"));

        let mut s2 = Session::new();
        s2.load_json(&json).unwrap();
        assert_eq!(s2.board().annotations.len(), 1);
        assert_eq!(s2.board().annotations[0].text, "这里是关键路径");
        assert_eq!(s2.tick(), 0);
    }
}
