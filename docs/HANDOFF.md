# LogicLab 交接摘要

> 项目根：`C:\Users\Orlean-Crispy\Desktop\LogicLab`
> 规格书：`logic-sandbox-spec-v3.md`（自包含，含 22 条 ADR）
> 版本留档：`releases/{X86,ARM64}/`，命名 `LogicLab-<版本>-<架构>.exe`

## 1. 目标与硬约束

自己的逻辑电路沙盒（对标《图灵完备》沙盒模式，但更专业）。
- **唯一硬指标：性能**；**必须能在 ARM64 Windows 上运行**。
- 桌面窗口应用，游戏式启动；半游戏半专业定位。

## 2. 当前状态：0.1.0 Beta 2

| 部分 | 状态 |
|---|---|
| `core`（纯 Rust，零引擎依赖） | ✅ 68 个单测全绿 |
| `cli`（headless 命令行） | ✅ components / demo / example / info / run / bench |
| `bridge`（gdext 薄桥） | ✅ x64 + arm64 release 均编译通过 |
| Godot 壳（main / CircuitCanvas / ui_panel） | ✅ GUI 与 headless 均启动无错 |
| 回归测试 `godot/tests/smoke.gd` | ✅ 36 项全通过 |

## 3. 验证手段（最省事的顺序）

```powershell
# 1) core + cli，完全不需要 Godot
cargo test
cargo run --release -p logiclab-cli -- bench

# 2) 全链路（headless 会加载 GDExtension，所以这条最快）
& 'C:\Users\Orlean-Crispy\Desktop\Godot_v4.7.2-stable.exe' --headless --path godot --script res://tests/smoke.gd

# 3) 真渲染（_draw 只有 GUI 模式会跑）
& 'C:\Users\Orlean-Crispy\Desktop\Godot_v4.7.2-stable.exe' --path godot --quit-after 200
```

## 4. 架构

```
Godot 壳（GDScript）    交互 / 自绘渲染 / 面板（不硬编码任何组件知识）
      ↓ 调方法、拿扁平数组
bridge（gdext，薄）     命令翻译 + CircuitView → PackedInt64Array
      ↓ 纯 Rust 调用
core（零引擎依赖）      数据模型 / tick 引擎 / 网表 / 序列化 / 视图 / DRC / 示例
      ↓
cli（headless）         同一套 core 的非 Godot 消费者
```

### 三条性能架构法则（新增代码必须过检）

1. **零 per-component 节点渲染**：单个自绘 Canvas，批量 `draw_*`。
2. **增量同步，绝不整帧全量快照**：值走 `take_changed()`，几何只在 revision 变化时重建。
3. **编辑以命令流入 core**，渲染只读状态。

另：core 热路径零分配；逻辑层禁浮点（ADR-22）。

## 5. 实测（release）

| 指标 | 数值 | 规格 |
|---|---|---|
| 10001 组件每拍仿真 | **0.227 ms** | < 5 ms ✅ |
| 万级电路装载（含网表推导） | 3.4 ms | — |
| 单文件体积 | x64 43.7 MB / arm64 34.9 MB | — |

## 6. core 模块地图

| 文件 | 职责 |
|---|---|
| `values.rs` | 位宽 {1,4,8,16,32}、位级 X、wrap 算术 |
| `defs.rs` | **组件知识唯一数据源**：DefId / Params / PinDef / eval / **参数描述** |
| `board.rs` | Board / 网表推导（区间合并索引）/ 自动走线（网格 Dijkstra）/ 命中测试 |
| `engine.rs` | 双缓冲 tick、脏传播、CSR 邻接 |
| `view.rs` | 壳无关的 CircuitView（换壳只重写渲染器） |
| `save.rs` | 工程 JSON + schema 版本 |
| `drc.rs` | 多驱动 / 位宽 / 悬空 / 组合环 |
| `examples.rs` | 5 个内置示例电路 |

## 7. 关键坑（避免重复踩）

- **本机 `pwsh` 工具实为 Windows PowerShell 5.1**：`Set-Content` 默认 ANSI，
  **绝不用它做文本编辑**。只用 read / write / edit 工具。
- **TS 模板字符串里的反引号会终止字符串**：代码注释里别用反引号。
- 删除文件后 write 工具会拒绝；先写临时文件再 `Move-Item`。
- **GDScript 从 GDExtension 取值是 Variant，不能用 `:=` 推断类型**。
- **GDExtension 类（RefCounted）不是 Node**，GDScript 参数别标注 `: Node`。
- 跨脚本引用用 `preload`，别依赖全局 `class_name`。
- `pack.ps1` 从 `godot/build/` 取导出产物；launcher 在 `target/`（已并入 workspace）。
- `godot_set_project_setting` 会被沙箱拒绝（项目不在 workspace 内），改 `project.godot` 请用 edit 工具。
- **不要用 `godot_run_project`**：它会往项目里装 autoload，污染 `project.godot`。
  改用 pwsh 直接启动 Godot（当前是 danger-full-access，无沙箱拦截）。
- 模型无图像输入：**不要依赖截图**，用 smoke 测试或打印数值验证。

## 8. 待办（按优先级）

1. **撤销 / 重做**（ADR-9：core 内命令模式，UI 只发命令）
2. 交互补齐：右键菜单（删除元件 / 剪断导线）、框选、复制粘贴
3. 封装（Board → 组件，含自引用环检测）、Net Label
4. 波形查看器 + VCD 导出、测试台
5. Verilog 导出（**必须先过 DRC**，已就位）
6. 振荡检测（§5.1 要求 UI 警告）
7. 体积优化（design.md §17.2：引擎裁剪 ~25MB 或换纯 Rust 壳 ~5-10MB）

## 9. 旁支

- 原版 TC 脚本已反编译到 `C:\Users\Orlean-Crispy\Desktop\TC_scripts`
  （仅参考机制，**不得取其代码 / 素材 / 文案**，规格 §17 红线）。
  借鉴结论见 design.md §17.4。
