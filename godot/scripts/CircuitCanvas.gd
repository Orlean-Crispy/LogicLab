class_name CircuitCanvas
extends Control
const I18n = preload("res://scripts/i18n.gd")
## 自绘电路画布。
##
## 架构法则 #1：零 per-component 节点——所有元件、引脚、导线都靠 draw_* 批量绘制，
## 所以万级元件也不会把场景树撑爆。
## 架构法则 #2：值走增量通道（core.take_changed()），几何只在 revision 变化时重建。

const GRID := 8.0
const PIN_PX := 3.0
const NO_NET := 4294967295
const CUSTOM_COLOR := Color(0.22, 0.40, 0.50)
const BASE_COLORS := [
	Color(0.30, 0.42, 0.55),  # io
	Color(0.30, 0.48, 0.38),  # logic
	Color(0.44, 0.38, 0.55),  # routing
	Color(0.50, 0.40, 0.30),  # arith
	Color(0.32, 0.36, 0.52),  # memory
]

signal selection_changed(id: int)
signal status_changed(text: String)
signal stats_changed(text: String)
signal wave_changed()

var core = null

# 相机：view_offset 是视口中心对应的世界格坐标
var view_offset := Vector2.ZERO
var zoom := 1.0

# 视图缓存
var revision := -1
var comps := PackedInt64Array()
var pins_arr := PackedInt64Array()
var w_ranges := PackedInt64Array()
var w_points := PackedInt64Array()
var net_vals := PackedInt64Array()
var lib_labels := PackedStringArray()
var lib_cats := PackedInt64Array()
var ann_pos := PackedInt64Array()
var ann_texts := PackedStringArray()
var label_pos := PackedInt64Array()
var label_names := PackedStringArray()

# 交互
var pending_def := ""
var selected := -1
var selection: Array[int] = []
var sub_names := PackedStringArray()
var def_seven_seg := -1
var def_display := -1
var box_selecting := false
var box_start := Vector2i.ZERO
var box_end := Vector2i.ZERO
var _double_clicked := false
var dragging := false
var drag_anchor := Vector2i.ZERO
var drag_grab := Vector2i.ZERO
var drag_delta := Vector2i.ZERO
var wiring_from := Vector2i(-1, -1)
var wiring_mouse := Vector2.ZERO
var panning := false
var pan_last := Vector2.ZERO
var hover_pin := Vector2i(-1, -1)
var running := false
var acc := 0.0
var tick_rate := 4.0
var _stats_timer := 0.0
var _font: Font = null


func _ready() -> void:
	mouse_filter = Control.MOUSE_FILTER_STOP
	focus_mode = Control.FOCUS_ALL
	clip_contents = true


# ---------------------------------------------------------------------------
# 坐标
# ---------------------------------------------------------------------------

func world_to_screen(p: Vector2) -> Vector2:
	return (p - view_offset) * GRID * zoom + size * 0.5


func screen_to_world(s: Vector2) -> Vector2:
	return (s - size * 0.5) / (GRID * zoom) + view_offset


func screen_to_cell(s: Vector2) -> Vector2i:
	var w := screen_to_world(s)
	return Vector2i(roundi(w.x), roundi(w.y))


# ---------------------------------------------------------------------------
# 视图缓存
# ---------------------------------------------------------------------------

## 语言切换后重新拉取组件标签并重绘
func retranslate() -> void:
	lib_labels = PackedStringArray()
	refresh()


func refresh() -> void:
	if core == null:
		return
	revision = core.revision()
	comps = core.components()
	pins_arr = core.pins()
	w_ranges = core.wire_ranges()
	w_points = core.wire_points()
	net_vals = core.net_values()
	ann_pos = core.annotations()
	ann_texts = core.annotation_texts()
	label_pos = core.labels()
	label_names = core.label_names()
	sub_names = core.board_names(I18n.lang)
	if lib_labels.is_empty():
		lib_labels = core.library_labels(I18n.lang)
		lib_cats = core.library_category_codes()
		var lib_ids = core.library_ids()
		def_seven_seg = lib_ids.find("seven_seg")
		def_display = lib_ids.find("display")
	queue_redraw()
	_emit_stats()


func _net_field(net: int, k: int) -> int:
	if net < 0 or net == NO_NET:
		return 0
	var i := net * 2 + k
	if i < 0 or i >= net_vals.size():
		return 0
	return int(net_vals[i])


func _net_one(net: int) -> bool:
	return _net_field(net, 0) != 0


func _net_unknown(net: int) -> bool:
	return _net_field(net, 1) != 0


# ---------------------------------------------------------------------------
# 每帧
# ---------------------------------------------------------------------------

func _process(delta: float) -> void:
	if core == null:
		return
	var ticks_done := 0
	if running:
		acc += delta
		var dt := 1.0 / maxf(tick_rate, 0.1)
		# 低帧率时限制每帧最多推进的拍数，避免雪崩（规格 §3.3）
		var cap := maxi(1, int(tick_rate / 60.0) + 1) * 4
		while acc >= dt and ticks_done < cap:
			core.tick()
			acc -= dt
			ticks_done += 1

	var rev = core.revision()
	var structural = rev != revision
	if structural:
		refresh()
	elif ticks_done > 0:
		_apply_changed()

	_stats_timer += delta
	if _stats_timer > 0.25:
		_stats_timer = 0.0
		_emit_stats()


func _apply_changed() -> void:
	var changed = core.take_changed()
	if changed.is_empty():
		return
	var i := 0
	while i + 2 < changed.size():
		var n := int(changed[i])
		if n >= 0 and n * 2 + 1 < net_vals.size():
			net_vals[n * 2] = changed[i + 1]
			net_vals[n * 2 + 1] = changed[i + 2]
		i += 3
	queue_redraw()


func _emit_stats() -> void:
	if core == null:
		return
	var s = core.stats()
	if s.size() >= 8:
		stats_changed.emit(
			I18n.tf(
				"tick %d · 元件 %d · 导线 %d · 网络 %d · 本拍求值 %d",
				[int(s[5]), int(s[0]), int(s[4]), int(s[2]), int(s[6])]
			)
		)


# ---------------------------------------------------------------------------
# 绘制
# ---------------------------------------------------------------------------

func _draw() -> void:
	if _font == null:
		_font = get_theme_default_font()
	_draw_grid()
	_draw_wires()
	_draw_components()
	_draw_pins()
	_draw_annotations()
	_draw_labels()
	_draw_wiring_preview()
	_draw_box_select()


func _draw_grid() -> void:
	# 步长自适应，保证屏幕间距不小于 6px（否则缩小后会画出成千上万条线）
	var step := 1
	while float(step) * GRID * zoom < 6.0:
		step *= 2
	var line := Color(0.15, 0.17, 0.21)
	var major := Color(0.19, 0.22, 0.27)
	var tl := screen_to_world(Vector2.ZERO)
	var br := screen_to_world(size)
	var x := floori(tl.x / step) * step
	while float(x) <= br.x:
		var sx := world_to_screen(Vector2(float(x), 0.0)).x
		draw_line(Vector2(sx, 0.0), Vector2(sx, size.y), major if x % (step * 4) == 0 else line)
		x += step
	var y := floori(tl.y / step) * step
	while float(y) <= br.y:
		var sy := world_to_screen(Vector2(0.0, float(y))).y
		draw_line(Vector2(0.0, sy), Vector2(size.x, sy), major if y % (step * 4) == 0 else line)
		y += step


func _wire_color(net: int, status: int) -> Color:
	if net == NO_NET:
		return Color(0.34, 0.37, 0.44)
	if status == 2:
		return Color(0.95, 0.35, 0.35)
	if status == 1:
		return Color(0.40, 0.43, 0.50)
	if _net_unknown(net):
		return Color(0.95, 0.78, 0.30)
	if _net_one(net):
		return Color(0.35, 0.90, 0.55)
	return Color(0.30, 0.34, 0.40)


func _draw_wires() -> void:
	var i := 0
	while i + 4 < w_ranges.size():
		var start := int(w_ranges[i])
		var count := int(w_ranges[i + 1])
		var net := int(w_ranges[i + 2])
		var status := int(w_ranges[i + 3])
		var bits := int(w_ranges[i + 4])
		var col := _wire_color(net, status)
		var wpx := 1.0 + (1.5 if bits > 1 else 0.0)
		var prev := Vector2.ZERO
		var k := 0
		while k < count:
			var idx := start + k * 2
			if idx + 1 >= w_points.size():
				break
			var sp := world_to_screen(Vector2(float(w_points[idx]), float(w_points[idx + 1])))
			if k > 0:
				draw_line(prev, sp, col, wpx, true)
			prev = sp
			k += 1
		i += 5


## 世界空间里的文字必须跟着缩放走。
##
## 字号写死成像素会出现反直觉的结果：放大画布时元件框一直在长，字却纹丝不动，
## 于是看上去"放大反而字更小"；缩小时字又会撑破元件框。
## 下限保证缩到很小时还认得出，上限避免放到很大时糊成一片。
func _scaled_font_size(base: float, lo := 7.0, hi := 32.0) -> int:
	return int(clampf(base * zoom, lo, hi))


func _draw_components() -> void:
	var i := 0
	while i + 7 < comps.size():
		var id := int(comps[i])
		var def_idx := int(comps[i + 1])
		var x := int(comps[i + 2])
		var y := int(comps[i + 3])
		var w := int(comps[i + 4])
		var h := int(comps[i + 5])
		var sub := int(comps[i + 7])
		var off := drag_delta if (dragging and _is_selected(id)) else Vector2i.ZERO
		var tl := world_to_screen(Vector2(float(x + off.x), float(y + off.y)))
		var br := world_to_screen(Vector2(float(x + off.x + w), float(y + off.y + h)))
		var rect := Rect2(tl, br - tl)
		var col: Color
		if sub >= 0:
			col = CUSTOM_COLOR
		else:
			var cat := int(lib_cats[def_idx]) if def_idx < lib_cats.size() else 0
			col = BASE_COLORS[cat % BASE_COLORS.size()]
		draw_rect(rect, col, true)
		draw_rect(rect, col.lightened(0.45), false, 1.0)
		if _is_selected(id):
			draw_rect(rect.grow(2.0), Color(1.0, 0.85, 0.35), false, 2.0)
		if sub >= 0:
			if zoom > 0.35:
				var nm := String(sub_names[sub]) if sub < sub_names.size() else "子电路"
				_draw_fitted(tl, nm, Color(0.85, 0.95, 1.0), rect.size.y)
		elif def_idx == def_seven_seg:
			_draw_seven_seg(rect, id)
		elif def_idx == def_display:
			_draw_display(rect, id)
		elif zoom > 0.5 and def_idx < lib_labels.size():
			_draw_fitted(tl, String(lib_labels[def_idx]), Color(0.88, 0.91, 0.95), rect.size.y)
		i += 8


## 把一行标签塞进元件的屏幕矩形里：字号跟着缩放，同时不超过矩形高度。
func _draw_fitted(tl: Vector2, text: String, col: Color, box_h: float) -> void:
	var fs := _scaled_font_size(9.0, 7.0, 24.0)
	fs = mini(fs, int(maxf(7.0, box_h * 1.1)))
	draw_string(_font, tl + Vector2(3.0, float(fs) + 1.0), text, HORIZONTAL_ALIGNMENT_LEFT, -1, fs, col)


## 七段数码管：十进制读数直接画在元件上（值来自引脚，别处不再存一份）
func _draw_seven_seg(rect: Rect2, id: int) -> void:
	draw_rect(rect, Color(0.05, 0.06, 0.08), true)
	var v := _first_input_value(id)
	draw_string(
		_font,
		rect.position + Vector2(4.0, rect.size.y * 0.5 + 7.0),
		str(v),
		HORIZONTAL_ALIGNMENT_LEFT,
		-1,
		int(maxf(10.0, rect.size.y * 0.75)),
		Color(0.35, 1.0, 0.55)
	)


## 点阵屏：帧缓冲住在 core 里（instance_mem），壳只读不写
func _draw_display(rect: Rect2, id: int) -> void:
	draw_rect(rect, Color(0.03, 0.04, 0.05), true)
	var fb: PackedInt64Array = core.instance_mem(id)
	if fb.is_empty():
		return
	var info = core.component_info(id)
	var cols := int(info[1]) if info.size() > 1 else 8
	if cols <= 0:
		cols = 8
	var rows := fb.size()
	var cw := rect.size.x / float(cols)
	var ch := rect.size.y / float(rows)
	for r in rows:
		var bits := int(fb[r])
		for c in cols:
			if bits & (1 << c):
				draw_rect(Rect2(rect.position + Vector2(c * cw, r * ch), Vector2(cw, ch)), Color(0.4, 0.95, 0.6), true)


func _first_input_value(id: int) -> int:
	var p: PackedInt64Array = core.pin_value(id, 0)
	return int(p[0]) if p.size() > 0 else 0


func _draw_pins() -> void:
	# 缩得太小时不画引脚：万级元件下这一层是主要开销
	if zoom < 0.45:
		return
	var r := maxf(PIN_PX, 2.0 * zoom)
	var i := 0
	while i + 6 < pins_arr.size():
		var inst := int(pins_arr[i])
		var slot := int(pins_arr[i + 1])
		var x := int(pins_arr[i + 2])
		var y := int(pins_arr[i + 3])
		var dir := int(pins_arr[i + 4])
		var bits := int(pins_arr[i + 5])
		var net := int(pins_arr[i + 6])
		var off := drag_delta if (dragging and inst == selected) else Vector2i.ZERO
		var sp := world_to_screen(Vector2(float(x + off.x), float(y + off.y)))
		var col := Color(0.52, 0.58, 0.68) if dir == 0 else Color(0.42, 0.60, 0.82)
		if net != NO_NET:
			if _net_unknown(net):
				col = Color(0.95, 0.78, 0.30)
			elif _net_one(net):
				col = Color(0.40, 0.95, 0.62)
		if inst == wiring_from.x and slot == wiring_from.y:
			col = Color(1.0, 0.85, 0.35)
		elif inst == hover_pin.x and slot == hover_pin.y:
			col = Color(1.0, 0.95, 0.70)
		if bits > 1:
			draw_rect(Rect2(sp - Vector2(r, r), Vector2(r * 2.0, r * 2.0)), col, true)
		else:
			draw_circle(sp, r, col)
		i += 7


func _draw_annotations() -> void:
	var i := 0
	while i + 2 < ann_pos.size():
		var idx := i / 3
		var text := String(ann_texts[idx]) if idx < ann_texts.size() else ""
		var sp := world_to_screen(Vector2(float(ann_pos[i]), float(ann_pos[i + 1])))
		var fs := _scaled_font_size(10.0)
		var size_px := Vector2(float(text.length()) * float(fs) * 0.62 + 10.0, float(fs) * 1.5)
		draw_rect(Rect2(sp + Vector2(0, -float(fs) * 1.2), size_px), Color(0.07, 0.08, 0.10, 0.80), true)
		draw_rect(Rect2(sp + Vector2(0, -float(fs) * 1.2), size_px), Color(0.35, 0.34, 0.24), false, 1.0)
		draw_string(
			_font,
			sp + Vector2(5, -1),
			text,
			HORIZONTAL_ALIGNMENT_LEFT,
			-1,
			fs,
			Color(0.88, 0.83, 0.55)
		)
		i += 3


func _draw_labels() -> void:
	var i := 0
	while i + 1 < label_pos.size():
		var idx := i / 2
		var name := String(label_names[idx]) if idx < label_names.size() else ""
		var sp := world_to_screen(Vector2(float(label_pos[i]), float(label_pos[i + 1])))
		draw_circle(sp, maxf(2.0, 3.0 * zoom), Color(0.45, 0.72, 0.95))
		if zoom > 0.4:
			draw_string(
				_font,
				sp + Vector2(6, 4),
				name,
				HORIZONTAL_ALIGNMENT_LEFT,
				-1,
				_scaled_font_size(10.0),
				Color(0.62, 0.82, 1.0)
			)
		i += 2


func _draw_wiring_preview() -> void:
	if wiring_from.x < 0:
		return
	var from := Vector2.ZERO
	var found := false
	var i := 0
	while i + 6 < pins_arr.size():
		if int(pins_arr[i]) == wiring_from.x and int(pins_arr[i + 1]) == wiring_from.y:
			from = world_to_screen(Vector2(float(pins_arr[i + 2]), float(pins_arr[i + 3])))
			found = true
			break
		i += 7
	if not found:
		return
	var to := world_to_screen(wiring_mouse)
	var mid := Vector2(to.x, from.y)
	draw_polyline(
		PackedVector2Array([from, mid, to]), Color(1.0, 0.85, 0.35, 0.85), 1.5, true
	)


# ---------------------------------------------------------------------------
# 交互
# ---------------------------------------------------------------------------

func _gui_input(event: InputEvent) -> void:
	if core == null:
		return
	if event is InputEventMouseButton:
		_on_mouse_button(event)
	elif event is InputEventMouseMotion:
		_on_mouse_motion(event)
	elif event is InputEventKey and event.pressed and not event.echo:
		_on_key(event)


func _on_mouse_button(e: InputEventMouseButton) -> void:
	var cell := screen_to_cell(e.position)
	if e.pressed and e.button_index == MOUSE_BUTTON_WHEEL_UP:
		_zoom_at(e.position, 1.15)
		accept_event()
		return
	if e.pressed and e.button_index == MOUSE_BUTTON_WHEEL_DOWN:
		_zoom_at(e.position, 1.0 / 1.15)
		accept_event()
		return
	if e.button_index == MOUSE_BUTTON_RIGHT or e.button_index == MOUSE_BUTTON_MIDDLE:
		panning = e.pressed
		pan_last = e.position
		accept_event()
		return
	if e.button_index != MOUSE_BUTTON_LEFT:
		return
	accept_event()
	if e.pressed:
		_double_clicked = e.double_click
		_on_left_press(cell)
	else:
		_on_left_release(cell)


func _on_left_press(cell: Vector2i) -> void:
	# 1) 放置模式
	if pending_def != "":
		var id: int = core.add_component(pending_def, cell.x, cell.y)
		if id >= 0:
			set_selection([id])
			status_changed.emit("已放置")
		pending_def = ""
		drag_delta = Vector2i.ZERO
		queue_redraw()
		return

	# 2) 引脚上 → 开始连线
	var hit = core.pick_pin(cell.x, cell.y, 1)
	if hit.size() >= 2:
		wiring_from = Vector2i(int(hit[0]), int(hit[1]))
		wiring_mouse = Vector2(cell)
		queue_redraw()
		return

	# 3) 元件上 → 选中 / 多选 / 双击进子电路
	var cid: int = core.pick_component(cell.x, cell.y)
	if cid >= 0:
		if _double_clicked:
			_double_clicked = false
			if core.enter_sub(cid):
				set_selection([])
				refresh()
				center_on_content()
				status_changed.emit("进入子电路")
				return
		if _shift_down():
			var sel := selected_ids()
			if sel.has(cid):
				sel.erase(cid)
			else:
				sel.append(cid)
			set_selection(sel)
		elif not selected_ids().has(cid):
			set_selection([cid])
		var info = core.component_info(cid)
		if info.size() >= 8:
			drag_grab = cell - Vector2i(int(info[5]), int(info[6]))
		dragging = true
		drag_anchor = cell
		drag_delta = Vector2i.ZERO
		queue_redraw()
		return

	# 4) 注释上 → 选中并编辑
	var aid: int = core.pick_annotation(cell.x, cell.y)
	if aid >= 0:
		_edit_annotation(aid)
		return

	# 5) 空白 → 开始框选
	box_selecting = true
	box_start = cell
	box_end = cell
	if not _shift_down():
		set_selection([])
	queue_redraw()


func _on_left_release(cell: Vector2i) -> void:
	if wiring_from.x >= 0:
		var hit = core.pick_pin(cell.x, cell.y, 1)
		if hit.size() >= 2:
			var bi = int(hit[0])
			var bs := int(hit[1])
			if bi != wiring_from.x or bs != wiring_from.y:
				var ok: bool = core.connect_pins(wiring_from.x, wiring_from.y, bi, bs)
				status_changed.emit("已连线" if ok else "走线失败：找不到不误连的路径")
		wiring_from = Vector2i(-1, -1)
		queue_redraw()
		return

	if box_selecting:
		box_selecting = false
		_commit_box_select()
		queue_redraw()
		return

	if dragging:
		dragging = false
		if drag_delta == Vector2i.ZERO:
			# 单击（没移动）：开关类元件就地翻转
			var info = core.component_info(selected)
			if info.size() >= 8:
				var flags = core.param_flags(int(info[0]))
				if flags.size() >= 4 and int(flags[3]) == 1:
					core.toggle_input(selected)
					status_changed.emit("开关翻转")
		else:
			var info2 = core.component_info(selected)
			if info2.size() >= 8:
				core.move_components(PackedInt32Array(selected_ids()), drag_delta.x, drag_delta.y)
		drag_delta = Vector2i.ZERO
		queue_redraw()


func _on_mouse_motion(e: InputEventMouseMotion) -> void:
	if panning:
		view_offset -= (e.position - pan_last) / (GRID * zoom)
		pan_last = e.position
		queue_redraw()
		return
	var cell := screen_to_cell(e.position)
	if box_selecting:
		box_end = cell
		queue_redraw()
		return
	if dragging and selected >= 0:
		drag_delta = cell - drag_anchor
		queue_redraw()
		return
	if wiring_from.x >= 0:
		wiring_mouse = Vector2(cell)
		queue_redraw()
		return
	var hit = core.pick_pin(cell.x, cell.y, 1)
	var hp := Vector2i(-1, -1)
	if hit.size() >= 2:
		hp = Vector2i(int(hit[0]), int(hit[1]))
	if hp != hover_pin:
		hover_pin = hp
		queue_redraw()
	_update_tooltip(cell, hp)


## 悬停提示（v4 §8 的"悬停探针"）：元件名 + 引脚当前值
func _update_tooltip(cell: Vector2i, hp: Vector2i) -> void:
	var lines := PackedStringArray()
	var cid: int = core.pick_component(cell.x, cell.y)
	if cid >= 0:
		var info = core.component_info(cid)
		if info.size() >= 10:
			lines.append(
				"%s  [%s]" % [lib_labels[int(info[0])], String(core.instance_name(cid))]
			)
	if hp.x >= 0:
		lines.append("引脚值 %s" % _pin_value_text(hp.x, hp.y))
	var tip := "\n".join(lines)
	if tip != tooltip_text:
		tooltip_text = tip


func _pin_value_text(inst: int, slot: int) -> String:
	var i := 0
	while i + 6 < pins_arr.size():
		if int(pins_arr[i]) == inst and int(pins_arr[i + 1]) == slot:
			return _format_net(int(pins_arr[i + 6]), int(pins_arr[i + 5]))
		i += 7
	return "?"


func _format_net(net: int, bits: int) -> String:
	if net < 0:
		return "未连接"
	if _net_unknown(net):
		return "X（未定义/冲突）"
	var v := _net_field(net, 0)
	if bits <= 1:
		return "1" if v != 0 else "0"
	return "0x%X" % v


func _on_key(e: InputEventKey) -> void:
	if e.ctrl_pressed or e.meta_pressed:
		match e.keycode:
			KEY_Z:
				do_undo()
			KEY_Y:
				do_redo()
		return
	match e.keycode:
		KEY_R:
			rotate_selected()
		KEY_DELETE, KEY_BACKSPACE:
			delete_selected()
		KEY_ESCAPE:
			pending_def = ""
			wiring_from = Vector2i(-1, -1)
			set_selection([])
			queue_redraw()
		KEY_SPACE:
			do_tick()
		KEY_C:
			center_on_content()
		KEY_T:
			_insert_annotation_at_hover()
		KEY_L:
			_insert_label_at_hover()
		KEY_W:
			toggle_watch_at_mouse()
		KEY_E:
			enter_selected_sub()
		KEY_B:
			go_up_level()


## 在鼠标所在格插入一条注释，并立刻让用户输入内容
func _insert_annotation_at_hover() -> void:
	if core == null:
		return
	var cell := screen_to_cell(get_local_mouse_position())
	var id: int = core.add_annotation(cell.x, cell.y, "注释")
	refresh()
	status_changed.emit("已插入注释，按 Enter 编辑")
	_edit_annotation(id)


## 通用单行输入框（注释与网络标签共用）
func _prompt_text(title: String, initial: String, on_ok: Callable) -> void:
	var dlg := AcceptDialog.new()
	dlg.title = title
	dlg.ok_button_text = "确定"
	var edit := LineEdit.new()
	edit.text = initial
	edit.custom_minimum_size = Vector2(360, 0)
	dlg.add_child(edit)
	add_child(dlg)
	dlg.confirmed.connect(func(): on_ok.call(edit.text))
	dlg.confirmed.connect(dlg.queue_free)
	dlg.close_requested.connect(dlg.queue_free)
	dlg.canceled.connect(dlg.queue_free)
	dlg.popup_centered(Vector2i(420, 110))
	edit.select_all()
	edit.grab_focus()


func _edit_annotation(id: int) -> void:
	if id < 0:
		return
	var texts = core.annotation_texts()
	var cur := String(texts[id]) if id < texts.size() else ""
	_prompt_text(
		"注释内容",
		cur,
		func(t: String):
			core.set_annotation_text(id, t)
			refresh()
			status_changed.emit("注释已更新")
	)


## 在网络标签处插入/修改（同名即相连，v4 §8）
func _insert_label_at_hover() -> void:
	if core == null:
		return
	var cell := screen_to_cell(get_local_mouse_position())
	var id: int = core.add_label(cell.x, cell.y, "NET")
	refresh()
	_edit_label(id)


func _edit_label(id: int) -> void:
	if id < 0:
		return
	var names = core.label_names()
	var cur := String(names[id]) if id < names.size() else ""
	_prompt_text(
		"网络名（同名即相连）",
		cur,
		func(t: String):
			core.set_label_name(id, t)
			refresh()
			status_changed.emit("网络标签已更新")
	)


## 撤销 / 重做（ADR-9）
func do_undo() -> void:
	_apply_history(core.undo(), "撤销")


func do_redo() -> void:
	_apply_history(core.redo(), "重做")


func _apply_history(ok: bool, what: String) -> void:
	if not ok:
		status_changed.emit(I18n.t("没有可%s的操作") % what)
		return
	selected = -1
	selection_changed.emit(-1)
	refresh()
	status_changed.emit(I18n.t("已%s") % what)


## 从 DRC 列表定位到某个组件（v4 §8：双击条目定位并高亮）
func locate_component(id: int) -> void:
	if id < 0 or core == null:
		return
	selected = id
	selection_changed.emit(id)
	var info = core.component_info(id)
	if info.size() >= 8:
		view_offset = Vector2(float(info[5]) + 1.0, float(info[6]) + 1.0)
		if zoom < 1.5:
			zoom = 1.5
	status_changed.emit(I18n.t("已定位到 %s") % core.instance_name(id))
	queue_redraw()


func _zoom_at(pos: Vector2, factor: float) -> void:
	var before := screen_to_world(pos)
	zoom = clampf(zoom * factor, 0.05, 8.0)
	var after := screen_to_world(pos)
	view_offset += before - after
	queue_redraw()


func center_on_content() -> void:
	if core == null:
		return
	var b = core.content_bounds()
	if b.size() >= 4:
		var minx := float(b[0])
		var miny := float(b[1])
		var maxx := float(b[2])
		var maxy := float(b[3])
		view_offset = Vector2((minx + maxx) * 0.5, (miny + maxy) * 0.5)
		var w := maxx - minx + 8.0
		var h := maxy - miny + 8.0
		var fit := minf(size.x / (w * GRID), size.y / (h * GRID))
		zoom = clampf(fit, 0.1, 2.5)
	else:
		view_offset = Vector2.ZERO
		zoom = 1.0
	queue_redraw()


# ---------------------------------------------------------------------------
# 面板回调
# ---------------------------------------------------------------------------

func set_pending_def(def_id: String) -> void:
	pending_def = def_id
	status_changed.emit(I18n.t("点击画布放置：%s（Esc 取消）") % def_id)


func do_tick() -> void:
	core.tick()
	_apply_changed()
	_emit_stats()


func set_running(value: bool) -> void:
	running = value
	acc = 0.0


func set_tick_rate(rate: float) -> void:
	tick_rate = clampf(rate, 0.5, 240.0)


func do_reset() -> void:
	core.reset_sim()
	_apply_changed()
	status_changed.emit("仿真已复位")
	_emit_stats()


func delete_selected() -> void:
	var sel := selected_ids()
	if sel.is_empty():
		return
	core.remove_components(PackedInt32Array(sel))
	set_selection([])
	status_changed.emit(I18n.t("已删除 %d 个元件") % sel.size())
	queue_redraw()


func rotate_selected() -> void:
	if selected >= 0:
		core.rotate_component(selected)
		status_changed.emit("已旋转 90°")
		queue_redraw()


func set_display_name_of(id: int, name: String) -> void:
	if id >= 0 and name != "":
		core.set_display_name(id, name)
		status_changed.emit(I18n.t("实例名已改为 %s") % name)


func set_selected_param(key: String, value: int) -> void:
	if selected >= 0:
		core.set_param(selected, key, value)
		queue_redraw()


func load_example(example_id: String) -> void:
	if not core.load_example(example_id):
		status_changed.emit("载入示例失败")
		return
	set_selection([])
	refresh()
	center_on_content()
	status_changed.emit(I18n.t("已载入示例：%s") % example_id)


func save_project() -> void:
	var text := String(core.save_data())
	var f := FileAccess.open("user://logiclab_project.json", FileAccess.WRITE)
	if f:
		f.store_string(text)
		f.close()
		status_changed.emit("已保存到 user://logiclab_project.json")


func load_project() -> void:
	if not FileAccess.file_exists("user://logiclab_project.json"):
		status_changed.emit("没有找到存档")
		return
	var f := FileAccess.open("user://logiclab_project.json", FileAccess.READ)
	if f == null:
		return
	var text := f.get_as_text()
	f.close()
	if core.load_data(text):
		selected = -1
		selection_changed.emit(-1)
		refresh()
		center_on_content()
		status_changed.emit("已载入存档")
	else:
		status_changed.emit("存档损坏，未载入")




# ---------------------------------------------------------------------------
# 多选 / 框选 / 层次（ADR-28）
# ---------------------------------------------------------------------------

## 当前选区。selected 是"主选中"（属性面板与拖动看它），selection 是完整集合
func selected_ids() -> Array:
	if selection.is_empty() and selected >= 0:
		return [selected]
	return selection.duplicate()


func _is_selected(id: int) -> bool:
	if not selection.is_empty():
		return selection.has(id)
	return id == selected


func set_selection(ids: Array) -> void:
	selection.clear()
	for v in ids:
		selection.append(int(v))
	selected = selection[0] if selection.size() > 0 else -1
	selection_changed.emit(selected)


func _shift_down() -> bool:
	return Input.is_key_pressed(KEY_SHIFT)


func _draw_box_select() -> void:
	if not box_selecting:
		return
	var a := world_to_screen(Vector2(box_start))
	var b := world_to_screen(Vector2(box_end + Vector2i(1, 1)))
	var r := Rect2(a, b - a).abs()
	draw_rect(r, Color(0.35, 0.7, 1.0, 0.15), true)
	draw_rect(r, Color(0.45, 0.8, 1.0, 0.8), false, 1.0)


func _commit_box_select() -> void:
	var lo := Vector2i(mini(box_start.x, box_end.x), mini(box_start.y, box_end.y))
	var hi := Vector2i(maxi(box_start.x, box_end.x), maxi(box_start.y, box_end.y))
	var found: Array[int] = []
	var i := 0
	while i + 7 < comps.size():
		var x := int(comps[i + 2])
		var y := int(comps[i + 3])
		var w := int(comps[i + 4])
		var h := int(comps[i + 5])
		if x <= hi.x and y <= hi.y and x + w - 1 >= lo.x and y + h - 1 >= lo.y:
			found.append(int(comps[i]))
		i += 8
	if _shift_down():
		var sel := selected_ids()
		for id in found:
			if not sel.has(id):
				sel.append(id)
		set_selection(sel)
	else:
		set_selection(found)
	status_changed.emit(I18n.t("选中 %d 个元件") % selected_ids().size())


## 把当前选区封装成子电路；返回新图纸索引，失败返回 -1
func extract_selection(cname: String) -> int:
	var sel := selected_ids()
	if sel.is_empty():
		status_changed.emit("先选中要封装的元件")
		return -1
	var idx: int = core.extract_to_sub(PackedInt32Array(sel), cname)
	if idx < 0:
		status_changed.emit("封装失败")
		return -1
	set_selection([])
	refresh()
	center_on_content()
	status_changed.emit(I18n.t("已封装为子电路：%s") % cname)
	return idx



## 观察 / 取消观察鼠标下的导线（§9.1）。波形由 core 逐拍采样，壳只负责画。
func toggle_watch_at_mouse() -> void:
	if core == null:
		return
	var cell := screen_to_cell(get_local_mouse_position())
	var wi: int = core.pick_wire(cell.x, cell.y, 1)
	if wi < 0:
		status_changed.emit("鼠标下没有导线")
		return
	var net := _wire_net(wi)
	if net < 0:
		status_changed.emit("该导线没有网络")
		return
	var nm := "n%d" % net
	if core.wave_watch(net, nm):
		status_changed.emit(I18n.t("已观察 %s") % nm)
	else:
		core.wave_unwatch(net)
		status_changed.emit(I18n.t("已取消观察 %s") % nm)
	wave_changed.emit()
	queue_redraw()


func _wire_net(wi: int) -> int:
	var i := wi * 5
	if i + 2 >= w_ranges.size():
		return -1
	return int(w_ranges[i + 2])


## 进入选中的子电路（双击实例同样可以）
func enter_selected_sub() -> void:
	if selected < 0:
		status_changed.emit("先选中一个子电路实例")
		return
	if core.enter_sub(selected):
		set_selection([])
		refresh()
		center_on_content()
		status_changed.emit("进入子电路")
		queue_redraw()
	else:
		status_changed.emit("该元件不是子电路")


## 回到上一层
func go_up_level() -> void:
	var d: int = core.depth()
	if d <= 0:
		status_changed.emit("已在根图纸")
		return
	core.goto_depth(d - 1)
	set_selection([])
	refresh()
	center_on_content()
	queue_redraw()
	status_changed.emit("返回上一层")


## 直接跳到第 depth 层（面包屑点击）
func goto_level(depth: int) -> void:
	if core.goto_depth(depth):
		set_selection([])
		refresh()
		center_on_content()
		queue_redraw()


## 打开图纸库里的某张图纸
func open_board_index(idx: int) -> void:
	if core.open_board(idx):
		set_selection([])
		refresh()
		center_on_content()
		queue_redraw()


func clear_project() -> void:
	core.clear()
	selected = -1
	selection_changed.emit(-1)
	refresh()
	center_on_content()
	status_changed.emit("已新建空工程")
