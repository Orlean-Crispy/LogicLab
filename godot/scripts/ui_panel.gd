class_name UiPanel
extends CanvasLayer
## 侧边元件库 + 示例 + 底部运行控制 + 右侧属性面板 + DRC 检查。
##
## 元件表与参数控件都**由 core 提供**（DefId::ALL 与 DefId::params 是唯一数据源），
## 壳里不硬编码任何"哪个组件能改什么"的知识。

signal library_selected(def_id: String)
signal example_selected(example_id: String)
signal tick_pressed()
signal run_toggled(running: bool)
signal reset_pressed()
signal delete_pressed()
signal rotate_pressed()
signal param_changed(key: String, value: int)
signal tick_rate_changed(rate: float)
signal locate_requested(inst: int)
signal undo_pressed()
signal redo_pressed()
signal display_name_changed(id: int, name: String)
signal save_pressed()
signal load_pressed()
signal clear_pressed()

const PANEL_BG := Color(0.10, 0.11, 0.14, 0.94)
const HEAD := Color(0.58, 0.65, 0.75)

var core = null

var _lib_ids := PackedStringArray()
var _lib_labels := PackedStringArray()
var _lib_cats := PackedStringArray()
var _list: VBoxContainer
var _status: Label
var _stats: Label
var _drc_label: Label
var _props: VBoxContainer
var _run_btn: Button
var _example_picker: OptionButton
var _issues: ItemList
var _selected := -1


func _ready() -> void:
	var root := Control.new()
	root.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	root.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(root)

	_build_library_panel(root)
	_build_bottom_bar(root)
	_build_props_panel(root)


func _panel_style() -> StyleBoxFlat:
	var sb := StyleBoxFlat.new()
	sb.bg_color = PANEL_BG
	sb.set_corner_radius_all(6)
	sb.set_content_margin_all(8)
	sb.border_color = Color(0.22, 0.25, 0.31)
	sb.set_border_width_all(1)
	return sb


func _head_label(text: String) -> Label:
	var l := Label.new()
	l.text = text
	l.add_theme_font_size_override("font_size", 11)
	l.add_theme_color_override("font_color", HEAD)
	return l


func _hint(text: String) -> Label:
	var l := Label.new()
	l.text = text
	l.add_theme_font_size_override("font_size", 11)
	l.add_theme_color_override("font_color", Color(0.5, 0.56, 0.64))
	l.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	return l


func _build_library_panel(root: Control) -> void:
	var panel := PanelContainer.new()
	panel.add_theme_stylebox_override("panel", _panel_style())
	panel.set_anchors_and_offsets_preset(Control.PRESET_LEFT_WIDE)
	panel.offset_left = 10
	panel.offset_top = 10
	panel.offset_bottom = -10
	panel.offset_right = 218
	root.add_child(panel)

	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 4)
	panel.add_child(box)

	var title := Label.new()
	title.text = "LogicLab"
	title.add_theme_font_size_override("font_size", 18)
	title.add_theme_color_override("font_color", Color(0.90, 0.92, 0.96))
	box.add_child(title)

	# 示例电路：点一下就能看到能跑的电路长什么样
	box.add_child(_head_label("示例电路"))
	_example_picker = OptionButton.new()
	_example_picker.add_theme_font_size_override("font_size", 12)
	box.add_child(_example_picker)

	box.add_child(_head_label("元件库（点选后在画布上放置）"))

	var scroll := ScrollContainer.new()
	scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	box.add_child(scroll)

	_list = VBoxContainer.new()
	_list.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_list.add_theme_constant_override("separation", 2)
	scroll.add_child(_list)

	var hist := HBoxContainer.new()
	box.add_child(hist)
	var undo_btn := Button.new()
	undo_btn.text = "撤销 ^Z"
	undo_btn.add_theme_font_size_override("font_size", 11)
	undo_btn.pressed.connect(func(): undo_pressed.emit())
	hist.add_child(undo_btn)
	var redo_btn := Button.new()
	redo_btn.text = "重做 ^Y"
	redo_btn.add_theme_font_size_override("font_size", 11)
	redo_btn.pressed.connect(func(): redo_pressed.emit())
	hist.add_child(redo_btn)

	var row := HBoxContainer.new()
	box.add_child(row)
	var save_btn := Button.new()
	save_btn.text = "保存"
	save_btn.pressed.connect(func(): save_pressed.emit())
	row.add_child(save_btn)
	var load_btn := Button.new()
	load_btn.text = "载入"
	load_btn.pressed.connect(func(): load_pressed.emit())
	row.add_child(load_btn)
	var new_btn := Button.new()
	new_btn.text = "新建"
	new_btn.pressed.connect(func(): clear_pressed.emit())
	row.add_child(new_btn)

	box.add_child(_head_label("设计规则检查（双击条目定位）"))
	var check_btn := Button.new()
	check_btn.text = "运行检查"
	check_btn.add_theme_font_size_override("font_size", 12)
	check_btn.pressed.connect(_on_check_pressed)
	box.add_child(check_btn)

	_issues = ItemList.new()
	_issues.custom_minimum_size = Vector2(0, 120)
	_issues.add_theme_font_size_override("font_size", 11)
	_issues.item_activated.connect(_on_issue_activated)
	box.add_child(_issues)


func _build_bottom_bar(root: Control) -> void:
	var panel := PanelContainer.new()
	panel.add_theme_stylebox_override("panel", _panel_style())
	panel.set_anchors_and_offsets_preset(Control.PRESET_CENTER_BOTTOM)
	panel.offset_left = -270
	panel.offset_right = 270
	panel.offset_top = -54
	panel.offset_bottom = -10
	root.add_child(panel)

	var row := HBoxContainer.new()
	row.add_theme_constant_override("separation", 6)
	panel.add_child(row)

	var tick_btn := Button.new()
	tick_btn.text = "单步"
	tick_btn.pressed.connect(func(): tick_pressed.emit())
	row.add_child(tick_btn)

	_run_btn = Button.new()
	_run_btn.text = "运行"
	_run_btn.toggle_mode = true
	_run_btn.toggled.connect(_on_run_toggled)
	row.add_child(_run_btn)

	var reset_btn := Button.new()
	reset_btn.text = "复位"
	reset_btn.pressed.connect(func(): reset_pressed.emit())
	row.add_child(reset_btn)

	var rate := HSlider.new()
	rate.min_value = 1
	rate.max_value = 60
	rate.step = 1
	rate.value = 4
	rate.custom_minimum_size = Vector2(110, 0)
	rate.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	rate.value_changed.connect(func(v: float): tick_rate_changed.emit(v))
	row.add_child(rate)

	_status = Label.new()
	_status.text = "就绪"
	_status.add_theme_font_size_override("font_size", 11)
	_status.add_theme_color_override("font_color", Color(0.62, 0.70, 0.80))
	row.add_child(_status)


func _build_props_panel(root: Control) -> void:
	var panel := PanelContainer.new()
	panel.add_theme_stylebox_override("panel", _panel_style())
	panel.set_anchors_and_offsets_preset(Control.PRESET_TOP_RIGHT)
	panel.offset_left = -238
	panel.offset_right = -10
	panel.offset_top = 10
	panel.offset_bottom = 300
	root.add_child(panel)

	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 6)
	panel.add_child(box)

	_stats = Label.new()
	_stats.text = "—"
	_stats.add_theme_font_size_override("font_size", 11)
	_stats.add_theme_color_override("font_color", Color(0.62, 0.70, 0.80))
	_stats.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(_stats)

	_drc_label = Label.new()
	_drc_label.text = ""
	_drc_label.add_theme_font_size_override("font_size", 11)
	_drc_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(_drc_label)

	box.add_child(_head_label("选中元件"))

	var scroll := ScrollContainer.new()
	scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	box.add_child(scroll)

	_props = VBoxContainer.new()
	_props.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_props.add_theme_constant_override("separation", 6)
	scroll.add_child(_props)


# ---------------------------------------------------------------------------
# 元件库与示例
# ---------------------------------------------------------------------------

func build_library(c) -> void:
	core = c
	_lib_ids = c.library_ids()
	_lib_labels = c.library_labels()
	_lib_cats = c.library_categories()

	for child in _list.get_children():
		child.queue_free()
	var current := ""
	for i in _lib_ids.size():
		var cat: String = _lib_cats[i]
		if cat != current:
			current = cat
			_list.add_child(_head_label(cat))
		var btn := Button.new()
		btn.text = _lib_labels[i]
		btn.alignment = HORIZONTAL_ALIGNMENT_LEFT
		btn.add_theme_font_size_override("font_size", 12)
		btn.tooltip_text = _lib_ids[i]
		var def_id: String = _lib_ids[i]
		btn.pressed.connect(func(): library_selected.emit(def_id))
		_list.add_child(btn)

	var names = c.example_names()
	var ids = c.example_ids()
	var notes = c.example_notes()
	_example_picker.clear()
	_example_picker.add_item("（选择示例…）", 0)
	for i in names.size():
		_example_picker.add_item(names[i], i + 1)
		_example_picker.set_item_tooltip(i + 1, notes[i])
	_example_picker.item_selected.connect(
		func(idx: int):
			if idx > 0:
				example_selected.emit(String(ids[idx - 1]))
	)


# ---------------------------------------------------------------------------
# 状态
# ---------------------------------------------------------------------------

func set_status(text: String) -> void:
	if _status != null:
		_status.text = text


func set_stats(text: String) -> void:
	if _stats != null:
		_stats.text = text


func _on_run_toggled(pressed: bool) -> void:
	_run_btn.text = "暂停" if pressed else "运行"
	run_toggled.emit(pressed)


# ---------------------------------------------------------------------------
# 属性面板（控件由 core 的参数描述生成）
# ---------------------------------------------------------------------------

func on_selection_changed(id: int) -> void:
	_selected = id
	for child in _props.get_children():
		child.queue_free()
	if id < 0 or core == null:
		_props.add_child(_hint("未选中元件"))
		return

	var info = core.component_info(id)
	if info.size() < 10:
		return
	var def_idx := int(info[0])

	var name_label := Label.new()
	name_label.text = _lib_labels[def_idx] if def_idx < _lib_labels.size() else "?"
	name_label.add_theme_font_size_override("font_size", 14)
	name_label.add_theme_color_override("font_color", Color(0.92, 0.94, 0.97))
	_props.add_child(name_label)

	# 实例名（层级调试路径与导出的基准，ADR-25）
	_props.add_child(_head_label("实例名"))
	var name_edit := LineEdit.new()
	name_edit.text = String(core.instance_name(id))
	name_edit.add_theme_font_size_override("font_size", 11)
	name_edit.text_submitted.connect(func(t: String): display_name_changed.emit(id, t))
	_props.add_child(name_edit)

	var keys = core.param_keys(def_idx)
	var labels = core.param_labels(def_idx)
	var kinds = core.param_kinds(def_idx)
	var lo = core.param_lo(def_idx)
	var hi = core.param_hi(def_idx)
	var choices = core.param_choices(def_idx)
	var values = core.param_values(id)

	if keys.size() == 0:
		_props.add_child(_hint("该组件没有可调参数"))

	for i in keys.size():
		var key := String(keys[i])
		var label := String(labels[i])
		var kind := int(kinds[i])
		var cur := int(values[i]) if i < values.size() else 0
		if kind == 0:
			_props.add_child(_head_label(label))
			var labels_csv := ""
			var all_labels = core.param_choice_labels(def_idx)
			if i < all_labels.size():
				labels_csv = String(all_labels[i])
			_props.add_child(_choice_row(key, String(choices[i]), labels_csv, cur))
		elif kind == 1:
			_props.add_child(_int_row(key, label, int(lo[i]), int(hi[i]), cur))
		else:
			_props.add_child(_bool_row(key, label, cur != 0))

	var row := HBoxContainer.new()
	_props.add_child(row)
	var rot := Button.new()
	rot.text = "旋转 (R)"
	rot.pressed.connect(func(): rotate_pressed.emit())
	row.add_child(rot)
	var del := Button.new()
	del.text = "删除 (Del)"
	del.pressed.connect(func(): delete_pressed.emit())
	row.add_child(del)


## 档位按钮：值与显示名分开（移位器的"左移/算术右移"不能只显示数字）
func _choice_row(key: String, choices: String, labels_csv: String, cur: int) -> HBoxContainer:
	var row := HBoxContainer.new()
	row.add_theme_constant_override("separation", 2)
	var values := choices.split(",")
	var labels := labels_csv.split(",")
	for i in values.size():
		if values[i] == "":
			continue
		var v := int(values[i])
		var b := Button.new()
		b.text = labels[i] if i < labels.size() and labels[i] != "" else values[i]
		b.toggle_mode = true
		b.button_pressed = (v == cur)
		b.add_theme_font_size_override("font_size", 11)
		b.pressed.connect(func(): param_changed.emit(key, v))
		row.add_child(b)
	return row


func _int_row(key: String, label: String, lo: int, hi: int, cur: int) -> VBoxContainer:
	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 2)
	box.add_child(_head_label(label))
	var spin := SpinBox.new()
	spin.min_value = lo
	spin.max_value = hi
	spin.value = clampf(float(cur), float(lo), float(hi))
	spin.value_changed.connect(func(v: float): param_changed.emit(key, int(v)))
	box.add_child(spin)
	return box


func _bool_row(key: String, label: String, on: bool) -> CheckBox:
	var cb := CheckBox.new()
	cb.text = label
	cb.button_pressed = on
	cb.add_theme_font_size_override("font_size", 12)
	cb.toggled.connect(func(v: bool): param_changed.emit(key, 1 if v else 0))
	return cb


# ---------------------------------------------------------------------------
# 设计规则检查
# ---------------------------------------------------------------------------

func _on_issue_activated(index: int) -> void:
	var inst := int(_issues.get_item_metadata(index))
	locate_requested.emit(inst)


func _on_check_pressed() -> void:
	var summary = core.drc_summary()
	var details = core.drc_details()
	var errors := int(summary[0])
	var warnings := int(summary[1])

	# 问题列表：每条带实例号，双击即可定位（v4 §8）
	_issues.clear()
	var raw = core.drc_issues()
	var k := 0
	while k + 3 < raw.size():
		var severity := int(raw[k + 1])
		var inst := int(raw[k + 3])
		var mark := "✗" if severity == 1 else "!"
		_issues.add_item("%s %s" % [mark, String(details[k / 4])])
		_issues.set_item_metadata(_issues.item_count - 1, inst)
		k += 4
	if _issues.item_count == 0:
		_issues.add_item("（没有发现问题）")
		_issues.set_item_selectable(0, false)

	# 关键路径（v4 §9.5）：最长组合逻辑路径，单位 tick
	var cp = core.critical_path()
	if cp.size() >= 1:
		_drc_label.text += "\n关键路径：%d 级（含 %d 个组件）" % [int(cp[0]), cp.size() - 1]

	if _drc_label != null:
		_drc_label.text = "DRC：错误 %d · 警告 %d" % [errors, warnings]
		_drc_label.add_theme_color_override(
			"font_color",
			Color(0.95, 0.45, 0.45) if errors > 0 else (Color(0.9, 0.78, 0.35) if warnings > 0 else Color(0.45, 0.85, 0.55))
		)

	var text := "错误 %d · 警告 %d\n\n" % [errors, warnings]
	if details.size() == 0:
		text += "没有发现问题。"
	else:
		var limit := mini(details.size(), 40)
		for i in limit:
			text += "· %s\n" % details[i]
		if details.size() > limit:
			text += "… 另有 %d 条未显示" % (details.size() - limit)

	var dlg := AcceptDialog.new()
	dlg.title = "设计规则检查"
	dlg.dialog_text = text
	dlg.ok_button_text = "关闭"
	add_child(dlg)
	dlg.confirmed.connect(dlg.queue_free)
	dlg.canceled.connect(dlg.queue_free)
	dlg.close_requested.connect(dlg.queue_free)
	dlg.popup_centered(Vector2i(600, 440))
