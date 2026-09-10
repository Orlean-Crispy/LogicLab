class_name HierPanel
extends CanvasLayer

const I18n = preload("res://scripts/i18n.gd")
## 层次导航条：面包屑 + 图纸库 + 新建/改名/删除 + 封装 + 波形开关。
##
## 图纸与接口的知识全在 core 里（board_names / breadcrumb / board_refs），
## 这里只把它们摊成按钮，再把点击翻译回 open_board / goto_depth / add_board。

signal level_requested(depth: int)
signal open_requested(idx: int)
signal extract_requested(cname: String)
signal wave_toggled(on: bool)

const PANEL_BG := Color(0.10, 0.11, 0.14, 0.94)
const HEAD := Color(0.58, 0.65, 0.75)

## 与 ui_panel 保持一致：导航条必须避让左右侧栏，否则宽窗口下会撞上属性面板
const LIB_W := 218.0
const PROPS_W := 238.0

var core = null

var _bar: PanelContainer
var _crumbs: HBoxContainer
var _picker: OptionButton
var _info: Label
var _wave_btn: Button
var _updating := false
var _dialog: AcceptDialog
var _dialog_edit: LineEdit
var _dialog_cb := Callable()


func _ready() -> void:
	var layer := Control.new()
	layer.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	layer.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(layer)

	var bar := PanelContainer.new()
	var sb := StyleBoxFlat.new()
	sb.bg_color = PANEL_BG
	sb.set_corner_radius_all(6)
	sb.set_content_margin_all(6)
	sb.border_color = Color(0.22, 0.25, 0.31)
	sb.set_border_width_all(1)
	bar.add_theme_stylebox_override("panel", sb)
	layer.add_child(bar)
	# 位置由 _layout_bar 按窗口宽度算，不能用 CENTER_TOP：
	# 水平居中在宽窗口下会一直延伸到右上角，和属性面板叠在一起。
	bar.set_anchors_and_offsets_preset(Control.PRESET_TOP_LEFT, Control.PRESET_MODE_MINSIZE, 0)
	_bar = bar
	get_viewport().size_changed.connect(_layout_bar)
	_layout_bar()

	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 4)
	bar.add_child(box)

	_crumbs = HBoxContainer.new()
	_crumbs.add_theme_constant_override("separation", 2)
	box.add_child(_crumbs)

	var row := HBoxContainer.new()
	row.add_theme_constant_override("separation", 4)
	box.add_child(row)

	_picker = OptionButton.new()
	_picker.custom_minimum_size = Vector2(180, 0)
	_picker.item_selected.connect(_on_pick)
	row.add_child(_picker)

	row.add_child(_button("＋ 新建", _on_new))
	row.add_child(_button("改名", _on_rename))
	row.add_child(_button("删除", _on_delete))
	row.add_child(_button("封装选区", _on_extract))

	_wave_btn = Button.new()
	_wave_btn.text = "波形"
	_wave_btn.toggle_mode = true
	_wave_btn.toggled.connect(func(on): wave_toggled.emit(on))
	row.add_child(_wave_btn)

	_info = Label.new()
	_info.add_theme_font_size_override("font_size", 11)
	_info.add_theme_color_override("font_color", HEAD)
	# 它是运行时算出来的动态文本（含数字），由 refresh 自己翻译；
	# 若让控件树遍历插手，meta 会缓存「格式化后的中文」，而表里的 key 是模板，永远对不上。
	_info.set_meta("no_i18n", true)
	row.add_child(_info)


## 导航条在左右侧栏之间铺开，窗口变窄时跟着收窄
func _layout_bar() -> void:
	var vp := get_viewport().get_visible_rect().size
	var x := LIB_W + 8.0
	_bar.position = Vector2(x, 6.0)
	_bar.size = Vector2(maxf(340.0, vp.x - x - PROPS_W - 8.0), _bar.get_combined_minimum_size().y)


## 语言切换后重建导航条：面包屑与图纸下拉都是运行时才生成的文本
func retranslate() -> void:
	refresh()
	I18n.apply(self)


func _button(text: String, cb: Callable) -> Button:
	var b := Button.new()
	b.text = text
	b.add_theme_font_size_override("font_size", 12)
	b.pressed.connect(cb)
	return b


## 每帧轻量同步：层级可能被画布里的双击/快捷键改掉
func _process(_delta: float) -> void:
	if core == null or _updating:
		return
	if _picker.item_count != core.board_names(I18n.lang).size():
		refresh()
		return
	var cur: int = int(core.board_index())
	if _picker.selected != cur:
		refresh()


func refresh() -> void:
	if core == null or _updating:
		return
	_updating = true
	var names: PackedStringArray = core.board_names(I18n.lang)
	var cur: int = int(core.board_index())
	var path: PackedStringArray = core.breadcrumb(I18n.lang)

	for c in _crumbs.get_children():
		_crumbs.remove_child(c)
		c.queue_free()
	for i in path.size():
		if i > 0:
			var sep := Label.new()
			sep.text = "›"
			sep.add_theme_color_override("font_color", HEAD)
			_crumbs.add_child(sep)
		var b := Button.new()
		b.text = String(path[i])
		b.flat = true
		b.add_theme_font_size_override("font_size", 12)
		b.pressed.connect(_on_crumb.bind(i))
		_crumbs.add_child(b)

	_picker.clear()
	for i in names.size():
		_picker.add_item("%d · %s" % [i, String(names[i])], i)
	if cur >= 0 and cur < names.size():
		_picker.select(cur)
	var refs: int = int(core.board_refs(cur))
	_info.text = I18n.tf(
		"第 %d 层 · 共 %d 张图纸 · 本图纸被引用 %d 次",
		[int(core.depth()), names.size(), refs]
	)
	_updating = false
	# _process 会按需重跑 refresh，若不在这里收尾，中文原文会把切好的英文覆盖回去
	I18n.apply(self)


func _on_crumb(depth: int) -> void:
	level_requested.emit(depth)


func _on_pick(idx: int) -> void:
	open_requested.emit(_picker.get_item_id(idx))


func _on_new() -> void:
	_prompt("新建子电路", "子电路", func(n: String) -> void:
		var idx: int = int(core.add_board(n))
		if idx >= 0:
			open_requested.emit(idx))


func _on_rename() -> void:
	var cur: int = int(core.board_index())
	var names: PackedStringArray = core.board_names(I18n.lang)
	if cur < 0 or cur >= names.size():
		return
	_prompt("重命名图纸", String(names[cur]), func(n: String) -> void:
		core.set_board_name(cur, n)
		refresh())


func _on_delete() -> void:
	var cur: int = int(core.board_index())
	if int(core.board_names(I18n.lang).size()) <= 1:
		return
	core.remove_board(cur)
	refresh()


func _on_extract() -> void:
	_prompt("封装为子电路", "子电路", func(n: String) -> void: extract_requested.emit(n))


func _prompt(title: String, initial: String, cb: Callable) -> void:
	if _dialog == null:
		_dialog = AcceptDialog.new()
		_dialog_edit = LineEdit.new()
		_dialog_edit.custom_minimum_size = Vector2(280, 0)
		_dialog.add_child(_dialog_edit)
		add_child(_dialog)
		_dialog.confirmed.connect(_on_dialog_ok)
	_dialog.title = title
	_dialog_edit.text = initial
	_dialog_cb = cb
	_dialog.popup_centered()
	_dialog_edit.grab_focus()
	_dialog_edit.select_all()


func _on_dialog_ok() -> void:
	if _dialog_cb.is_valid():
		_dialog_cb.call(_dialog_edit.text)