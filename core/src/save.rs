//! 工程持久化（v3 §4 / ADR-11 / ADR-21）
//!
//! **core 不碰文件系统**：JSON 以 &str 进出，落盘是 shell / cli 的职责。
//! 这是「将来换壳时 core 一行不改」的前提之一。
//!
//! 格式约定：
//!   - 单 JSON 文件、自包含，可 diff、可进版本管理（ADR-11）
//!   - schema_version 显式记录；读到**更高**版本直接拒绝，而不是猜着读
//!   - 组件类型以稳定标识串存盘（见 DefId 的 Serialize 实现），
//!     所以重命名 Rust 变体不会破坏已保存的工程

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::board::Board;

/// 当前工程格式版本
pub const SCHEMA_VERSION: u32 = 1;

/// 工程（单 JSON 文件，自包含）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    /// 缺省视为最老版本，由 from_json 统一升格到 SCHEMA_VERSION
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub name: String,
    /// 工程可含多张图纸（§4）
    #[serde(default)]
    pub boards: Vec<Board>,
    /// 主图纸索引
    #[serde(default)]
    pub main_board: usize,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            name: "未命名工程".to_string(),
            boards: Vec::new(),
            main_board: 0,
        }
    }
}

impl Project {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string(), ..Default::default() }
    }

    /// 追加一张图纸，返回其索引
    pub fn add_board(&mut self, board: Board) -> usize {
        self.boards.push(board);
        self.boards.len() - 1
    }

    pub fn main(&self) -> Option<&Board> {
        self.boards.get(self.main_board)
    }

    pub fn main_mut(&mut self) -> Option<&mut Board> {
        self.boards.get_mut(self.main_board)
    }

    /// 紧凑 JSON（传输用）
    pub fn to_json(&self) -> Result<String, SaveError> {
        serde_json::to_string(self).map_err(|e| SaveError::Encode(e.to_string()))
    }

    /// 缩进 JSON —— **存盘用这个**：可 diff、可进版本管理（ADR-11）
    pub fn to_json_pretty(&self) -> Result<String, SaveError> {
        serde_json::to_string_pretty(self).map_err(|e| SaveError::Encode(e.to_string()))
    }

    /// 从 JSON 装载。引脚布局缓存会在此重建（它不入档）
    pub fn from_json(s: &str) -> Result<Project, SaveError> {
        let value: Value =
            serde_json::from_str(s).map_err(|e| SaveError::Decode(e.to_string()))?;
        // 顶层必须是对象：否则 serde 的紧凑数组表示会把任意 JSON 当成空工程收下
        if !value.is_object() {
            return Err(SaveError::Decode("工程文件顶层必须是 JSON 对象".to_string()));
        }
        let found = value.get("schema_version").and_then(Value::as_u64).unwrap_or(0) as u32;
        if found > SCHEMA_VERSION {
            return Err(SaveError::TooNew { found, supported: SCHEMA_VERSION });
        }
        // found < SCHEMA_VERSION 时在此逐级升格；当前尚无历史版本，故没有分支。
        // 新增版本时在这里追加 migrate_v1_to_v2 之类的转换即可。
        let mut project: Project =
            serde_json::from_value(value).map_err(|e| SaveError::Decode(e.to_string()))?;
        project.schema_version = SCHEMA_VERSION;
        for b in &mut project.boards {
            b.fixup();
        }
        if project.main_board >= project.boards.len() {
            project.main_board = 0;
        }
        Ok(project)
    }
}

/// 存档错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// 编码失败
    Encode(String),
    /// 解析失败
    Decode(String),
    /// 工程版本高于本程序支持的版本
    TooNew { found: u32, supported: u32 },
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Encode(e) => write!(f, "工程编码失败: {e}"),
            SaveError::Decode(e) => write!(f, "工程解析失败: {e}"),
            SaveError::TooNew { found, supported } => write!(
                f,
                "工程版本 {found} 高于本程序支持的 {supported}，请升级 LogicLab"
            ),
        }
    }
}

impl std::error::Error for SaveError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::{DefId, Params};

    fn sample() -> Project {
        let mut b = Board::new();
        let sw = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
        let led = b.add_instance(DefId::Led, Params::default().width(1), 10, 0);
        assert!(b.connect_pins((sw, 0), (led, 0)));
        b.add_instance(DefId::Adder, Params::default().width(8), 0, 20);
        let mut p = Project::new("测试工程");
        p.add_board(b);
        p
    }

    #[test]
    fn json_round_trip_is_byte_stable() {
        let p = sample();
        let j1 = p.to_json_pretty().unwrap();
        let p2 = Project::from_json(&j1).unwrap();
        let j2 = p2.to_json_pretty().unwrap();
        assert_eq!(j1, j2, "load→save→load 必须逐字节一致（§14 不变量）");
    }

    #[test]
    fn round_trip_preserves_electrical_topology() {
        let p = sample();
        let j = p.to_json().unwrap();
        let p2 = Project::from_json(&j).unwrap();
        let n1 = p.boards[0].compile();
        let n2 = p2.boards[0].compile();
        assert_eq!(n1.pin_net, n2.pin_net, "读档后网表必须完全一致");
        assert_eq!(n1.net_count, n2.net_count);
        assert_eq!(
            p2.boards[0].instances[0].pins.len(),
            1,
            "引脚布局缓存必须被重建"
        );
        assert_eq!(p2.boards[0].pin_total(), p.boards[0].pin_total());
    }

    #[test]
    fn component_ids_are_the_persisted_contract() {
        let p = sample();
        let j = p.to_json().unwrap();
        assert!(j.contains("\"switch\""), "组件类型必须以稳定 id 串存盘");
        assert!(j.contains("\"led\""));
        assert!(j.contains("\"adder\""));
    }

    #[test]
    fn newer_schema_is_rejected_not_guessed() {
        let mut p = sample();
        p.schema_version = SCHEMA_VERSION + 5;
        let j = p.to_json().unwrap();
        assert!(matches!(Project::from_json(&j), Err(SaveError::TooNew { .. })));
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(Project::from_json("{ not json").is_err());
        assert!(Project::from_json("[]").is_err());
        assert!(Project::from_json(r#"{"schema_version":1,"boards":[]}"#).is_ok());
    }
}
