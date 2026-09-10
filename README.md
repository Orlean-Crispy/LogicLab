# LogicLab

一个数字电路沙盒：对标《图灵完备（Turing Complete）》的沙盒模式，但按「半游戏半专业工具」的定位做得更专业一些。

- **纯 Rust 内核** + Godot 4 外壳（GDExtension 桥接）
- **tick 制离散仿真**：所有组件输出一律延迟 1 tick，信号沿导线逐拍传播看得见
- **为大体量设计而做**：一万个元件每拍仿真约 0.2 ms，脏标记让静默电路零求值
- **x64 与 ARM64 双架构**，单文件免安装
- 附带一个 headless CLI，**不依赖 Godot** 也能跑仿真

## 快速开始

### 直接运行

到 [Releases](../../releases) 下载对应架构的单文件 exe，双击即可。

### 从源码构建

前置：Rust 稳定版工具链、Godot 4.7.x、MSVC（Windows）。

```bash
# 1) 内核与命令行（完全不需要 Godot）
cargo test
cargo run --release -p logiclab-cli -- bench      # 万级规模性能基准
cargo run -p logiclab-cli -- demo                 # 半加器真值表 + 移位寄存器波形
cargo run -p logiclab-cli -- example out.json     # 生成示例工程
cargo run -p logiclab-cli -- components           # 列出全部内置组件

# 2) GDExtension 桥
cd bridge
cargo build --release --target x86_64-pc-windows-msvc
cargo build --release --target aarch64-pc-windows-msvc
# 把产物复制到 godot/ 下：
#   x86_64  -> godot/logiclab_bridge.dll
#   aarch64 -> godot/logiclab_bridge_arm64.dll

# 3) 运行
godot --path godot
```

### 回归测试

```bash
# headless 也会加载 GDExtension，所以这是最快的全链路验证
godot --headless --path godot --script res://tests/smoke.gd
```

## 操作

| 操作 | 方式 |
|---|---|
| 载入示例 | 左侧「示例电路」下拉 |
| 放置元件 | 左侧库中点击，再点画布 |
| 连线 | 从一个引脚拖到另一个引脚（自动避让，不会误连） |
| 选中 / 拖动 | 左键点元件 / 按住拖动 |
| 开关翻转 | 单击开关元件 |
| 旋转 / 删除 | 选中后按 R / Delete |
| 平移 / 缩放 / 居中 | 右键或中键拖动 / 滚轮 / C |
| 单步 / 运行 | 空格 / 底部按钮（右侧滑块调速） |
| 改参数 | 选中元件后在右侧面板改 |
| 设计规则检查 | 左侧按钮（多驱动 / 位宽 / 悬空 / 组合环） |

## 架构

```
Godot 壳（GDScript）    交互 / 自绘渲染 / 面板 —— 不硬编码任何组件知识
      ↓ 调方法、拿扁平数组
bridge（gdext，薄）     命令翻译 + CircuitView → PackedInt64Array
      ↓ 纯 Rust 调用
core（零引擎依赖）      数据模型 / tick 引擎 / 网表推导 / 序列化 / 视图 / DRC / 示例
      ↓
cli（headless）         同一套 core 的非 Godot 消费者
```

**铁律：core 的依赖树里没有任何引擎类型。** 所以 `cargo test` 不需要 Godot，
将来换壳（或干脆去掉 Godot）时 core 一行不改。

### core 模块

| 文件 | 职责 |
|---|---|
| `values.rs` | 位宽 {1,4,8,16,32}、位级未知标记、wrap 算术 |
| `defs.rs` | **组件知识的唯一数据源**：种类 / 参数 / 引脚 / 求值 / 可编辑参数描述 |
| `board.rs` | 编辑期模型、网表推导（区间合并索引）、自动走线（网格 Dijkstra）、命中测试 |
| `engine.rs` | 双缓冲 tick、脏传播、CSR 邻接 |
| `view.rs` | 壳无关的 `CircuitView`（换壳只需重写一个渲染器） |
| `save.rs` | 工程 JSON + schema 版本（core 不碰文件系统） |
| `drc.rs` | 设计规则检查 |
| `examples.rs` | 内置示例电路 |

### 三条性能法则（新增代码都要过这三条）

1. **零 per-component 节点渲染**：单个自绘画布，批量 `draw_*`。
2. **增量同步**：值走「变化的网络」增量通道，几何只在结构变化时重建。
3. **编辑以命令流入 core**，渲染只读状态。

## 仿真语义

- 所有组件输出**一律延迟 1 tick**（含触发器：时钟沿在 t 被读到，输出在 t+1 出现）
- 导线零延迟、同一网络同值
- 两阶段更新：先全部求值到暂存区，再原子提交，结果与遍历顺序无关
- 未连接的输入读作 0
- 算术一律 wrap 回绕，逻辑层全整数、不出现浮点

注意：**组合反馈（如 SR 锁存器）能仿真但会被 DRC 报错**。反馈本身合法，只要环上有时序元件；
纯组合环对应真实硬件里的振荡，也是 Verilog 导出的硬伤，所以按错误处理。

## 性能

| 指标 | 数值 |
|---|---|
| 10001 个组件，每拍全部活动 | ~0.23 ms/拍 |
| 万级电路装载（含网表推导） | 3.4 ms |
| 静默电路每拍求值组件数 | 0 |

## 许可

尚未指定许可证。在加上 LICENSE 之前，默认保留所有权利。
