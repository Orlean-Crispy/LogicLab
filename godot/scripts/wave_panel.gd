class_name WavePanel
extends Control

const I18n = preload("res://scripts/i18n.gd")
## 波形查看器（§9.1）：读 core 的逐拍采样缓冲，画成方波。
##
## 采样是 core 在 sim_tick 里累积的，壳不参与记录——将来换壳波形数据不用重做，
## VCD 导出也直接来自同一份缓冲。

const ROW_H := 20.0
const NAME_W := 104.0
const WINDOW := 260      # 一屏显示多少拍

var core = null

var _font: Font = null
var _last_tick := -1


func _process(_delta: float) -> void:
	if not visible or core == null:
		return
	var t := int(core.tick_count())
	if t != _last_tick:
		_last_tick = t
		queue_redraw()


func _draw() -> void:
	if core == null:
		return
	if _font == null:
		_font = get_theme_default_font()
	draw_rect(Rect2(Vector2.ZERO, size), Color(0.06, 0.07, 0.09, 0.96), true)
	draw_line(Vector2(0, 0), Vector2(size.x, 0), Color(0.2, 0.23, 0.29), 1.0)
	var names: PackedStringArray = core.wave_names()
	var traces: PackedInt64Array = core.wave_traces()
	if names.is_empty():
		draw_string(_font, Vector2(10.0, 20.0), I18n.t("波形：鼠标停在导线上按 W 观察该网络"), HORIZONTAL_ALIGNMENT_LEFT, -1, 12, Color(0.55, 0.6, 0.68))
		return
	var total := int(traces[2]) if traces.size() >= 3 else 0
	var span := mini(WINDOW, total)
	var plot_w := size.x - NAME_W - 14.0
	if span <= 0 or plot_w <= 20.0:
		draw_string(_font, Vector2(10.0, 20.0), I18n.t("波形：等待采样（运行几拍）"), HORIZONTAL_ALIGNMENT_LEFT, -1, 12, Color(0.55, 0.6, 0.68))
		return
	var step := plot_w / float(span)
	# 竖直刻度：每 10 拍一条淡线
	var k := 0
	while k <= span:
		if k % 10 == 0:
			var gx := NAME_W + k * step
			draw_line(Vector2(gx, 0.0), Vector2(gx, size.y), Color(0.14, 0.16, 0.20), 1.0)
		k += 1
	for r in names.size():
		var y0 := 6.0 + r * ROW_H
		if y0 + ROW_H > size.y:
			break
		var samples: PackedInt64Array = core.wave_samples(r)
		var bits := int(traces[r * 3 + 1]) if r * 3 + 1 < traces.size() else 1
		var label := "%s  [%d 位]" % [String(names[r]), bits] if bits > 1 else String(names[r])
		draw_string(_font, Vector2(8.0, y0 + 14.0), label, HORIZONTAL_ALIGNMENT_LEFT, int(NAME_W - 12.0), 11, Color(0.75, 0.82, 0.9))
		var start := maxi(0, samples.size() - span)
		var hi := y0 + 3.0
		var lo := y0 + ROW_H - 5.0
		var mx := 1
		for i in range(start, samples.size()):
			mx = maxi(mx, int(samples[i]))
		var col := Color(0.42, 0.85, 1.0) if bits == 1 else Color(0.95, 0.78, 0.35)
		var px := NAME_W
		var py := lo
		var first := true
		for i in range(start, samples.size()):
			var v := int(samples[i])
			var y := lo - (lo - hi) * (float(v) / float(mx))
			if first:
				py = y
				first = false
			draw_line(Vector2(px, py), Vector2(px, y), col, 1.5)
			draw_line(Vector2(px, y), Vector2(px + step, y), col, 1.5)
			px += step
			py = y


## 波形面板的期望高度：按观察数自适应
func preferred_height() -> float:
	var n := 0
	if core != null:
		n = core.wave_names().size()
	return maxf(80.0, 24.0 + n * ROW_H)