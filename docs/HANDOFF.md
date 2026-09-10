# LogicLab 交接摘要

> 项目根：`C:\Users\Orlean-Crispy\Desktop\LogicLab`
> 规格书：**v4**（含 28 条 ADR）
> 版本留档：`releases/{X86,ARM64}/`，命名 `LogicLab-<版本>-<架构>.exe`
> 远 端：https://github.com/Orlean-Crispy/LogicLab

## 1. 当前状态：0.1.0 Beta 3

| 部分 | 状态 |
|---|---|
| `core`（纯 Rust，零引擎依赖） | 73 个单测全绿 |
| `cli`（headless 命令行） | components / demo / example / info / run / bench |
| `bridge`（gdext 薄桥，598 行） | 基于 Session，x64 + arm64 |
| Godot 壳 | GUI 与 headless 均启动无错 |
| 回归 `godot/tests/smoke.gd` | 52 项全通过 |

## 2. 架构（三层 + 门面）

```
Godot 壳（GDScript）    交互 / 自绘渲染 / 面板（不硬编码组件知识）
      ↓ 调方法、拿扁平数组
bridge（gdext，薄）     命令翻译 + CircuitView → PackedInt64Array
      ↓
core
  ├─ session  编辑与仿真的统一门面 ← 编辑即重置（ADR-23）在这里保证
  ├─ board    数据模型 / 网表推导 / 自动走线 / 命中测试
  ├─ engine   双缓冲 tick + 脏传播
  ├─ defs     组件知识唯一数据源（含参数描述）
  ├─ view     壳无关呈现数据
  ├─ save     工程 JSON
  └─ drc      设计规则检查
      ↓
cli（headless）         同一套 core 的非 Godot 消费者
```

**铁律**：core 依赖树里没有任何引擎类型。

## 3. 验证手段（最省事的顺序）

```powershell
cargo test                                    # core + cli，不需要 Godot
cargo run --release -p logiclab-cli -- bench   # 万级规模基准

# 全链路（headless 会加载 GDExtension，最快）
& 'C:\Users\Orlean-Crispy\Desktop\Godot_v4.7.2-stable.exe' --headless --path godot --script res://tests/smoke.gd

# 真渲染（_draw 只有 GUI 模式会跑）
& 'C:\Users\Orlean-Crispy\Desktop\Godot_v4.7.2-stable.exe' --path godot --quit-after 200
```

单个脚本语法检查用 `godot_validate_script` 工具——它能看到真错误，
而运行时只会含糊地报 "Could not resolve script"。

## 4. 实测（release）

| 指标 | 数值 | 规格 |
|---|---|---|
| 10001 组件每拍仿真 | ~0.23 ms | < 5 ms ✅ |
| 万级电路装载（含网表推导） | 3.4 ms | — |
| 单文件体积 | x64 43.7 MB / arm64 34.9 MB | — |

## 5. 关键坑（避免重复踩）

- **GDScript 只认 `#` 注释**：写 `//` 或 `///` 会让整个脚本解析失败，
  而运行时只报 "Could not resolve script"，必须用校验工具才看得到真错误。
- **本机 `pwsh` 工具实为 Windows PowerShell 5.1**：`Set-Content` 默认 ANSI，
  **绝不用它做文本编辑**；它也不认 `\n`（要用 `[char]10`），反引号才是它的转义符。
- **TS 模板字符串里的反引号会终止字符串**：注释里别用反引号。
- **GDScript 从 GDExtension 取值是 Variant，不能用 `:=` 推断类型**。
- **GDExtension 类（RefCounted）不是 Node**，GDScript 参数别标注 `: Node`。
- 跨脚本引用用 `preload`，别依赖全局 `class_name`。
- 删除文件后 write 工具会拒绝；先写临时文件再 `Move-Item`。
- `pack.ps1` 从 `godot/build/` 取导出产物；launcher 在 workspace 的 `target/`。
- **不要用 `godot_run_project`**：它会往项目里装 autoload 污染 `project.godot`。
- 模型无图像输入：**不要依赖截图**，用 smoke 测试或打印数值验证。

## 6. 待办（按优先级）

1. **撤销/重做**（ADR-9，命令栈挂 Session；编辑体验最大短板）
2. 封装（Board → 组件，含自引用环检测、涌现延迟 ADR-28）
3. Net Label、多 Board
4. 波形查看器 + VCD（层级命名）、测试台、CLI JSON 报告
5. 仿真 checkpoint（v4 §9.7）
6. Verilog 导出（DRC 已就位，正好接上）
7. 振荡检测（§5.1 要求 UI 警告）

## 7. 旁支

- 原版 TC 脚本在 `C:\Users\Orlean-Crispy\Desktop\TC_scripts`（仅参考机制，
  **不得取其代码 / 素材 / 文案**）。
- **GPL 提醒**：hneemann/Digital 与 logisim-evolution 均为 GPLv3，
  只能看思路、重新实现；复制代码会让本项目传染为 GPL（v4 §17）。
