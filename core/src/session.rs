//! 编辑与仿真的统一门面（v3 §3.3 的 driver 层 + v4 ADR-9 / ADR-23）
//!
//! 存在的理由：
//!   1. **ADR-23 编辑即重置**：任何编辑生效后仿真整体回到上电态。收在门面里，
//!      壳就不可能漏（漏掉的表现是 undo 后寄存器值未定义，属于必然踩的坑）。
//!   2. **ADR-9 撤销/重做**：命令历史天然属于这里，壳只调 undo/redo。
//!   3. 缓存 pin_start，提供 O(1) 的引脚读数。
//!
//! 所有编辑都走 `edit()` 这一个入口，它负责"存快照 → 改板 → 重建仿真"。
//! 新增编辑操作只要写一行，不会再漏掉任何收尾工作。

use std::collections::VecDeque;

use crate::board::{Board, NO_NET};
use crate::defs::{DefId, Params};
use crate::engine::Engine;
use crate::examples;
use crate::save::{Project, SaveError};
use crate::view::{build_view, CircuitView};

/// 编辑会话：工程 + 引擎 + 视图 + 命令历史
pub struct Session {
    project: Project,
    engine: Engine,
    view: CircuitView,
    /// 实例索引 → 该实例第一个全局引脚编号（O(1) 读引脚值）
    pin_start: Vec<u32>,
    /// 每次编辑 +1；壳据此判断几何是否变了
    revision: u64,
    /// 撤销栈（快照式，见 history_limit 的说明）
    undo: VecDeque<Board>,
    redo: Vec<Board>,
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
        let mut s = Self {
            project,
            engine: Engine::new(),
            view: CircuitView::default(),
            pin_start: Vec::new(),
            revision: 1,
            undo: VecDeque::new(),
            redo: Vec::new(),
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

    /// 层级调试路径（ADR-25）：实例名/引脚名
    pub fn hierarchy_path(&self, inst: u32, pin: &str) -> String {
        format!("{}/{pin}", self.board().instance_name(inst).unwrap_or("?"))
    }

    /// 读某个实例的某个引脚当前值。
    ///
    /// 刻意读**引脚**而不是"实例输出"——探针这类纯观察组件不参与求值，
    /// 没有输出可读，但它的输入引脚一样有值。
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

    /// 用内置示例替换当前图纸
    pub fn load_example(&mut self, id: &str) -> bool {
        let Some(board) = examples::build(id) else {
            return false;
        };
        self.edit(|b| *b = board);
        true
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
        self.redo.push(self.board().clone());
        self.set_board(prev);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push_back(self.board().clone());
        self.set_board(next);
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

    /// 拨开关：**不算编辑**，不重置仿真（它本就是运行期交互）
    pub fn set_input(&mut self, id: u32, value: u32) {
        let Some(p) = self.board().instance(id).map(|i| i.params) else {
            return;
        };
        self.board_mut().set_params(id, Params { value, ..p });
        self.engine.set_input(id, value);
        self.view = build_view(self.board(), &self.engine);
    }

    pub fn toggle_input(&mut self, id: u32) {
        let Some(p) = self.board().instance(id).map(|i| i.params) else {
            return;
        };
        let next = if p.value != 0 { 0 } else { 1 };
        self.set_input(id, next);
    }

    /// 手动复位（效果与"编辑即重置"一致，但由用户主动触发）
    pub fn reset_sim(&mut self) {
        self.rebuild();
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
        let (undo, redo) = (std::mem::take(&mut self.undo), std::mem::take(&mut self.redo));
        *self = Self::from_project(p);
        self.undo = undo;
        self.undo.clear();
        self.redo = redo;
        self.redo.clear();
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

    /// **唯一的编辑入口**：存快照 → 改板 → 重建仿真（ADR-23 + ADR-9）
    fn edit<R>(&mut self, f: impl FnOnce(&mut Board) -> R) -> R {
        let before = self.board().clone();
        let result = f(self.board_mut());
        self.push_history(before);
        self.after_edit();
        result
    }

    /// 撤销栈用**整板快照**而不是可反演命令对象。
    ///
    /// ADR-9 要求"命令可反演"，但删除元件要连带恢复导线、索引还会位移，
    /// 逐条写逆操作既啰嗦又易错。快照只要一次 clone（万级电路约 1 ms），
    /// 而编辑是人手触发的低频操作——复杂度换来的收益更值。
    /// 深度按电路规模自适应，避免大电路把内存吃满。
    fn push_history(&mut self, before: Board) {
        let n = before.instances.len() + before.wires.len();
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
        // 新的编辑让"重做"失效
        self.redo.clear();
    }

    fn set_board(&mut self, b: Board) {
        let main = self.project.main_board;
        self.project.boards[main] = b;
        self.after_edit();
    }

    /// 编辑之后的统一收尾：**整个仿真回到上电态**（ADR-23）。
    ///
    /// 重建引擎而不是"修补"，因为编辑可能改变网表结构、引脚布局、位宽——
    /// 逐个修补既复杂又必然遗漏。万级电路的装载约 3 ms，编辑是低频操作，值得。
    fn after_edit(&mut self) {
        self.rebuild();
        self.revision += 1;
    }

    /// 由当前图纸重建引擎与视图（不递增版本号）
    fn rebuild(&mut self) {
        let main = self.project.main_board;
        let board = &self.project.boards[main];
        self.engine = Engine::new();
        self.engine.load_board(board);
        self.view = build_view(board, &self.engine);
        self.pin_start = board.compile().pin_start;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        // 逐步撤销回到初始态：序列化必须完全相等（§14 不变量）
        assert!(s.undo());
        assert!(s.undo());
        assert!(s.undo());
        assert!(s.undo());
        assert!(!s.can_undo());
        assert_eq!(s.to_json().unwrap(), initial, "全量 undo 应回到初始态");

        // 再全部重做
        assert!(s.redo());
        assert!(s.redo());
        assert!(s.redo());
        assert!(s.redo());
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

    /// 空操作不该占用撤销栈，否则历史里会塞满"改了但值没变"的记录
    #[test]
    fn unchanged_param_is_not_an_edit() {
        let mut s = Session::new();
        let id = s.add_component(DefId::And, 0, 0);
        let snap = s.to_json().unwrap();
        let depth = undo_depth(&mut s);
        assert_eq!(s.to_json().unwrap(), snap, "数历史深度不该改变电路");

        assert!(s.set_param(id, "width", 1), "写入与当前相同的值应返回成功");
        assert_eq!(undo_depth(&mut s), depth, "空操作不该增加历史深度");
        assert!(!s.set_param(id, "不存在的键", 1), "未知键应被拒绝");
    }

    /// 数一共能撤销几步，然后原样恢复到当前状态
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
        let b = s.add_component(DefId::Or, 4, 0);
        assert_eq!(s.board().instance_name(a), Some("inst_0"));
        assert_eq!(s.board().instance_name(b), Some("inst_1"));
        s.set_display_name(a, "我的反相器");
        assert_eq!(s.hierarchy_path(a, "Q"), "我的反相器/Q");
    }

    #[test]
    fn net_labels_join_by_name() {
        // 两段互不接触的导线，各贴一个同名标签 → 应当连通
        let mut s = Session::new();
        let a = s.add_component(DefId::Switch, 0, 0);
        let b = s.add_component(DefId::Led, 40, 0);
        s.connect_pins((a, 0), (b, 0));
        let wires = s.board().wires.len();
        assert_eq!(wires, 1);

        // 再来一组：两条独立导线用标签相连
        let mut s2 = Session::new();
        let sw = s2.add_component(DefId::Switch, 0, 0);
        // 探针必须落在第二条导线的端点上，否则它压根没接进网络
        let led = s2.add_component(DefId::Led, 28, 0);
        // 手工画两条断开的导线
        s2.edit(|b| {
            b.add_wire(vec![crate::board::Point::new(2, 0), crate::board::Point::new(10, 0)]);
            b.add_wire(vec![crate::board::Point::new(20, 0), crate::board::Point::new(28, 0)]);
        });
        s2.add_label(6, 0, "BUS");
        s2.add_label(24, 0, "BUS");
        let nl = s2.board().compile();
        let net_sw = nl.pin_net[nl.pin_start[sw as usize] as usize];
        let net_led = nl.pin_net[nl.pin_start[led as usize] as usize];
        assert_ne!(net_sw, NO_NET);
        assert_eq!(net_sw, net_led, "同名标签应把两段导线并成一个网络");
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
}
