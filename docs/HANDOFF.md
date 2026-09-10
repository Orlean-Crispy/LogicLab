# LogicLab 交接摘要

> 项目根：`C:\Users\Orlean-Crispy\Desktop\LogicLab`
> 规格书：**v4**（含 28 条 ADR）
> 版本留档：`releases/{X86,ARM64}/`，命名 `LogicLab-<版本>-<架构>.exe`
> 远 端：https://github.com/Orlean-Crispy/LogicLab

## 1. 当前状态：0.2.0 Checkpoint 2

> 版本命名说明：项目整体处于测试期，不用 alpha/beta 这类希腊字母后缀，
> 改用 **Checkpoint**（检查点）——每次攒够一批功能就封一个检查点，语义中性、可无限延续。

| 部分 | 状态 |
|---|---|
| `core`（纯 Rust，零引擎依赖） | **99** 个单测全绿 |
| `cli`（headless 命令行） | components / schema / demo / example / info / run / bench |
| `bridge`（gdext 薄桥） | 基于 Session，x64 + arm64 |
| Godot 壳 | GUI 与 headless 均启动无错 |
| 回归 `godot/tests/smoke.gd` | **119** 项全通过 |

**沙盒功能已具备**：放置/拖动/旋转/删除/**撤销重做**/连线（自动避让）/开关/参数编辑/
文本注释/**网络标签**/平移缩放/单步运行调速/存取/DRC 与双击定位/关键路径/悬停探针/
**多选与框选**/**层次封装（子电路）**/**图纸库与面包屑导航**/**波形查看 + VCD 导出**/
**数码管与内存映射点阵屏**/**中英双语界面（顶部一键切换）**。

## 2. 架构（三层 + 门面）

```
Godot 壳（GDScript）    交互 / 自绘渲染 / 面板（不硬编码组件知识）
      ↓ 调方法、拿扁平数组
bridge（gdext，薄）     命令翻译 + CircuitView → PackedInt64Array
      ↓
core
  ├─ session    编辑与仿真的统一门面 ← 编辑即重置（ADR-23）在这里保证
  ├─ board      数据模型 / 网表推导 / 自动走线 / 命中测试
  ├─ elaborate  层次展开：多图纸 → 一张扁平网表（ADR-28）
  ├─ engine     双缓冲 tick + 脏传播（只认 Flat）
  ├─ defs       组件知识唯一数据源（含参数描述）
  ├─ view       壳无关呈现数据
  ├─ save       工程 JSON
  └─ drc        设计规则检查（全工程 + 循环引用）
      ↓
cli（headless）         同一套 core 的非 Godot 消费者
```

**铁律**：core 依赖树里没有任何引擎类型。

## 3. 层次模型（ADR-28）怎么工作的

- 一张图纸就是一个**子电路定义**；图纸里的 `input_pin` / `output_pin` 是它的接口。
- 接口引脚按**元件名排序**，同名按实例索引——顺序稳定、可预测。
- `Session::extract_to_sub` 是 TC 那种"选中一片→封装成元件"：跨边界的连线自动
  转成接口，外部连线自动重新接回。
- 仿真前 `elaborate::build` 把整个层次树压成 `Flat`：外壳不产生运行时组件，
  外壳引脚与接口引脚并成同一个网络。**热路径完全不知道层次存在**。
- 同一张图纸实例化多次就展开多份，互不共享状态；自引用/超深（>32）被截断并置
  `truncated`，绝不允许递归爆栈。

## 4. 验证手段（最省事的顺序）

```powershell
cargo test                                     # core + cli，不需要 Godot
cargo run --release -p logiclab-cli -- bench    # 万级规模基准

# 全链路（headless 会加载 GDExtension，最快）
& 'C:\Users\Orlean-Crispy\Desktop\Godot_v4.7.2-stable.exe' --headless --path godot --script res://tests/smoke.gd

# 真渲染（_draw 只有 GUI 模式会跑）
& 'C:\Users\Orlean-Crispy\Desktop\Godot_v4.7.2-stable.exe' --path godot --quit-after 200
```

单个脚本语法检查用 `godot_validate_script` 工具——它能看到真错误，
而运行时只会含糊地报 "Could not resolve script"。

## 5. 实测（release）

| 指标 | 数值 | 规格 |
|---|---|---|
| 10001 组件每拍仿真 | ~0.41 ms | < 5 ms ✅ |
| 万级电路单次编辑（含展开） | ~13.5 ms | 一帧内 ✅ |
| 撤销一步 | ~15.8 ms | — |
| 单文件体积 | x64 43.7 MB / arm64 35.0 MB | — |

## 6. 关键坑（避免重复踩）

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

## 7. 还没做的（按价值排序）

1. **测试台**（CSV + 随机模式 + 层级断言，§9.3）—— 有了波形，这是下一个自然缺口
2. **Verilog 导出**（§9.2，DRC 与关键路径都已就位）
3. 仿真 checkpoint 与回溯（§9.7）
4. 复制粘贴 / 对齐吸附 / 多选参数批量编辑
5. 虚拟外设补全：键盘输入、七段译码表、UART 终端（§6.6）
6. 汇编器 / ISA / CPU 教学层（§11）

## 8. 旁支

- 原版 TC 脚本在 `C:\Users\Orlean-Crispy\Desktop\TC_scripts`（仅参考机制，
  **不得取其代码 / 素材 / 文案**）。
- **GPL 提醒**：hneemann/Digital 与 logisim-evolution 均为 GPLv3，
  只能看思路、重新实现；复制代码会让本项目传染为 GPL（v4 §17）。
- **本项目的 MIT 与上面这条红线是绑在一起的**：MIT 只对「自己写的代码」成立。
  一旦真把 GPL 代码或素材抄进来，MIT 就站不住了——要么把那段整个移除并重新实现，
  要么把项目整体改成 GPLv3。所以「只借鉴机制、绝不复制实现」不是洁癖，是许可证前提。
