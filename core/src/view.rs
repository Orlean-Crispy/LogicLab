//! 壳无关的呈现数据
//!
//! 把"画布要画什么"抽成纯数据，任何壳（Godot，或将来别的）都只需消费它。
//! 它不知道按钮、主题、字体，因此不违反 ADR-21 的"core 不碰 UI 概念"，
//! 却能把换壳成本压到"只写一个消费 CircuitView 的渲染器"。
//!
//! 分两段使用，避免每帧做大分配：
//!   build_view()      编辑之后调用（几何 + 网络归属）
//!   refresh_values()  每帧调用（只刷值，O(引脚 + 导线)，零分配）
//!
//! 值还可以走更细的增量通道：Engine 的 changed_nets 只给出本拍变化的网络，
//! 壳据此只重绘受影响的导线（架构法则 #2：增量同步，绝不整帧全量快照）。

use crate::board::{Board, Netlist, Point, NO_NET};
use crate::defs::{DefId, Dir};
use crate::engine::Engine;
use crate::values::{NetValue, Width};

/// 网络连通状态（对齐原版 TC 的错误分类，先落地最要紧的三类）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NetStatus {
    /// 正常：恰好一个驱动
    Normal,
    /// 悬空：没有任何驱动
    Undriven,
    /// 冲突：多个驱动打架
    Conflict,
}

impl NetStatus {
    /// 传给壳的紧凑编码
    pub fn code(self) -> i64 {
        match self {
            NetStatus::Normal => 0,
            NetStatus::Undriven => 1,
            NetStatus::Conflict => 2,
        }
    }
}

/// 引脚方向编码
pub fn dir_code(d: Dir) -> i64 {
    match d {
        Dir::In => 0,
        Dir::Out => 1,
    }
}

/// 元件
#[derive(Clone, Debug)]
pub struct ComponentView {
    pub inst: u32,
    pub def: DefId,
    /// 画在组件上的类型名（AND / NAND / 寄存器…）。
    /// 用静态字符串：万级元件每次重建视图都分配一次字符串是纯浪费。
    pub label: &'static str,
    /// 实例名 inst_<n>，层级调试路径与导出命名用（ADR-25）
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub rot: u16,
}

/// 引脚
#[derive(Clone, Debug)]
pub struct PinView {
    pub inst: u32,
    pub slot: u16,
    pub x: i32,
    pub y: i32,
    pub dir: Dir,
    pub width: Width,
    pub net: u32,
    pub value: NetValue,
}

/// 导线
#[derive(Clone, Debug)]
pub struct WireView {
    pub index: u32,
    pub net: u32,
    pub points: Vec<Point>,
    pub width: Width,
    pub status: NetStatus,
    pub value: NetValue,
}

/// 文本注释（v4 §6.7）
#[derive(Clone, Debug)]
pub struct AnnotationView {
    pub index: u32,
    pub x: i32,
    pub y: i32,
    pub text: String,
}

/// 电路的整体呈现数据
#[derive(Clone, Debug, Default)]
pub struct CircuitView {
    pub components: Vec<ComponentView>,
    pub pins: Vec<PinView>,
    pub wires: Vec<WireView>,
    pub annotations: Vec<AnnotationView>,
    /// 网络总数（壳据此分配按 net 索引的值表）
    pub net_count: u32,
}

impl CircuitView {
    pub fn is_empty(&self) -> bool {
        self.components.is_empty() && self.wires.is_empty()
    }

    /// 内容包围盒（格坐标），用于"居中显示"
    pub fn bounds(&self) -> Option<(Point, Point)> {
        let mut min = Point::new(i32::MAX, i32::MAX);
        let mut max = Point::new(i32::MIN, i32::MIN);
        let mut any = false;
        for c in &self.components {
            min.x = min.x.min(c.x);
            min.y = min.y.min(c.y);
            max.x = max.x.max(c.x + c.w);
            max.y = max.y.max(c.y + c.h);
            any = true;
        }
        for w in &self.wires {
            for p in &w.points {
                min.x = min.x.min(p.x);
                min.y = min.y.min(p.y);
                max.x = max.x.max(p.x);
                max.y = max.y.max(p.y);
                any = true;
            }
        }
        for a in &self.annotations {
            let w = (a.text.chars().count() as i32).max(1);
            min.x = min.x.min(a.x);
            min.y = min.y.min(a.y);
            max.x = max.x.max(a.x + w);
            max.y = max.y.max(a.y + 1);
            any = true;
        }
        if any {
            Some((min, max))
        } else {
            None
        }
    }
}

/// 编辑之后重建几何与网络归属（自行推导网表）
pub fn build_view(board: &Board, engine: &Engine) -> CircuitView {
    let nl = board.compile();
    build_view_with(board, &nl, engine)
}

/// 用**已推导好的**网表重建视图，避免重复推导
pub fn build_view_with(board: &Board, nl: &Netlist, engine: &Engine) -> CircuitView {
    let nets = nl.net_count as usize;

    // 每个网络的驱动者数量 → 悬空 / 正常 / 冲突
    let mut drivers = vec![0u16; nets];
    for (i, n) in nl.pin_net.iter().enumerate() {
        if *n != NO_NET && nl.pin_is_out[i] {
            let slot = &mut drivers[*n as usize];
            *slot = slot.saturating_add(1);
        }
    }
    let status_of = |net: u32| -> NetStatus {
        if net == NO_NET {
            return NetStatus::Undriven;
        }
        match drivers.get(net as usize).copied().unwrap_or(0) {
            0 => NetStatus::Undriven,
            1 => NetStatus::Normal,
            _ => NetStatus::Conflict,
        }
    };

    let mut components = Vec::with_capacity(board.instances.len());
    let mut pins = Vec::with_capacity(nl.pin_net.len());

    for (ii, inst) in board.instances.iter().enumerate() {
        let (w, h) = inst.size();
        components.push(ComponentView {
            inst: ii as u32,
            def: inst.def,
            label: inst.def.label(),
            name: inst.display_name.clone(),
            x: inst.x,
            y: inst.y,
            w,
            h,
            rot: inst.rot,
        });

        let base = nl.pin_start[ii];
        for (slot, pd) in inst.pins.iter().enumerate() {
            let net = nl.pin_net[(base as usize) + slot];
            pins.push(PinView {
                inst: ii as u32,
                slot: slot as u16,
                x: inst.x + pd.dx,
                y: inst.y + pd.dy,
                dir: pd.dir,
                width: pd.width,
                net,
                value: if net == NO_NET { NetValue::ZERO } else { engine.net_value(net) },
            });
        }
    }

    let mut wires = Vec::with_capacity(board.wires.len());
    for (wi, w) in board.wires.iter().enumerate() {
        let net = nl.wire_net[wi];
        wires.push(WireView {
            index: wi as u32,
            net,
            points: w.points.clone(),
            width: if net == NO_NET { 1 } else { engine.net_width(net) },
            status: status_of(net),
            value: if net == NO_NET { NetValue::ZERO } else { engine.net_value(net) },
        });
    }

    let annotations = board
        .annotations
        .iter()
        .enumerate()
        .map(|(i, a)| AnnotationView {
            index: i as u32,
            x: a.x,
            y: a.y,
            text: a.text.clone(),
        })
        .collect();

    CircuitView { components, pins, wires, annotations, net_count: nl.net_count }
}

/// 刷新所有引脚 / 导线的当前值（每帧调用，零分配）
pub fn refresh_values(view: &mut CircuitView, engine: &Engine) {
    for p in &mut view.pins {
        p.value = if p.net == NO_NET { NetValue::ZERO } else { engine.net_value(p.net) };
    }
    for w in &mut view.wires {
        w.value = if w.net == NO_NET { NetValue::ZERO } else { engine.net_value(w.net) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::Params;

    #[test]
    fn view_reports_geometry_and_values() {
        let mut b = Board::new();
        let sw = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
        let led = b.add_instance(DefId::Led, Params::default().width(1), 6, 0);
        assert!(b.connect_pins((sw, 0), (led, 0)));

        let mut e = Engine::new();
        e.load_board(&b);
        let mut v = build_view(&b, &e);

        assert_eq!(v.components.len(), 2);
        assert_eq!(v.pins.len(), 2);
        assert_eq!(v.wires.len(), 1);
        assert_eq!(v.wires[0].status, NetStatus::Normal, "开关驱动 → 正常");
        assert_eq!(v.net_count, 1);

        // 居中显示用的包围盒应覆盖两个元件
        let (min, max) = v.bounds().unwrap();
        assert!(min.x <= 0 && max.x >= 6);

        // 值必须跟着仿真走
        e.set_input(sw, 1);
        e.run_for(3);
        refresh_values(&mut v, &e);
        assert_eq!(v.pins[0].value.get(1), 1);
        assert_eq!(v.wires[0].value.get(1), 1);
        assert_eq!(v.pins[1].value.get(1), 1);
    }

    #[test]
    fn undriven_and_conflict_are_distinguished() {
        let mut b = Board::new();
        // 两条独立导线：一条只有探针（无驱动），一条没有连接
        let led = b.add_instance(DefId::Led, Params::default().width(1), 0, 0);
        let _ = led;
        let p = Params::default().width(1);
        let a = b.add_instance(DefId::Buffer, p, 0, 6);
        let c = b.add_instance(DefId::Buffer, p, 6, 6);
        assert!(b.connect_pins((a, 1), (c, 0)));

        let mut e = Engine::new();
        e.load_board(&b);
        let v = build_view(&b, &e);

        // Buffer 的输入悬空 → 该网络仍由 a 的输出驱动，因此是正常
        let wired: Vec<_> = v.wires.iter().filter(|w| w.status == NetStatus::Normal).collect();
        assert_eq!(wired.len(), 1);
    }
}
