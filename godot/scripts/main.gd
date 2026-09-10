extends Control
## 装配 core（Rust）+ 画布 + 面板，并把它们的信号接起来。
## 所有电路状态都在 core 里，这里只做转发。

const CircuitCanvasScript := preload("res://scripts/CircuitCanvas.gd")
const UiPanelScript := preload("res://scripts/ui_panel.gd")

var core                    # LogicLab（GDExtension 类）
var canvas                  # CircuitCanvas
var panel                   # UiPanel

func _ready() -> void:
	core = LogicLab.new()

	canvas = CircuitCanvasScript.new()
	canvas.core = core
	canvas.name = "CircuitCanvas"
	canvas.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	add_child(canvas)

	panel = UiPanelScript.new()
	panel.core = core
	panel.name = "UiPanel"
	add_child(panel)

	# 面板 → 画布
	panel.library_selected.connect(canvas.set_pending_def)
	panel.tick_pressed.connect(canvas.do_tick)
	panel.run_toggled.connect(canvas.set_running)
	panel.reset_pressed.connect(canvas.do_reset)
	panel.delete_pressed.connect(canvas.delete_selected)
	panel.rotate_pressed.connect(canvas.rotate_selected)
	panel.param_changed.connect(canvas.set_selected_param)
	panel.example_selected.connect(canvas.load_example)
	panel.tick_rate_changed.connect(canvas.set_tick_rate)
	panel.locate_requested.connect(canvas.locate_component)
	panel.display_name_changed.connect(canvas.set_display_name_of)
	panel.save_pressed.connect(canvas.save_project)
	panel.load_pressed.connect(canvas.load_project)
	panel.clear_pressed.connect(canvas.clear_project)

	# 画布 → 面板
	canvas.selection_changed.connect(panel.on_selection_changed)
	canvas.status_changed.connect(panel.set_status)
	canvas.stats_changed.connect(panel.set_stats)

	panel.build_library(core)
	canvas.refresh()
	canvas.center_on_content()

	# 先给个能直接看到东西的起点：载入半加器示例
	canvas.load_example("half_adder")

	get_window().title = "LogicLab 0.1.0 Beta 3"
	print("LogicLab 0.1.0 Beta 3 就绪：元件库 ", core.library_ids().size(), " 种")
