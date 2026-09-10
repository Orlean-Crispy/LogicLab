//! 组件定义与行为（v3 §4 / §5 / §6）
//!
//! 本模块是组件知识的**唯一数据源**：引脚布局、参数、求值行为都定义在这里，
//! 壳只负责渲染与交互，不再自行维护一份组件表（避免两处定义漂移）。
//!
//! 分层：
//!   DefId      组件种类（编译期枚举，热路径可直接 match 派发）
//!   Params     实例的可编辑参数（位宽 / 输入数 / 常量值 / 时钟周期 …），Copy 且无分配
//!   PinDef      引脚定义（相对格坐标 + 位宽 + 方向），仅在编辑期生成
//!   CompState  实例私有状态（锁存值 / 上一拍时钟 / 计数器 / 存储器）
//!   eval()     纯求值：读输入 + 状态 → 写输出。**绝不直接改 NET**（双缓冲铁律）

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::values::{
    add_wrap, bits_needed, mul_wrap, normalize_width, sar, shl, shr, sub_wrap, to_signed,
    width_mask, Bit, NetValue, Width,
};

/// 引脚方向
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    In,
    Out,
}

/// 引脚定义（相对组件的格坐标）
#[derive(Clone, Debug)]
pub struct PinDef {
    pub name: String,
    pub dx: i32,
    pub dy: i32,
    pub width: Width,
    pub dir: Dir,
}

/// 构造引脚（模块内便捷函数，避免每处重复写字段名）
fn pin(name: &str, dx: i32, dy: i32, width: Width, dir: Dir) -> PinDef {
    PinDef { name: name.to_string(), dx, dy, width, dir }
}

/// 元件库分组（UI 直接消费）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Io,
    Logic,
    Routing,
    Arithmetic,
    Memory,
}

impl Category {
    pub fn id(self) -> &'static str {
        match self {
            Category::Io => "io",
            Category::Logic => "logic",
            Category::Routing => "routing",
            Category::Arithmetic => "arith",
            Category::Memory => "memory",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::Io => "输入输出",
            Category::Logic => "逻辑门",
            Category::Routing => "多位与选择",
            Category::Arithmetic => "算术",
            Category::Memory => "存储",
        }
    }

    /// 紧凑编码（壳用它查颜色表，避免比较字符串）
    pub fn code(self) -> i64 {
        match self {
            Category::Io => 0,
            Category::Logic => 1,
            Category::Routing => 2,
            Category::Arithmetic => 3,
            Category::Memory => 4,
        }
    }
}

// ---------------------------------------------------------------------------
// 参数
// ---------------------------------------------------------------------------

/// 可选引脚 / 行为开关
pub const OPT_SIGNED: u32 = 1 << 0;
/// 时序元件暴露 enable 引脚
pub const OPT_ENABLE: u32 = 1 << 1;
/// 时序元件暴露 reset 引脚
pub const OPT_RESET: u32 = 1 << 2;
/// 加减法器暴露进 / 借位
pub const OPT_CARRY: u32 = 1 << 3;
/// 移位器方向：置位 = 左移
pub const OPT_SHIFT_LEFT: u32 = 1 << 4;

/// Splitter / Merger 的低段位宽（存在 Params::value 里），夹到 1..width-1
#[inline]
pub fn low_bits(p: &Params) -> Width {
    let w = p.width.max(2);
    (p.value.max(1).min(w as u32 - 1)) as Width
}

/// 实例参数（Copy，热路径友好）
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Params {
    /// 数据位宽
    pub width: Width,
    /// 门的输入数 / MUX 通道数 / 编码器输入数
    pub inputs: u8,
    /// 常量值 / 开关状态
    pub value: u32,
    /// 见 OPT_* 标志
    pub opts: u32,
    /// Clock 高电平持续 tick 数
    pub high: u16,
    /// Clock 低电平持续 tick 数
    pub low: u16,
    /// RAM / ROM 深度
    pub depth: u16,
}

impl Default for Params {
    fn default() -> Self {
        Self { width: 1, inputs: 2, value: 0, opts: 0, high: 1, low: 1, depth: 256 }
    }
}

impl Params {
    pub fn width(mut self, w: u32) -> Self {
        self.width = normalize_width(w);
        self
    }

    pub fn inputs(mut self, n: u8) -> Self {
        self.inputs = n;
        self
    }

    pub fn opts(mut self, o: u32) -> Self {
        self.opts = o;
        self
    }

    pub fn depth(mut self, d: u16) -> Self {
        self.depth = d;
        self
    }

    /// 按名读取参数（键名见 DefId::params 里的 ParamDesc）
    pub fn get_named(&self, key: &str) -> i64 {
        match key {
            "width" => self.width as i64,
            "inputs" => self.inputs as i64,
            "value" => self.value as i64,
            "low_bits" => low_bits(self) as i64,
            "opts" => self.opts as i64,
            "high" => self.high as i64,
            "low" => self.low as i64,
            "depth" => self.depth as i64,
            "signed" => i64::from(self.opts & OPT_SIGNED != 0),
            "enable" => i64::from(self.opts & OPT_ENABLE != 0),
            "reset" => i64::from(self.opts & OPT_RESET != 0),
            "carry" => i64::from(self.opts & OPT_CARRY != 0),
            "mode" => shift_mode(self) as i64,
            _ => 0,
        }
    }

    /// 按名写入参数；返回是否被接受
    pub fn set_named(&mut self, key: &str, value: i64) -> bool {
        fn flag(p: &mut Params, mask: u32, on: bool) {
            if on {
                p.opts |= mask;
            } else {
                p.opts &= !mask;
            }
        }
        match key {
            "width" => {
                self.width = normalize_width(value.max(0) as u32);
                true
            }
            "inputs" => {
                self.inputs = value.clamp(2, 8) as u8;
                true
            }
            "value" => {
                self.value = value.clamp(0, u32::MAX as i64) as u32;
                true
            }
            "opts" => {
                self.opts = value as u32;
                true
            }
            "low_bits" => {
                let w = self.width.max(2) as i64;
                self.value = value.clamp(1, w - 1) as u32;
                true
            }
            "high" => {
                self.high = value.clamp(1, u16::MAX as i64) as u16;
                true
            }
            "low" => {
                self.low = value.clamp(1, u16::MAX as i64) as u16;
                true
            }
            "depth" => {
                self.depth = value.clamp(2, 65535) as u16;
                true
            }
            "signed" => {
                flag(self, OPT_SIGNED, value != 0);
                true
            }
            "enable" => {
                flag(self, OPT_ENABLE, value != 0);
                true
            }
            "reset" => {
                flag(self, OPT_RESET, value != 0);
                true
            }
            "carry" => {
                flag(self, OPT_CARRY, value != 0);
                true
            }
            "mode" => {
                self.value = value.clamp(0, 2) as u32;
                true
            }
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// 可编辑参数描述
//
// UI 与 CLI 据此外自动生成控件，从而不必各自硬编码一份"哪个组件能改什么"。
// 组件知识的唯一数据源这条规矩，对参数同样适用。
// ---------------------------------------------------------------------------

/// 参数控件类型
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamKind {
    /// 从固定档位里选；labels 为空则显示数字本身
    Choice { values: &'static [i64], labels: &'static [&'static str] },
    /// 整数范围
    Int { min: i64, max: i64 },
    /// 布尔开关，对应 opts 里的某一位
    Bool { bit: u32 },
}

/// 一个可编辑参数的描述
#[derive(Clone, Copy, Debug)]
pub struct ParamDesc {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
}

const WIDTH_CHOICES: &[i64] = &[1, 4, 8, 16, 32];

const P_WIDTH: ParamDesc = ParamDesc {
    key: "width",
    label: "位宽",
    kind: ParamKind::Choice { values: WIDTH_CHOICES, labels: &[] },
};
const P_INPUTS: ParamDesc =
    ParamDesc { key: "inputs", label: "输入数", kind: ParamKind::Int { min: 2, max: 8 } };
const P_VALUE: ParamDesc =
    ParamDesc { key: "value", label: "值", kind: ParamKind::Int { min: 0, max: 65535 } };
const P_HIGH: ParamDesc =
    ParamDesc { key: "high", label: "高电平拍数", kind: ParamKind::Int { min: 1, max: 255 } };
const P_LOW: ParamDesc =
    ParamDesc { key: "low", label: "低电平拍数", kind: ParamKind::Int { min: 1, max: 255 } };
const P_DEPTH: ParamDesc =
    ParamDesc { key: "depth", label: "深度", kind: ParamKind::Int { min: 2, max: 4096 } };
const P_LOW_BITS: ParamDesc =
    ParamDesc { key: "low_bits", label: "低段位宽", kind: ParamKind::Int { min: 1, max: 31 } };
const P_SIGNED: ParamDesc =
    ParamDesc { key: "signed", label: "有符号比较", kind: ParamKind::Bool { bit: OPT_SIGNED } };
const P_ENABLE: ParamDesc =
    ParamDesc { key: "enable", label: "使能引脚", kind: ParamKind::Bool { bit: OPT_ENABLE } };
const P_RESET: ParamDesc =
    ParamDesc { key: "reset", label: "复位引脚", kind: ParamKind::Bool { bit: OPT_RESET } };
const P_CARRY: ParamDesc =
    ParamDesc { key: "carry", label: "进位引脚", kind: ParamKind::Bool { bit: OPT_CARRY } };
/// 移位器方向（v4 §6.4 要求左移 / 逻辑右移 / 算术右移三种）
const P_SHIFT_MODE: ParamDesc = ParamDesc {
    key: "mode",
    label: "移位方向",
    kind: ParamKind::Choice {
        values: &[0, 1, 2],
        labels: &["左移", "逻辑右移", "算术右移"],
    },
};

/// 移位器方向：0 左移、1 逻辑右移、2 算术右移
#[inline]
pub fn shift_mode(p: &Params) -> u8 {
    p.value.min(2) as u8
}

// ---------------------------------------------------------------------------
// 组件种类
// ---------------------------------------------------------------------------

/// 内置组件种类
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum DefId {
    // 输入 / 输出
    Button,
    Switch,
    Clock,
    Constant,
    Led,
    // 逻辑门
    Not,
    Buffer,
    And,
    Or,
    Nand,
    Nor,
    Xor,
    Xnor,
    // 多位 / 选择
    Splitter,
    Merger,
    Mux,
    Decoder,
    PriorityEncoder,
    // 算术
    HalfAdder,
    FullAdder,
    Adder,
    Subtractor,
    Multiplier,
    Comparator,
    Shifter,
    Alu,
    // 存储
    Dff,
    Register,
    Counter,
    Ram,
    Rom,
    // 层次化（ADR-28）：子电路接口与外壳。
    // Custom 不出现在元件库（ALL）里——它的引脚与行为都由被引用的图纸决定。
    Custom,
    /// 子电路内部：把外部输入引进来的接口元件
    InputPin,
    /// 子电路内部：把结果送出去的接口元件
    OutputPin,
    // 外设
    SevenSeg,
    Display,
}

impl DefId {
    /// 全部内置组件（元件库顺序）
    pub const ALL: &'static [DefId] = &[
        DefId::Button,
        DefId::Switch,
        DefId::Clock,
        DefId::Constant,
        DefId::Led,
        DefId::Not,
        DefId::Buffer,
        DefId::And,
        DefId::Or,
        DefId::Nand,
        DefId::Nor,
        DefId::Xor,
        DefId::Xnor,
        DefId::Splitter,
        DefId::Merger,
        DefId::Mux,
        DefId::Decoder,
        DefId::PriorityEncoder,
        DefId::HalfAdder,
        DefId::FullAdder,
        DefId::Adder,
        DefId::Subtractor,
        DefId::Multiplier,
        DefId::Comparator,
        DefId::Shifter,
        DefId::Alu,
        DefId::Dff,
        DefId::Register,
        DefId::Counter,
        DefId::Ram,
        DefId::Rom,
        DefId::InputPin,
        DefId::OutputPin,
        DefId::SevenSeg,
        DefId::Display,
    ];

    /// 稳定标识串（序列化 / 桥接用）
    pub fn id(self) -> &'static str {
        match self {
            DefId::Button => "button",
            DefId::Switch => "switch",
            DefId::Clock => "clock",
            DefId::Constant => "constant",
            DefId::Led => "led",
            DefId::Not => "not",
            DefId::Buffer => "buffer",
            DefId::And => "and",
            DefId::Or => "or",
            DefId::Nand => "nand",
            DefId::Nor => "nor",
            DefId::Xor => "xor",
            DefId::Xnor => "xnor",
            DefId::Splitter => "splitter",
            DefId::Merger => "merger",
            DefId::Mux => "mux",
            DefId::Decoder => "decoder",
            DefId::PriorityEncoder => "prio_enc",
            DefId::HalfAdder => "half_adder",
            DefId::FullAdder => "full_adder",
            DefId::Adder => "adder",
            DefId::Subtractor => "subtractor",
            DefId::Multiplier => "multiplier",
            DefId::Comparator => "comparator",
            DefId::Shifter => "shifter",
            DefId::Alu => "alu",
            DefId::Dff => "dff",
            DefId::Register => "register",
            DefId::Counter => "counter",
            DefId::Ram => "ram",
            DefId::Rom => "rom",
            DefId::Custom => "custom",
            DefId::InputPin => "input_pin",
            DefId::OutputPin => "output_pin",
            DefId::SevenSeg => "seven_seg",
            DefId::Display => "display",
        }
    }

    /// 由标识串还原
    pub fn from_id(s: &str) -> Option<DefId> {
        if s == DefId::Custom.id() {
            return Some(DefId::Custom);
        }
        DefId::ALL.iter().copied().find(|d| d.id() == s)
    }

    /// 显示名（行业通用术语）
    pub fn label(self) -> &'static str {
        match self {
            DefId::Button => "按钮",
            DefId::Switch => "开关",
            DefId::Clock => "时钟",
            DefId::Constant => "常量",
            DefId::Led => "探针 / LED",
            DefId::Not => "NOT",
            DefId::Buffer => "Buffer",
            DefId::And => "AND",
            DefId::Or => "OR",
            DefId::Nand => "NAND",
            DefId::Nor => "NOR",
            DefId::Xor => "XOR",
            DefId::Xnor => "XNOR",
            DefId::Splitter => "Splitter",
            DefId::Merger => "Merger",
            DefId::Mux => "MUX",
            DefId::Decoder => "译码器",
            DefId::PriorityEncoder => "优先编码器",
            DefId::HalfAdder => "半加器",
            DefId::FullAdder => "全加器",
            DefId::Adder => "加法器",
            DefId::Subtractor => "减法器",
            DefId::Multiplier => "乘法器",
            DefId::Comparator => "比较器",
            DefId::Shifter => "移位器",
            DefId::Alu => "ALU",
            DefId::Dff => "D 触发器",
            DefId::Register => "寄存器",
            DefId::Counter => "计数器",
            DefId::Ram => "RAM",
            DefId::Rom => "ROM",
            DefId::Custom => "子电路",
            DefId::InputPin => "输入接口",
            DefId::OutputPin => "输出接口",
            DefId::SevenSeg => "数码管",
            DefId::Display => "显示屏",
        }
    }

    pub fn category(self) -> Category {
        match self {
            DefId::Button | DefId::Switch | DefId::Clock | DefId::Constant | DefId::Led => {
                Category::Io
            }
            DefId::Not
            | DefId::Buffer
            | DefId::And
            | DefId::Or
            | DefId::Nand
            | DefId::Nor
            | DefId::Xor
            | DefId::Xnor => Category::Logic,
            DefId::Splitter
            | DefId::Merger
            | DefId::Mux
            | DefId::Decoder
            | DefId::PriorityEncoder => Category::Routing,
            DefId::HalfAdder
            | DefId::FullAdder
            | DefId::Adder
            | DefId::Subtractor
            | DefId::Multiplier
            | DefId::Comparator
            | DefId::Shifter
            | DefId::Alu => Category::Arithmetic,
            DefId::Dff | DefId::Register | DefId::Counter | DefId::Ram | DefId::Rom => {
                Category::Memory
            }
            DefId::Custom
            | DefId::InputPin
            | DefId::OutputPin
            | DefId::SevenSeg
            | DefId::Display => Category::Io,
        }
    }

    /// 纯观察组件：只有输入、不驱动任何网络（万级探针也不拖慢 tick）
    pub fn is_sink(self) -> bool {
        matches!(self, DefId::Led | DefId::SevenSeg | DefId::OutputPin)
    }

    /// 引擎**不为其建运行时组件**的种类。
    ///
    /// 观察终端与子电路接口本身没有行为：接口引脚在层次展开时已被并成同一个
    /// 网络（elaborate），外壳则由子电路内容替代。它们照样进网表、照样能连线。
    pub fn is_passive(self) -> bool {
        self.is_sink() || matches!(self, DefId::InputPin | DefId::Custom)
    }

    /// 无输入源组件
    pub fn is_source(self) -> bool {
        matches!(
            self,
            DefId::Button | DefId::Switch | DefId::Clock | DefId::Constant | DefId::InputPin
        )
    }

    /// 含内部状态的时序组件
    pub fn is_sequential(self) -> bool {
        matches!(
            self,
            DefId::Dff | DefId::Register | DefId::Counter | DefId::Ram | DefId::Display
        )
    }

    /// 是否允许多位
    pub fn supports_width(self) -> bool {
        !matches!(
            self,
            DefId::Clock
                | DefId::Led
                | DefId::Button
                | DefId::Switch
                | DefId::HalfAdder
                | DefId::FullAdder
                | DefId::Dff
                | DefId::Custom
        )
    }

    /// 参数化输入数
    pub fn has_input_count(self) -> bool {
        matches!(
            self,
            DefId::And
                | DefId::Or
                | DefId::Nand
                | DefId::Nor
                | DefId::Xor
                | DefId::Xnor
                | DefId::Mux
                | DefId::PriorityEncoder
        )
    }

    pub fn default_params(self) -> Params {
        match self {
            DefId::Not | DefId::Buffer => Params::default().width(1),
            DefId::And | DefId::Or | DefId::Nand | DefId::Nor | DefId::Xor | DefId::Xnor => {
                Params::default().width(1).inputs(2)
            }
            DefId::Clock => Params::default().width(1),
            DefId::Constant | DefId::Switch | DefId::Button => Params::default().width(1),
            DefId::Led => Params::default().width(1),
            DefId::Splitter => Params::default().width(4),
            DefId::Merger => Params::default().width(4),
            DefId::Mux => Params::default().width(8).inputs(2),
            DefId::Decoder => Params::default().width(8),
            DefId::PriorityEncoder => Params::default().width(1).inputs(4),
            DefId::HalfAdder | DefId::FullAdder => Params::default().width(1),
            DefId::Adder | DefId::Subtractor => Params::default().width(8).opts(OPT_CARRY),
            DefId::Multiplier => Params::default().width(8),
            DefId::Comparator => Params::default().width(8),
            DefId::Shifter => Params::default().width(8),
            DefId::Alu => Params::default().width(8),
            DefId::Dff => Params::default().width(1).opts(OPT_ENABLE),
            DefId::Register => Params::default().width(8).opts(OPT_ENABLE | OPT_RESET),
            DefId::Counter => Params::default().width(8).opts(OPT_ENABLE | OPT_RESET),
            DefId::Ram => Params::default().width(8).depth(256),
            DefId::Rom => Params::default().width(8).depth(256),
            DefId::Custom => Params::default(),
            DefId::InputPin | DefId::OutputPin | DefId::SevenSeg => Params::default().width(1),
            // 显示屏：位宽 = 每行像素数，深度 = 行数
            DefId::Display => Params::default().width(8).depth(8),
        }
    }

    /// 该组件可编辑的参数（UI 据此自动生成控件）
    pub fn params(self) -> &'static [ParamDesc] {
        match self {
            DefId::Clock => &[P_HIGH, P_LOW],
            DefId::Constant => &[P_WIDTH, P_VALUE],
            DefId::Button | DefId::Switch => &[P_WIDTH],
            DefId::Led => &[P_WIDTH],
            DefId::Not | DefId::Buffer => &[P_WIDTH],
            DefId::And | DefId::Or | DefId::Nand | DefId::Nor | DefId::Xor | DefId::Xnor => {
                &[P_WIDTH, P_INPUTS]
            }
            DefId::Splitter | DefId::Merger => &[P_WIDTH, P_LOW_BITS],
            DefId::Mux => &[P_WIDTH, P_INPUTS],
            DefId::Decoder => &[P_WIDTH],
            DefId::PriorityEncoder => &[P_INPUTS],
            DefId::HalfAdder | DefId::FullAdder => &[],
            DefId::Adder | DefId::Subtractor => &[P_WIDTH, P_CARRY],
            DefId::Multiplier => &[P_WIDTH],
            DefId::Comparator => &[P_WIDTH, P_SIGNED],
            DefId::Shifter => &[P_WIDTH, P_SHIFT_MODE],
            DefId::Alu => &[P_WIDTH],
            DefId::Dff => &[P_ENABLE, P_RESET],
            DefId::Register | DefId::Counter => &[P_WIDTH, P_ENABLE, P_RESET],
            DefId::Ram | DefId::Rom => &[P_WIDTH, P_DEPTH],
            DefId::Custom => &[],
            DefId::InputPin | DefId::OutputPin | DefId::SevenSeg => &[P_WIDTH],
            DefId::Display => &[P_WIDTH, P_DEPTH],
        }
    }

    /// 该实例的引脚布局（编辑期调用一次）
    pub fn pins(self, p: &Params) -> Vec<PinDef> {
        let w = p.width;
        let n = (p.inputs.max(2)) as i32;
        let ins = Dir::In;
        let out = Dir::Out;
        let mut v: Vec<PinDef> = Vec::new();

        match self {
            DefId::Not | DefId::Buffer => {
                v.push(pin("A", 0, 0, w, ins));
                v.push(pin("Y", 2, 0, w, out));
            }
            DefId::And | DefId::Or | DefId::Nand | DefId::Nor | DefId::Xor | DefId::Xnor => {
                for i in 0..n {
                    v.push(pin(&format!("A{i}"), 0, i, w, ins));
                }
                v.push(pin("Y", 2, (n - 1) / 2, w, out));
            }
            DefId::Button | DefId::Switch | DefId::Constant => {
                v.push(pin("Q", 2, 0, w, out));
            }
            DefId::Clock => {
                v.push(pin("CLK", 2, 0, 1, out));
            }
            DefId::Led => {
                v.push(pin("D", 0, 0, w, ins));
            }
            // 任意两段位宽组合（v4 §6.3）：LO + HI = 总位宽，可级联拆更多段
            DefId::Splitter => {
                let low = low_bits(p);
                v.push(pin("D", 0, 0, w, ins));
                v.push(pin("LO", 2, 0, low, out));
                v.push(pin("HI", 2, 2, w - low, out));
            }
            DefId::Merger => {
                let low = low_bits(p);
                v.push(pin("LO", 0, 0, low, ins));
                v.push(pin("HI", 0, 2, w - low, ins));
                v.push(pin("Y", 2, 1, w, out));
            }
            DefId::Mux => {
                let sel_w = bits_needed(p.inputs.max(2) as u32);
                for i in 0..n {
                    v.push(pin(&format!("D{i}"), 0, i, w, ins));
                }
                v.push(pin("S", 0, n, sel_w, ins));
                v.push(pin("Y", 2, (n - 1) / 2, w, out));
            }
            DefId::Decoder => {
                let nout = 1i32 << bits_needed(p.width.max(2) as u32);
                v.push(pin("A", 0, 0, bits_needed(nout as u32), ins));
                for i in 0..nout {
                    v.push(pin(&format!("Y{i}"), 2, i, 1, out));
                }
            }
            DefId::PriorityEncoder => {
                for i in 0..n {
                    v.push(pin(&format!("A{i}"), 0, i, 1, ins));
                }
                v.push(pin("IDX", 2, 0, bits_needed(n as u32), out));
                v.push(pin("VALID", 2, 1, 1, out));
            }
            DefId::HalfAdder => {
                v.push(pin("A", 0, 0, 1, ins));
                v.push(pin("B", 0, 1, 1, ins));
                v.push(pin("S", 2, 0, 1, out));
                v.push(pin("C", 2, 1, 1, out));
            }
            DefId::FullAdder => {
                v.push(pin("A", 0, 0, 1, ins));
                v.push(pin("B", 0, 1, 1, ins));
                v.push(pin("CI", 0, 2, 1, ins));
                v.push(pin("S", 2, 0, 1, out));
                v.push(pin("CO", 2, 1, 1, out));
            }
            DefId::Adder | DefId::Subtractor => {
                v.push(pin("A", 0, 0, w, ins));
                v.push(pin("B", 0, 1, w, ins));
                if p.opts & OPT_CARRY != 0 {
                    v.push(pin(if self == DefId::Adder { "CI" } else { "BI" }, 0, 2, 1, ins));
                }
                v.push(pin("S", 2, 0, w, out));
                if p.opts & OPT_CARRY != 0 {
                    v.push(pin(if self == DefId::Adder { "CO" } else { "BO" }, 2, 1, 1, out));
                }
            }
            DefId::Multiplier => {
                v.push(pin("A", 0, 0, w, ins));
                v.push(pin("B", 0, 1, w, ins));
                v.push(pin("P", 2, 0, w, out));
            }
            DefId::Comparator => {
                v.push(pin("A", 0, 0, w, ins));
                v.push(pin("B", 0, 1, w, ins));
                v.push(pin("EQ", 2, 0, 1, out));
                v.push(pin("LT", 2, 1, 1, out));
                v.push(pin("LTU", 2, 2, 1, out));
            }
            DefId::Shifter => {
                v.push(pin("A", 0, 0, w, ins));
                v.push(pin("SH", 0, 1, bits_needed(w as u32), ins));
                v.push(pin("Y", 2, 0, w, out));
            }
            DefId::Alu => {
                v.push(pin("A", 0, 0, w, ins));
                v.push(pin("B", 0, 1, w, ins));
                v.push(pin("OP", 0, 2, 4, ins));
                v.push(pin("Y", 2, 0, w, out));
            }
            DefId::Dff | DefId::Register => {
                let dw = if self == DefId::Dff { 1 } else { w };
                v.push(pin("D", 0, 0, dw, ins));
                v.push(pin("CLK", 0, 1, 1, ins));
                if p.opts & OPT_ENABLE != 0 {
                    v.push(pin("EN", 0, 2, 1, ins));
                }
                if p.opts & OPT_RESET != 0 {
                    v.push(pin("RST", 0, 3, 1, ins));
                }
                v.push(pin("Q", 2, 1, dw, out));
            }
            DefId::Counter => {
                v.push(pin("CLK", 0, 0, 1, ins));
                if p.opts & OPT_ENABLE != 0 {
                    v.push(pin("EN", 0, 1, 1, ins));
                }
                if p.opts & OPT_RESET != 0 {
                    v.push(pin("RST", 0, 2, 1, ins));
                }
                v.push(pin("Q", 2, 0, w, out));
            }
            DefId::Ram => {
                let aw = bits_needed(p.depth.max(2) as u32);
                v.push(pin("ADDR", 0, 0, aw, ins));
                v.push(pin("DIN", 0, 1, w, ins));
                v.push(pin("WE", 0, 2, 1, ins));
                v.push(pin("CLK", 0, 3, 1, ins));
                v.push(pin("DOUT", 2, 1, w, out));
            }
            DefId::Rom => {
                let aw = bits_needed(p.depth.max(2) as u32);
                v.push(pin("ADDR", 0, 0, aw, ins));
                v.push(pin("DOUT", 2, 0, w, out));
            }
            // 子电路接口与外壳：引脚来自图纸内容，由 Session::sync_shapes 写入缓存
            DefId::InputPin => v.push(pin("Y", 2, 0, w, out)),
            DefId::OutputPin => v.push(pin("A", 0, 0, w, ins)),
            DefId::SevenSeg => v.push(pin("D", 0, 0, w, ins)),
            DefId::Display => {
                let aw = bits_needed(p.depth.max(2) as u32);
                v.push(pin("DATA", 0, 0, w, ins));
                v.push(pin("ADDR", 0, 1, aw, ins));
                v.push(pin("WE", 0, 2, 1, ins));
                v.push(pin("CLK", 0, 3, 1, ins));
            }
            DefId::Custom => {}
        }
        v
    }

    /// 输出引脚数量（引擎用于校验 / 预分配）
    pub fn out_count(self, p: &Params) -> usize {
        match self {
            DefId::Led => 0,
            DefId::Splitter => 2,
            DefId::Decoder => 1usize << bits_needed(p.width.max(2) as u32),
            DefId::PriorityEncoder => 2,
            DefId::HalfAdder | DefId::FullAdder => 2,
            DefId::Adder | DefId::Subtractor => {
                if p.opts & OPT_CARRY != 0 {
                    2
                } else {
                    1
                }
            }
            DefId::Comparator => 3,
            DefId::Custom | DefId::OutputPin | DefId::SevenSeg | DefId::Display => 0,
            _ => 1,
        }
    }

    /// 输入引脚数量
    pub fn in_count(self, p: &Params) -> usize {
        match self {
            DefId::Button | DefId::Switch | DefId::Clock | DefId::Constant => 0,
            DefId::Led => 1,
            DefId::Not | DefId::Buffer => 1,
            DefId::And
            | DefId::Or
            | DefId::Nand
            | DefId::Nor
            | DefId::Xor
            | DefId::Xnor => p.inputs.max(2) as usize,
            DefId::Splitter => 1,
            DefId::Merger => 2,
            DefId::Mux => p.inputs.max(2) as usize + 1,
            DefId::Decoder => 1,
            DefId::PriorityEncoder => p.inputs.max(2) as usize,
            DefId::HalfAdder => 2,
            DefId::FullAdder => 3,
            DefId::Adder | DefId::Subtractor => {
                if p.opts & OPT_CARRY != 0 {
                    3
                } else {
                    2
                }
            }
            DefId::Multiplier => 2,
            DefId::Comparator => 2,
            DefId::Shifter => 2,
            DefId::Alu => 3,
            DefId::Dff | DefId::Register => {
                2 + (p.opts & OPT_ENABLE != 0) as usize + (p.opts & OPT_RESET != 0) as usize
            }
            DefId::Counter => {
                1 + (p.opts & OPT_ENABLE != 0) as usize + (p.opts & OPT_RESET != 0) as usize
            }
            DefId::Ram => 4,
            DefId::Rom => 1,
            DefId::Custom | DefId::InputPin => 0,
            DefId::OutputPin | DefId::SevenSeg => 1,
            DefId::Display => 4,
        }
    }
}

impl std::fmt::Display for DefId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

// 序列化用稳定标识串，而不是 Rust 变体名——id() 是持久化契约，
// 重命名 Rust 变体不会破坏已保存的工程文件。
impl Serialize for DefId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for DefId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        DefId::from_id(&s).ok_or_else(|| serde::de::Error::custom(format!("未知组件类型: {s}")))
    }
}

// ---------------------------------------------------------------------------
// 实例状态
// ---------------------------------------------------------------------------

/// 实例私有状态。**不是**仿真输出——输出永远经暂存区提交。
#[derive(Clone, Debug, Default)]
pub struct CompState {
    /// 锁存值（D 触发器 / 寄存器）
    pub reg: u32,
    /// 上一 tick 的时钟电平，用于上升沿检测（ADR-1）
    pub prev_clk: u8,
    /// 计数器当前值
    pub cnt: u32,
    /// RAM / ROM 内容
    pub mem: Vec<u32>,
}

impl CompState {
    /// 按实例内容初始化存储器
    pub fn with_mem(init: &[u32], depth: u16) -> Self {
        let mut mem = vec![0u32; depth as usize];
        let n = init.len().min(mem.len());
        mem[..n].copy_from_slice(&init[..n]);
        Self { mem, ..Default::default() }
    }
}

// ---------------------------------------------------------------------------
// 求值
// ---------------------------------------------------------------------------

/// 上升沿检测并记录本拍时钟（ADR-1 / §16：状态只在实例内部演进）
#[inline]
fn rising(clk: NetValue, st: &mut CompState) -> bool {
    let cur = u8::from(clk.bit(0) == Bit::One);
    let edge = st.prev_clk == 0 && cur == 1;
    st.prev_clk = cur;
    edge
}

/// 求值入口。要求 `outs.len() == def.out_count(p)`，`ins.len() == def.in_count(p)`。
pub fn eval(def: DefId, p: &Params, ins: &[NetValue], st: &mut CompState, outs: &mut [NetValue]) {
    match def {
        DefId::Not
        | DefId::Buffer
        | DefId::And
        | DefId::Or
        | DefId::Nand
        | DefId::Nor
        | DefId::Xor
        | DefId::Xnor => eval_gate(def, p, ins, outs),
        DefId::Constant | DefId::Switch | DefId::Button => {
            outs[0] = NetValue::from_u32(p.value, p.width);
        }
        DefId::Clock => eval_clock(p, st, outs),
        DefId::Splitter | DefId::Merger | DefId::Mux | DefId::Decoder
        | DefId::PriorityEncoder => eval_routing(def, p, ins, outs),
        DefId::HalfAdder
        | DefId::FullAdder
        | DefId::Adder
        | DefId::Subtractor
        | DefId::Multiplier
        | DefId::Comparator
        | DefId::Shifter
        | DefId::Alu => eval_arith(def, p, ins, outs),
        DefId::Dff | DefId::Register | DefId::Counter => eval_reg(def, p, ins, st, outs),
        DefId::Ram | DefId::Rom => eval_mem(def, p, ins, st, outs),
        DefId::Display => eval_display(p, ins, st),
        // 观察终端与子电路接口没有行为：它们的引脚在展开时已并成同一个网络
        DefId::Led | DefId::SevenSeg | DefId::OutputPin | DefId::InputPin | DefId::Custom => {}
    }
}

/// 显示屏：内存映射帧缓冲。每拍在时钟上升沿按写使能写入一行。
///
/// 画面状态放在 CompState.mem 里而不是壳里——「编辑即重置」「撤销」「读档」
/// 于是自动把画面一并复位，壳不需要知道任何像素细节。
fn eval_display(p: &Params, ins: &[NetValue], st: &mut CompState) {
    let edge = rising(ins[3], st);
    if !edge || ins[2].bit(0) != Bit::One {
        return;
    }
    let rows = p.depth.max(2) as usize;
    if st.mem.len() < rows {
        st.mem.resize(rows, 0);
    }
    let addr = (ins[1].get(32) as usize) % rows;
    st.mem[addr] = ins[0].get(32) & width_mask(p.width);
}

/// 时钟：上电输出低，低 `low` 拍后翻高、持续 `high` 拍（§5.2）
fn eval_clock(p: &Params, st: &mut CompState, outs: &mut [NetValue]) {
    let period = p.high as u32 + p.low as u32;
    if period == 0 {
        outs[0] = NetValue::ZERO;
        return;
    }
    let phase = st.cnt % period;
    st.cnt = st.cnt.wrapping_add(1);
    outs[0] = NetValue::from_bit(phase >= p.low as u32);
}

fn eval_gate(def: DefId, p: &Params, ins: &[NetValue], outs: &mut [NetValue]) {
    let w = p.width;
    let m = width_mask(w);

    if matches!(def, DefId::Not | DefId::Buffer) {
        let a = ins.first().copied().unwrap_or(NetValue::ZERO).with_width(w);
        outs[0] = if def == DefId::Not {
            NetValue::new(!a.val & m, a.unk)
        } else {
            a
        };
        return;
    }

    let mut it = ins.iter().map(|v| v.with_width(w));
    let first = it.next().unwrap_or(NetValue::ZERO);
    let mut val = first.val;
    let mut unk = first.unk;
    for x in it {
        unk |= x.unk;
        val = match def {
            DefId::And | DefId::Nand => val & x.val,
            DefId::Or | DefId::Nor => val | x.val,
            DefId::Xor | DefId::Xnor => val ^ x.val,
            _ => val,
        };
    }
    if matches!(def, DefId::Nand | DefId::Nor | DefId::Xnor) {
        val = !val & m;
    }
    outs[0] = NetValue::new(val & m, unk);
}

fn eval_routing(def: DefId, p: &Params, ins: &[NetValue], outs: &mut [NetValue]) {
    let w = p.width;
    match def {
        DefId::Splitter => {
            let low = low_bits(p);
            let a = ins[0];
            outs[0] = NetValue::new(a.val & width_mask(low), a.unk & width_mask(low));
            outs[1] = NetValue::new(
                (a.val >> low) & width_mask(w - low),
                (a.unk >> low) & width_mask(w - low),
            );
        }
        DefId::Merger => {
            let low = low_bits(p);
            let lo = ins[0];
            let hi = ins[1];
            outs[0] = NetValue::new(
                (lo.val & width_mask(low)) | ((hi.val & width_mask(w - low)) << low),
                (lo.unk & width_mask(low)) | ((hi.unk & width_mask(w - low)) << low),
            );
        }
        DefId::Mux => {
            let k = p.inputs.max(2) as usize;
            let sel_w = bits_needed(k as u32);
            let sel = ins[k].get(sel_w) as usize;
            outs[0] = if sel < k {
                ins[sel].with_width(w)
            } else {
                NetValue::all_unknown(w)
            };
        }
        DefId::Decoder => {
            let a = ins[0];
            if a.has_unknown() {
                for o in outs.iter_mut() {
                    *o = NetValue::all_unknown(1);
                }
            } else {
                let idx = a.get(bits_needed(outs.len() as u32));
                for (i, o) in outs.iter_mut().enumerate() {
                    *o = NetValue::from_bit(i as u32 == idx);
                }
            }
        }
        DefId::PriorityEncoder => {
            let mut idx = 0u32;
            let mut valid = false;
            for (i, v) in ins.iter().enumerate() {
                if !valid && v.bit(0) == Bit::One {
                    idx = i as u32;
                    valid = true;
                }
            }
            outs[0] = NetValue::from_u32(idx, bits_needed(ins.len() as u32));
            outs[1] = NetValue::from_bit(valid);
        }
        _ => {}
    }
}

fn eval_arith(def: DefId, p: &Params, ins: &[NetValue], outs: &mut [NetValue]) {
    let w = p.width;
    match def {
        DefId::HalfAdder => {
            let a = ins[0].bit(0) == Bit::One;
            let b = ins[1].bit(0) == Bit::One;
            outs[0] = NetValue::from_bit(a ^ b);
            outs[1] = NetValue::from_bit(a & b);
        }
        DefId::FullAdder => {
            let a = u32::from(ins[0].bit(0) == Bit::One);
            let b = u32::from(ins[1].bit(0) == Bit::One);
            let c = u32::from(ins[2].bit(0) == Bit::One);
            let (s, co) = add_wrap(a, b, c, 1);
            outs[0] = NetValue::from_u32(s, 1);
            outs[1] = NetValue::from_u32(co, 1);
        }
        DefId::Adder => {
            let cin = if p.opts & OPT_CARRY != 0 {
                u32::from(ins[2].bit(0) == Bit::One)
            } else {
                0
            };
            let (s, co) = add_wrap(ins[0].get(w), ins[1].get(w), cin, w);
            outs[0] = NetValue::from_u32(s, w);
            if outs.len() > 1 {
                outs[1] = NetValue::from_u32(co, 1);
            }
        }
        DefId::Subtractor => {
            let bin = if p.opts & OPT_CARRY != 0 {
                u32::from(ins[2].bit(0) == Bit::One)
            } else {
                0
            };
            let (d, bo) = sub_wrap(ins[0].get(w), ins[1].get(w), bin, w);
            outs[0] = NetValue::from_u32(d, w);
            if outs.len() > 1 {
                outs[1] = NetValue::from_u32(bo, 1);
            }
        }
        DefId::Multiplier => {
            outs[0] = NetValue::from_u32(mul_wrap(ins[0].get(w), ins[1].get(w), w), w);
        }
        DefId::Comparator => {
            let a = ins[0].get(w);
            let b = ins[1].get(w);
            outs[0] = NetValue::from_bit(a == b);
            outs[1] = NetValue::from_bit(to_signed(a, w) < to_signed(b, w));
            outs[2] = NetValue::from_bit(a < b);
        }
        DefId::Shifter => {
            let a = ins[0].get(w);
            let sh = ins[1].get(bits_needed(w as u32));
            let y = match shift_mode(p) {
                0 => shl(a, sh, w),
                1 => shr(a, sh, w),
                _ => sar(a, sh, w),
            };
            outs[0] = NetValue::from_u32(y, w);
        }
        DefId::Alu => {
            let a = ins[0].get(w);
            let b = ins[1].get(w);
            let op = ins[2].get(4);
            let y = match op {
                0 => add_wrap(a, b, 0, w).0,
                1 => sub_wrap(a, b, 0, w).0,
                2 => a & b,
                3 => a | b,
                4 => a ^ b,
                5 => !a & width_mask(w),
                6 => a,
                7 => b,
                8 => shl(a, b & 31, w),
                9 => shr(a, b & 31, w),
                10 => sar(a, b & 31, w),
                11 => mul_wrap(a, b, w),
                _ => 0,
            };
            outs[0] = NetValue::from_u32(y, w);
        }
        _ => {}
    }
}

fn eval_reg(def: DefId, p: &Params, ins: &[NetValue], st: &mut CompState, outs: &mut [NetValue]) {
    let w = if def == DefId::Dff { 1 } else { p.width };

    // 引脚顺序由 DefId::pins 钉死：D, CLK, [EN], [RST]
    let (clk, edge) = if def == DefId::Counter {
        (ins[0], rising(ins[0], st))
    } else {
        (ins[1], rising(ins[1], st))
    };
    let _ = clk;

    let mut k = if def == DefId::Counter { 1 } else { 2 };
    let en = if p.opts & OPT_ENABLE != 0 {
        let v = ins[k].bit(0) != Bit::Zero;
        k += 1;
        v
    } else {
        true
    };
    let rst = if p.opts & OPT_RESET != 0 {
        ins[k].bit(0) == Bit::One
    } else {
        false
    };

    // ADR-3：reset 优先于时钟锁存
    if rst {
        if def == DefId::Counter {
            st.cnt = 0;
        } else {
            st.reg = 0;
        }
    } else if edge && en {
        if def == DefId::Counter {
            st.cnt = st.cnt.wrapping_add(1) & width_mask(p.width);
        } else {
            st.reg = ins[0].get(w);
        }
    }

    outs[0] = if def == DefId::Counter {
        NetValue::from_u32(st.cnt, p.width)
    } else {
        NetValue::from_u32(st.reg, w)
    };
}

fn eval_mem(def: DefId, p: &Params, ins: &[NetValue], st: &mut CompState, outs: &mut [NetValue]) {
    let w = p.width;
    let aw = bits_needed(p.depth.max(2) as u32);
    let addr = ins[0].get(aw) as usize;

    if def == DefId::Ram {
        let edge = rising(ins[3], st);
        let we = ins[2].bit(0) == Bit::One;
        if edge && we && addr < st.mem.len() {
            st.mem[addr] = ins[1].get(w);
        }
    }

    outs[0] = if addr < st.mem.len() {
        NetValue::from_u32(st.mem[addr], w)
    } else {
        NetValue::ZERO
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 引脚布局与 in/out 计数必须自洽（防止两处定义漂移）
    #[test]
    fn pin_layout_matches_counts() {
        for &def in DefId::ALL {
            let p = def.default_params();
            let pins = def.pins(&p);
            let ins = pins.iter().filter(|x| x.dir == Dir::In).count();
            let outs = pins.iter().filter(|x| x.dir == Dir::Out).count();
            assert_eq!(ins, def.in_count(&p), "{def} 输入数不一致");
            assert_eq!(outs, def.out_count(&p), "{def} 输出数不一致");
        }
    }

    /// 参数化变体（位宽 / 输入数 / 可选引脚）也要自洽
    #[test]
    fn pin_layout_matches_counts_for_variants() {
        let widths = [1u8, 4, 8, 16, 32];
        for &def in DefId::ALL {
            for &w in &widths {
                let mut p = def.default_params();
                p.width = normalize_width(w as u32);
                let pins = def.pins(&p);
                assert_eq!(
                    pins.iter().filter(|x| x.dir == Dir::In).count(),
                    def.in_count(&p),
                    "{def} w={w} 输入数不一致"
                );
                assert_eq!(
                    pins.iter().filter(|x| x.dir == Dir::Out).count(),
                    def.out_count(&p),
                    "{def} w={w} 输出数不一致"
                );
            }
            if def.has_input_count() {
                for n in 2u8..=8 {
                    let mut p = def.default_params();
                    p.inputs = n;
                    let pins = def.pins(&p);
                    assert_eq!(
                        pins.iter().filter(|x| x.dir == Dir::In).count(),
                        def.in_count(&p),
                        "{def} n={n} 输入数不一致"
                    );
                }
            }
        }
    }

    fn one(def: DefId, p: &Params, ins: &[NetValue]) -> NetValue {
        let mut st = CompState::default();
        let mut outs = vec![NetValue::ZERO; def.out_count(p)];
        eval(def, p, ins, &mut st, &mut outs);
        outs[0]
    }

    /// **全部 8 种门 × 全部输入组合**，逐一对照「把输入当布尔值再套布尔运算」的参考结果。
    ///
    /// 刻意穷举而不是挑几个用例：门的语义错一位，随机用例可能很久都撞不上。
    #[test]
    fn gates_cover_all_input_combinations() {
        for n in 2..=3u8 {
            let p = DefId::And.default_params().inputs(n).width(1);
            for mask in 0u32..(1 << n) {
                let ins: Vec<NetValue> = (0..n)
                    .map(|i| NetValue::from_bit(mask & (1 << i) != 0))
                    .collect();
                let bit = |i: usize| ins[i].get(1) != 0;
                let all = (0..n as usize).all(bit);
                let any = (0..n as usize).any(bit);
                let odd = (0..n as usize).filter(|&i| bit(i)).count() % 2 == 1;
                for (def, want) in [
                    (DefId::And, all),
                    (DefId::Nand, !all),
                    (DefId::Or, any),
                    (DefId::Nor, !any),
                    (DefId::Xor, odd),
                    (DefId::Xnor, !odd),
                ] {
                    assert_eq!(
                        one(def, &p, &ins).get(1),
                        u32::from(want),
                        "{def:?} 在 n={n} mask={mask:b} 上出错"
                    );
                }
                // 单输入门只看第一个输入
                assert_eq!(one(DefId::Not, &p, &ins[..1]).get(1), u32::from(!bit(0)));
                assert_eq!(one(DefId::Buffer, &p, &ins[..1]).get(1), u32::from(bit(0)));
            }
        }
    }

    #[test]
    fn multi_bit_gates_are_bitwise() {
        let p = DefId::And.default_params().width(8);
        let a = NetValue::from_u32(0xF0, 8);
        let b = NetValue::from_u32(0x3C, 8);
        assert_eq!(one(DefId::And, &p, &[a, b]).get(8), 0x30);
        assert_eq!(one(DefId::Or, &p, &[a, b]).get(8), 0xFC);
        assert_eq!(one(DefId::Xor, &p, &[a, b]).get(8), 0xCC);
    }

    /// v4 §6.3：Splitter / Merger 是**任意两段位宽**组合，不是拆成一个个 1 位
    #[test]
    fn splitter_merger_are_inverse() {
        // 8 位 → 低 4 位 + 高 4 位
        let ps = Params { value: 4, ..DefId::Splitter.default_params().width(8) };
        let pm = Params { value: 4, ..DefId::Merger.default_params().width(8) };
        let v = NetValue::from_u32(0xB7, 8);
        let mut st = CompState::default();
        let mut parts = vec![NetValue::ZERO; 2];
        eval(DefId::Splitter, &ps, &[v], &mut st, &mut parts);
        assert_eq!(parts[0].get(4), 0x7, "LO 段是低 4 位");
        assert_eq!(parts[1].get(4), 0xB, "HI 段是高 4 位");
        let mut merged = vec![NetValue::ZERO; 1];
        eval(DefId::Merger, &pm, &parts, &mut st, &mut merged);
        assert_eq!(merged[0].get(8), 0xB7, "拆开再合并应还原");
    }

    /// 非对半的两段也要正确（例如 8 → 3 + 5）
    #[test]
    fn splitter_supports_uneven_segments() {
        let ps = Params { value: 3, ..DefId::Splitter.default_params().width(8) };
        let pm = Params { value: 3, ..DefId::Merger.default_params().width(8) };
        let v = NetValue::from_u32(0b1011_0101, 8);
        let mut st = CompState::default();
        let mut parts = vec![NetValue::ZERO; 2];
        eval(DefId::Splitter, &ps, &[v], &mut st, &mut parts);
        assert_eq!(parts[0].get(3), 0b101);
        assert_eq!(parts[1].get(5), 0b10110);
        let mut merged = vec![NetValue::ZERO; 1];
        eval(DefId::Merger, &pm, &parts, &mut st, &mut merged);
        assert_eq!(merged[0].get(8), 0b1011_0101);
    }

    #[test]
    fn mux_selects_channel() {
        let p = DefId::Mux.default_params().width(8).inputs(4);
        let ins = [
            NetValue::from_u32(10, 8),
            NetValue::from_u32(20, 8),
            NetValue::from_u32(30, 8),
            NetValue::from_u32(40, 8),
            NetValue::from_u32(2, 8),
        ];
        assert_eq!(one(DefId::Mux, &p, &ins).get(8), 30);
    }

    #[test]
    fn adder_carries_out_and_wraps() {
        let p = DefId::Adder.default_params().width(8).opts(OPT_CARRY);
        let mut st = CompState::default();
        let mut outs = vec![NetValue::ZERO; 2];
        eval(
            DefId::Adder,
            &p,
            &[
                NetValue::from_u32(200, 8),
                NetValue::from_u32(100, 8),
                NetValue::ZERO,
            ],
            &mut st,
            &mut outs,
        );
        assert_eq!(outs[0].get(8), 44);
        assert_eq!(outs[1].get(1), 1);
    }

    #[test]
    fn comparator_distinguishes_signed_and_unsigned() {
        let p = DefId::Comparator.default_params().width(8);
        let mut st = CompState::default();
        let mut outs = vec![NetValue::ZERO; 3];
        // 0xFF = -1 (signed) / 255 (unsigned)，与 1 比较
        eval(
            DefId::Comparator,
            &p,
            &[NetValue::from_u32(0xFF, 8), NetValue::from_u32(1, 8)],
            &mut st,
            &mut outs,
        );
        assert_eq!(outs[0].get(1), 0, "EQ 应为 0");
        assert_eq!(outs[1].get(1), 1, "有符号 -1 < 1");
        assert_eq!(outs[2].get(1), 0, "无符号 255 < 1 为假");
    }

    #[test]
    fn alu_ops() {
        let p = DefId::Alu.default_params().width(8);
        let run = |op: u32| {
            one(
                DefId::Alu,
                &p,
                &[
                    NetValue::from_u32(12, 8),
                    NetValue::from_u32(10, 8),
                    NetValue::from_u32(op, 4),
                ],
            )
            .get(8)
        };
        assert_eq!(run(0), 22);
        assert_eq!(run(1), 2);
        assert_eq!(run(2), 8);
        assert_eq!(run(3), 14);
        assert_eq!(run(4), 6);
        assert_eq!(run(5), 243);
        assert_eq!(run(8), 12u32.wrapping_shl(10) & 0xFF);
    }

    #[test]
    fn register_latches_on_rising_edge_only() {
        let p = DefId::Register.default_params().width(8).opts(0);
        let mut st = CompState::default();
        let mut outs = vec![NetValue::ZERO; 1];
        let d = NetValue::from_u32(0x5A, 8);
        // 第 1 拍：clk=0 → 不锁存
        eval(DefId::Register, &p, &[d, NetValue::ZERO], &mut st, &mut outs);
        assert_eq!(outs[0].get(8), 0);
        // 第 2 拍：clk 上升沿 → 锁存
        eval(DefId::Register, &p, &[d, NetValue::ONE], &mut st, &mut outs);
        assert_eq!(outs[0].get(8), 0x5A);
        // 第 3 拍：clk 保持高，D 变化 → 不锁存
        eval(
            DefId::Register,
            &p,
            &[NetValue::from_u32(0x11, 8), NetValue::ONE],
            &mut st,
            &mut outs,
        );
        assert_eq!(outs[0].get(8), 0x5A);
        // 第 4 拍：clk 落回 0
        eval(DefId::Register, &p, &[d, NetValue::ZERO], &mut st, &mut outs);
        assert_eq!(outs[0].get(8), 0x5A);
        // 第 5 拍：再次上升沿 → 锁存新值
        eval(
            DefId::Register,
            &p,
            &[NetValue::from_u32(0x11, 8), NetValue::ONE],
            &mut st,
            &mut outs,
        );
        assert_eq!(outs[0].get(8), 0x11);
    }

    #[test]
    fn register_enable_gates_latch() {
        let p = DefId::Register.default_params().width(8).opts(OPT_ENABLE);
        let mut st = CompState::default();
        let mut outs = vec![NetValue::ZERO; 1];
        let d = NetValue::from_u32(0x77, 8);
        // 上升沿但 EN=0 → 不锁存
        eval(DefId::Register, &p, &[d, NetValue::ONE, NetValue::ZERO], &mut st, &mut outs);
        assert_eq!(outs[0].get(8), 0);
        // 落回并再上升，EN=1 → 锁存
        eval(DefId::Register, &p, &[d, NetValue::ZERO, NetValue::ONE], &mut st, &mut outs);
        eval(DefId::Register, &p, &[d, NetValue::ONE, NetValue::ONE], &mut st, &mut outs);
        assert_eq!(outs[0].get(8), 0x77);
    }

    #[test]
    fn reset_beats_clock_edge() {
        let p = DefId::Register
            .default_params()
            .width(8)
            .opts(OPT_ENABLE | OPT_RESET);
        let mut st = CompState::default();
        let mut outs = vec![NetValue::ZERO; 1];
        let d = NetValue::from_u32(0x99, 8);
        // 先存进一个值
        eval(DefId::Register, &p, &[d, NetValue::ZERO, NetValue::ONE, NetValue::ZERO], &mut st, &mut outs);
        eval(DefId::Register, &p, &[d, NetValue::ONE, NetValue::ONE, NetValue::ZERO], &mut st, &mut outs);
        assert_eq!(outs[0].get(8), 0x99);
        // RST=1 且同时有上升沿 → 复位优先
        eval(DefId::Register, &p, &[d, NetValue::ZERO, NetValue::ONE, NetValue::ZERO], &mut st, &mut outs);
        eval(DefId::Register, &p, &[d, NetValue::ONE, NetValue::ONE, NetValue::ONE], &mut st, &mut outs);
        assert_eq!(outs[0].get(8), 0);
    }

    #[test]
    fn counter_increments_on_edge_and_wraps() {
        let p = DefId::Counter.default_params().width(4).opts(0);
        let mut st = CompState::default();
        let mut outs = vec![NetValue::ZERO; 1];
        let mut ticks = 0u32;
        // 15 次完整时钟周期后应回绕到 15
        for _ in 0..15 {
            eval(DefId::Counter, &p, &[NetValue::ZERO], &mut st, &mut outs);
            eval(DefId::Counter, &p, &[NetValue::ONE], &mut st, &mut outs);
            ticks += 1;
        }
        assert_eq!(outs[0].get(4), 15, "ticks={ticks}");
        // 再来一次上升沿 → 回绕到 0
        eval(DefId::Counter, &p, &[NetValue::ZERO], &mut st, &mut outs);
        eval(DefId::Counter, &p, &[NetValue::ONE], &mut st, &mut outs);
        assert_eq!(outs[0].get(4), 0);
    }

    #[test]
    fn ram_write_then_read() {
        let p = DefId::Ram.default_params().width(8).depth(16);
        let mut st = CompState::with_mem(&[], 16);
        let mut outs = vec![NetValue::ZERO; 1];
        // 上升沿 + WE=1 → 写入 addr=3
        eval(DefId::Ram, &p, &[NetValue::from_u32(3, 4), NetValue::from_u32(0xAB, 8), NetValue::ONE, NetValue::ZERO], &mut st, &mut outs);
        eval(DefId::Ram, &p, &[NetValue::from_u32(3, 4), NetValue::from_u32(0xAB, 8), NetValue::ONE, NetValue::ONE], &mut st, &mut outs);
        // 组合读
        eval(DefId::Ram, &p, &[NetValue::from_u32(3, 4), NetValue::ZERO, NetValue::ZERO, NetValue::ZERO], &mut st, &mut outs);
        assert_eq!(outs[0].get(8), 0xAB);
    }

    #[test]
    fn clock_phase_starts_low() {
        let p = Params { width: 1, inputs: 2, value: 0, opts: 0, high: 2, low: 3, depth: 256 };
        let mut st = CompState::default();
        let mut outs = vec![NetValue::ZERO; 1];
        let mut seq = Vec::new();
        for _ in 0..10 {
            eval(DefId::Clock, &p, &[], &mut st, &mut outs);
            seq.push(outs[0].get(1));
        }
        // 低 3 拍 → 高 2 拍 → 循环
        assert_eq!(seq, vec![0, 0, 0, 1, 1, 0, 0, 0, 1, 1]);
    }

    #[test]
    fn decoder_and_priority_encoder() {
        let pd = DefId::Decoder.default_params().width(4);
        let mut st = CompState::default();
        let mut outs = vec![NetValue::ZERO; 4];
        eval(DefId::Decoder, &pd, &[NetValue::from_u32(2, 2)], &mut st, &mut outs);
        assert_eq!(outs.iter().map(|v| v.get(1)).collect::<Vec<_>>(), vec![0, 0, 1, 0]);

        let pe = DefId::PriorityEncoder.default_params().inputs(4);
        let mut outs2 = vec![NetValue::ZERO; 2];
        eval(
            DefId::PriorityEncoder,
            &pe,
            &[
                NetValue::ZERO,
                NetValue::ZERO,
                NetValue::ONE,
                NetValue::ONE,
            ],
            &mut st,
            &mut outs2,
        );
        assert_eq!(outs2[0].get(2), 2);
        assert_eq!(outs2[1].get(1), 1);
    }

    #[test]
    fn unknown_propagates_through_gates() {
        let p = DefId::And.default_params().width(4);
        let x = NetValue::new(0b0000, 0b0010);
        let one_v = NetValue::from_u32(0b1111, 4);
        // 未知位 & 1 → 仍未知；已知位按位与
        let r = one(DefId::And, &p, &[x, one_v]);
        assert_eq!(r.bit(1), Bit::X);
        assert_eq!(r.bit(0), Bit::Zero);
        assert_eq!(r.bit(3), Bit::Zero);
    }

    /// 引擎依赖"输入引脚连续在前、输出连续在后"的布局不变量
    #[test]
    fn pins_group_inputs_before_outputs() {
        for &def in DefId::ALL {
            let mut variants = vec![def.default_params()];
            let mut p = def.default_params();
            p.opts |= OPT_ENABLE | OPT_RESET | OPT_CARRY;
            variants.push(p);
            for p in variants {
                let pins = def.pins(&p);
                let first_out = pins.iter().position(|x| x.dir == Dir::Out);
                let last_in = pins.iter().rposition(|x| x.dir == Dir::In);
                if let (Some(fo), Some(li)) = (first_out, last_in) {
                    assert!(li < fo, "{def} 的输入/输出引脚未连续分组");
                }
            }
        }
    }
}
