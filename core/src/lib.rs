//! LogicLab core —— 数字电路沙盒的纯 Rust 内核（v3 规格 §3.1）
//!
//! **铁律：本 crate 及其依赖树中不得出现任何 godot 类型**（ADR-21）。
//! core 不碰文件系统、不碰时钟、不碰 UI 概念：JSON 以 `&str` 进出，tick 由外部驱动。
//! 这是"将来换壳（egui / Tauri / Bevy）时 core 一行不改"的唯一保证。
//!
//! 模块划分：
//!   values  值模型（位宽、四态位、wrap 算术）
//!   defs    组件定义与求值行为（组件知识的唯一数据源）
//!   board   编辑期数据模型（元件 / 导线）与网表推导
//!   engine  离散 tick 仿真引擎（双缓冲 + 脏传播）
//!   save    工程持久化（JSON 以 &str 进出，不碰文件系统）
//!   view    壳无关的呈现数据（换壳时只重写渲染器）
//!   drc     设计规则检查（多驱动 / 位宽 / 悬空 / 组合环）

pub mod board;
pub mod defs;
pub mod drc;
pub mod engine;
pub mod examples;
pub mod save;
pub mod session;
pub mod values;
pub mod view;

pub use board::{Board, Instance, Netlist, Point, Wire, NO_NET};
pub use defs::{Category, CompState, DefId, Dir, Params, PinDef};
pub use engine::{Engine, NO_COMP};
pub use drc::{check as drc_check, Issue, IssueKind, Severity};
pub use examples::{Example, ALL as EXAMPLES};
pub use save::{Project, SaveError, SCHEMA_VERSION};
pub use session::Session;
pub use values::{format_value, Bit, NetValue, Width};
pub use view::{build_view, refresh_values, CircuitView, NetStatus};
