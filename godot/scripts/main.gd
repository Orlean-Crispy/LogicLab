extends Control
## 装配 core（Rust）+ 画布 + 面板，并把它们的信号接起来。
## 所有电路状态都在 core 里，这里只做转发。

const CircuitCanvasScript := preload("res://scripts/CircuitCanvas.gd")
const UiPanelScript := preload("res://scripts/ui_panel.gd")
const HierPanelScript := preload("res://scripts/hier_panel.gd")
const WavePanelScript := preload("res://scripts/wave_panel.gd")

const VERSION := "0.1.0"

var core                    # LogicLab（GDExtension 类）
var canvas                  # CircuitCanvas
var panel                   # UiPanel
var hier                    # HierPanel
var wave                    # WavePanel


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

	hier = HierPanelScript.new()
	hier.core = core
	hier.name = "HierPanel"
	add_child(hier)

	wave = WavePanelScript.new()
	wave.core = core
	wave.name = "WavePanel"
	wave.visible = false
	wave.set_anchors_and_offsets_preset(Control.PRESET_BOTTOM_WIDE)
	wave.offset_top = -180
	wave.offset_left = 8
	wave.offset_right = -8
	wave.offset_bottom = -8
	wave.mouse_filter = Control.MOUSE_FILTER_STOP
	add_child(wave)

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
	panel.undo_pressed.connect(canvas.do_undo)
	panel.redo_pressed.connect(canvas.do_redo)
	panel.display_name_changed.connect(canvas.set_display_name_of)
	panel.save_pressed.connect(canvas.save_project)
	panel.load_pressed.connect(canvas.load_project)
	panel.clear_pressed.connect(canvas.clear_project)

	# 层次条 → 画布
	hier.level_requested.connect(canvas.goto_level)
	hier.open_requested.connect(canvas.open_board_index)
	hier.extract_requested.connect(_on_extract)
	hier.wave_toggled.connect(_on_wave_toggled)

	# 画布 → 面板
	canvas.selection_changed.connect(panel.on_selection_changed)
	canvas.status_changed.connect(panel.set_status)
	canvas.stats_changed.connect(panel.set_stats)
	canvas.wave_changed.connect(_on_wave_changed)

	panel.build_library(core)
	canvas.refresh()
	canvas.center_on_content()
	hier.refresh()

	# 起点用层次示例：双击半加器实例就能进去看它内部怎么搭的
	canvas.load_example("full_adder")
	hier.refresh()

	get_window().title = "LogicLab %s" % VERSION
	print("LogicLab %s 就绪：元件 %d 种 · 示例 %d 个" % [VERSION, core.library_ids().size(), core.example_ids().size()])


func _on_extract(cname: String) -> void:
	if canvas.extract_selection(cname) >= 0:
		hier.refresh()


func _on_wave_toggled(on: bool) -> void:
	wave.visible = on
	if on:
		wave.queue_redraw()


func _on_wave_changed() -> void:
	if wave.visible:
		wave.queue_redraw()