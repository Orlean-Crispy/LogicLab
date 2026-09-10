extends RefCounted
## 界面双语。
##
## 设计取舍：**中文原文就是 key**，英文走这张表。
## 好处是新增文案时不必同时维护两份语言文件——漏翻只会退回中文，而不是显示一个 key。
## 代价是中英文字符串必须逐字一致，改中文就等于改 key（改错了也只是退回中文，不会崩）。
##
## 两类用法：
##   - 静态 UI：apply(node) 遍历控件树替换，原文存在 meta 里所以可以反复切换
##   - 动态文本（状态栏、提示、对话框标题）：在发射处包一层 t()

const ZH := 0
const EN := 1

static var lang := ZH


static func is_en() -> bool:
	return lang == EN


static func toggle() -> int:
	lang = EN if lang == ZH else ZH
	return lang


## 动态文本用这个
static func t(zh: String) -> String:
	if lang == ZH:
		return zh
	return String(EN_TABLE.get(zh, zh))


## 静态 UI：把一棵控件树里所有可见文本换成当前语言。
## 原文缓存在 meta 里，所以中英来回切都能还原（前提是代码没有在别处直接改 text）。
static func apply(root: Node) -> void:
	if root == null:
		return
	_apply_node(root)
	for c in root.get_children():
		apply(c)


static func _apply_node(n: Node) -> void:
	if n is Label or n is Button or n is CheckBox:
		_swap(n, "text")
	elif n is OptionButton:
		var ob := n as OptionButton
		for i in ob.item_count:
			ob.set_item_text(i, t(ob.get_item_text(i)))
	elif n is AcceptDialog:
		_swap(n, "title")
	elif n is LineEdit:
		var le := n as LineEdit
		if le.placeholder_text != "":
			_swap(le, "placeholder_text")


## 不能叫 _set：那是 Object 的内建虚函数，签名对不上会直接报错
static func _swap(n: Node, prop: String) -> void:
	var cur := String(n.get(prop))
	if not n.has_meta("zh"):
		n.set_meta("zh", cur)
	n.set(prop, t(String(n.get_meta("zh"))))


const EN_TABLE := {
	# --- 画布状态与提示 ---
	"tick %d · 元件 %d · 导线 %d · 网络 %d · 本拍求值 %d": "tick %d · %d components · %d wires · %d nets · %d evaluated",
	"X（未定义/冲突）": "X (undefined/conflict)",
	"撤销": "Undo",
	"重做": "Redo",
	"存档损坏，未载入": "Save is corrupt, not loaded",
	"点击画布放置：%s（Esc 取消）": "Click the canvas to place: %s (Esc to cancel)",
	"返回上一层": "Went up one level",
	"仿真已复位": "Simulation reset",
	"封装失败": "Encapsulation failed",
	"该导线没有网络": "That wire has no net",
	"该元件不是子电路": "That component is not a sub-circuit",
	"进入子电路": "Entered sub-circuit",
	"开关翻转": "Switch toggled",
	"没有可%s的操作": "Nothing to %s",
	"没有找到存档": "No save file found",
	"确定": "OK",
	"实例名已改为 %s": "Instance renamed to %s",
	"鼠标下没有导线": "No wire under the cursor",
	"网络标签已更新": "Net label updated",
	"网络名（同名即相连）": "Net name (same name = connected)",
	"未连接": "unconnected",
	"先选中要封装的元件": "Select components to encapsulate first",
	"先选中一个子电路实例": "Select a sub-circuit instance first",
	"悬停探针": "Hover probe",
	"选中 %d 个元件": "%d components selected",
	"已%s": "%s",
	"已保存到 user://logiclab_project.json": "Saved to user://logiclab_project.json",
	"已插入注释，按 Enter 编辑": "Annotation inserted, press Enter to edit",
	"已定位到 %s": "Located %s",
	"已放置": "Placed",
	"已封装为子电路：%s": "Encapsulated as sub-circuit: %s",
	"已观察 %s": "Watching %s",
	"已连线": "Connected",
	"已取消观察 %s": "Stopped watching %s",
	"已删除 %d 个元件": "Deleted %d components",
	"已新建空工程": "New empty project",
	"已旋转 90°": "Rotated 90°",
	"已在根图纸": "Already at the root drawing",
	"已载入存档": "Save loaded",
	"已载入示例：%s": "Example loaded: %s",
	"引脚值 %s": "Pin value %s",
	"载入示例失败": "Failed to load example",
	"主选中": "primary",
	"注释": "Annotation",
	"注释内容": "Annotation text",
	"注释已更新": "Annotation updated",
	"子电路": "Sub-circuit",
	"走线失败：找不到不误连的路径": "Routing failed: no path free of mis-connections",
	# --- 层次导航条 ---
	"＋ 新建": "+ New",
	"波形": "Waveform",
	"第 %d 层 · 共 %d 张图纸 · 本图纸被引用 %d 次": "Level %d · %d drawings · referenced %d times",
	"封装为子电路": "Encapsulate as sub-circuit",
	"封装选区": "Encapsulate selection",
	"改名": "Rename",
	"删除": "Delete",
	"新建子电路": "New sub-circuit",
	"重命名图纸": "Rename drawing",
	# --- 侧栏与底部条 ---
	"（没有发现问题）": "(no issues found)",
	"（选择示例…）": "(choose an example…)",
	"\n关键路径：%d 级（含 %d 个组件）": "\nCritical path: %d levels (%d components)",
	"… 另有 %d 条未显示": "… %d more not shown",
	"保存": "Save",
	"本图纸 %d 错 · %d 警%s": "This drawing: %d errors · %d warnings%s",
	"  ·  全工程 %d 错 %d 警（%d 张图纸）": "  ·  project-wide %d errors %d warnings (%d drawings)",
	"撤销 ^Z": "Undo ^Z",
	"重做 ^Y": "Redo ^Y",
	"错误 %d · 警告 %d\n\n": "%d errors · %d warnings\n\n",
	"单步": "Step",
	"复位": "Reset",
	"该组件没有可调参数": "This component has no editable parameters",
	"关闭": "Close",
	"就绪": "Ready",
	"没有发现问题。": "No issues found.",
	"删除 (Del)": "Delete (Del)",
	"设计规则检查": "Design rule check",
	"设计规则检查（双击条目定位）": "Design rule check (double-click to locate)",
	"实例名": "Instance name",
	"示例电路": "Example circuits",
	"未选中元件": "No component selected",
	"新建": "New",
	"旋转 (R)": "Rotate (R)",
	"选中元件": "Selected component",
	"元件库（点选后在画布上放置）": "Library (click, then place on canvas)",
	"运行": "Run",
	"运行检查": "Run check",
	"载入": "Load",
	"暂停": "Pause",
	# --- 波形面板 ---
	"%s  [%d 位]": "%s  [%d bits]",
	"波形：等待采样（运行几拍）": "Waveform: waiting for samples (run a few ticks)",
	"波形：鼠标停在导线上按 W 观察该网络": "Waveform: hover a wire and press W to watch that net",
	# --- 启动日志 ---
	"LogicLab %s %s 就绪：元件 %d 种 · 示例 %d 个": "LogicLab %s %s ready: %d components · %d examples",
}
