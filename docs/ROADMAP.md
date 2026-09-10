# LogicLab 路线图

> 依据：规格书 v4（本目录 `logic-sandbox-spec-v4.md`）
> 长期目标：推进到满足规格 **§15 全部七条验收标准**，路线遵循 **§13 里程碑**
> 当前版本：**0.2.0 Checkpoint 1**

## 里程碑进度

| 里程碑 | 范围 | 状态 |
|---|---|---|
| **M0** 纯 Rust 核心 | 数据模型 / tick 引擎 / 网表推导 / JSON 往返 / undo / 基础组件 / cli | ✅ 完成 |
| **M1** Godot 最小壳 | GDExtension / 编辑交互 / 自绘渲染 / 调速 / 注释 | ✅ 完成 |
| **M2** 完整编辑器 + 全组件库 | §6 全组件 / 封装 / net label / 多 Board / DRC / 万元件性能 | ✅ 完成 |
| **M3** 专业四件套 + checkpoint | 波形 + VCD / 测试台 / CLI headless 回归 / 关键路径 / checkpoint | 🚧 进行中 |
| **M4** Verilog 导出 | 结构化导出 + 单时钟约束 + 外设 black-box + iverilog/Yosys 验收 | ⬜ 未开始 |
| **M5** 造机与外设 | Keyboard / UART / 汇编器 + ISA JSON / ROM 烧录 / 参考 CPU 教程 | 🚧 部分 |
| **M6+** | P1/P2 按需排期（FSM 编辑器、三态总线、SVG、i18n、Yosys 报告…） | ⬜ |

## M3 明细（当前主战场）

| 项 | 规格要求 | 现状 |
|---|---|---|
| 波形记录 | 环形缓冲，默认 10000 tick/通道，可配 | ⚠️ 现为 4096 上限后**停止**记录，需改环形 |
| 波形 UI | 多通道 / **双游标量延迟** / 缩放 / 跳转边沿 | ⚠️ 有多通道与方波绘制，缺双游标与跳转 |
| 多 bit 显示 | 总线按 **hex 值**显示（游标处读数） | ⚠️ 现为阶梯高度，缺数值读数 |
| VCD 导出 | 标准格式，1 tick = 1 ns，**信号名用层级路径**（ADR-25） | ⚠️ 格式已对，名字仍是 `net<id>` |
| 测试台 | CSV 向量 + 约束随机 + **种子显式记录** + 顶层 pin/层级 Net 断言 + 失配定位 | ⬜ 未开始 |
| CLI 回归 | headless 跑全工程测试向量，人可读文本 + JSON 报告 | ⬜ 未开始 |
| 关键路径 | 最长组合路径（按 tick 计延迟）+ 路径组件序列 | ✅ 已完成 |
| checkpoint | 全组件状态 + tick 快照，恢复后与连续运行逐 tick 一致 | ⬜ 未开始 |

**M3 退出标准**：给一个时序 bug 电路，用双游标量出延迟差并定位；
CLI 跑全工程测试向量出 JSON；checkpoint 恢复后与连续运行的波形逐 tick 一致。

## M4 要点（Verilog 导出）

- 用户封装组件 → Verilog module（递归展开或保持层次，可选）
- 组合逻辑 → `assign`；时序元件 → `always @(posedge clk)`；RAM → reg 数组 + `initial $readmemh(...)`
- 实例名取 `display_name` 并做标识符 sanitize（ADR-27）
- **单时钟约束**（ADR-20）：仅允许一个 Clock 实例，否则 DRC 报错阻止导出；Clock 映射为顶层输入端口 `clk`
- **虚拟外设**（ADR-24）：Display / Keyboard / UART / 数码管 映射为顶层 black-box 端口模块，
  注释标明"此处接真机引脚或替换为你的外设模块"，**不静默丢弃**
- 导出注释必须写明：游戏内 1 tick 门延迟是可视化手段，导出件是常规零延迟组合 + 时钟沿同步，
  **周期级等价而非 tick 级等价**
- **验收线**：iverilog 仿真通过 + Yosys 综合通过，且与 core 仿真周期等价

## M5 要点（造机玩法）

- 外设：Keyboard、UART 终端、七段译码表
- ISA 定义（工程内 JSON）+ 汇编语法 + 独立 CLI（可脚本化灌 ROM）
- ROM 一键烧录（hex/bin 导入导出）
- 参考 CPU 骨架 + 教程文档（**非关卡**，沙盒内的可选深水区）
- **验收线**：自建 8 位 CPU + UART 跑通一段含条件跳转的真实程序

## §15 最终验收标准（长期目标的判定线）

1. 用它搭一个 8 位 CPU + UART，跑通一段含条件跳转的真实程序
2. 任意电路导出 Verilog：**iverilog 仿真 + Yosys 综合双双通过**（游戏与工具的分水岭）
3. 用户能自建测试向量做回归，失配报告能定位到 Net 和 tick
4. 出 bug 时能用波形查看器量出"哪条 Net 哪个 tick 出的问题"
5. 万级元件工程仿真每 tick < 5 ms
6. `cargo check -p core -p cli` 在无 Godot 环境编译通过 —— core 可独立存活
7. checkpoint 恢复后行为与连续运行逐 tick 一致

## 贯穿全程的硬约束

- **core 依赖树零引擎类型**（ADR-21）：换壳时 core 一行不改
- **性能**：万级元件 < 5 ms/tick；编辑保持在一帧内
- **编辑即重置**（ADR-23）：所有编辑统一经 `Session::edit()`，撤销与重置不可能漏
- **确定性不变量**（§14）：同电路同输入逐 tick 可复现；打乱实例顺序结果不变；
  JSON round-trip 逐字节一致；全量 undo 回到初始态序列化相等
- **DRC 必须先于 Verilog 导出**：可综合约束要早一天定进数据模型
- **版权红线**（§17）：TC 脚本、hneemann/Digital、logisim-evolution 只能看思路，不得取代码 / 素材 / 文案
