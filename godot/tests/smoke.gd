extends SceneTree
## headless 冒烟测试：验证 core 的核心链路（放置 → 连线 → 仿真 → 存取 → 编辑）
## 跑法：godot --headless --path godot --script res://tests/smoke.gd
## 这是 beta 的回归底线：任何一项失败都说明桥接或 core 出了问题。

var _fails := 0


func _check(cond: bool, what: String) -> void:
	if cond:
		print("  [ok]   ", what)
	else:
		print("  [FAIL] ", what)
		_fails += 1


func _pin_net(core, inst: int, slot: int) -> int:
	var arr = core.pins()
	var i := 0
	while i + 6 < arr.size():
		if int(arr[i]) == inst and int(arr[i + 1]) == slot:
			return int(arr[i + 6])
		i += 7
	return -1


func _initialize() -> void:
	print("=== LogicLab core 冒烟 ===")
	var core = LogicLab.new()

	print("[1] 元件库")
	var ids = core.library_ids()
	_check(ids.size() >= 30, "内置组件 >= 30 种（实际 %d）" % ids.size())
	_check(core.library_labels().size() == ids.size(), "标签数与 id 数一致")
	_check(core.library_categories().size() == ids.size(), "分类数与 id 数一致")

	print("[2] 放置与命中")
	var sw: int = core.add_component("switch", 0, 0)
	var led: int = core.add_component("led", 6, 0)
	_check(sw >= 0 and led >= 0, "放置开关与探针")
	var hit = core.pick_pin(2, 0, 1)
	_check(hit.size() == 2 and int(hit[0]) == sw, "命中开关输出引脚")
	var cid: int = core.pick_component(6, 0)
	_check(cid == led, "命中探针元件")

	print("[3] 连线")
	var ok: bool = core.connect_pins(sw, 0, led, 0)
	_check(ok, "自动走线成功")
	var net := _pin_net(core, led, 0)
	_check(net >= 0, "探针输入已接入网络")
	_check(core.net_count() >= 1, "网络数 >= 1")

	print("[4] 仿真（1 tick 门延迟）")
	core.toggle_input(sw)
	core.run_for(3)
	_check(core.net_value(net) == 1, "开关置 1 后网络为 1")
	core.toggle_input(sw)
	core.run_for(3)
	_check(core.net_value(net) == 0, "开关置 0 后网络为 0")

	print("[5] 存取的往返")
	var json := String(core.save_data())
	_check(json.length() > 50, "导出 JSON（%d 字节）" % json.length())
	var core2 = LogicLab.new()
	_check(core2.load_data(json), "载入成功")
	var st = core2.stats()
	_check(int(st[0]) == 2, "载入后仍有 2 个元件")

	print("[6] 编辑")
	var g: int = core2.add_component("and", 12, 4)
	_check(g >= 0, "追加 AND")
	core2.rotate_component(g)
	var info = core2.component_info(g)
	_check(info.size() >= 8 and int(info[7]) == 90, "旋转 90° 生效")
	core2.set_width(g, 8)
	var info2 = core2.component_info(g)
	_check(int(info2[1]) == 8, "位宽改为 8")
	_check(core2.remove_component(g), "删除元件")
	var st2 = core2.stats()
	_check(int(st2[0]) == 2, "删除后剩 2 个元件")

	print("[7] 视图数据")
	var comps = core2.components()
	_check(comps.size() == 2 * 7, "components 数组长度 = 元件数 × 7")
	var pins = core2.pins()
	_check(pins.size() == 2 * 7, "pins 数组长度 = 引脚数 × 7")
	var wr = core2.wire_ranges()
	var wp = core2.wire_points()
	_check(wr.size() % 5 == 0 and wp.size() == int(wr[1]) * 2, "导线折线长度自洽")
	var bounds = core2.content_bounds()
	_check(bounds.size() == 4, "内容包围盒可用")

	print("[8] 内置示例")
	var ex_ids = core.example_ids()
	_check(ex_ids.size() >= 4, "内置示例 >= 4 个（实际 %d）" % ex_ids.size())
	_check(core.example_names().size() == ex_ids.size(), "示例名与 id 数量一致")
	for eid in ex_ids:
		var ce = LogicLab.new()
		if not ce.load_example(eid):
			_check(false, "载入示例 %s" % eid)
			continue
		var est = ce.stats()
		_check(int(est[0]) > 0 and int(est[4]) > 0, "载入示例 %s（%d 元件 %d 导线）" % [eid, int(est[0]), int(est[4])])

	print("[9] 参数编辑（控件由 core 的参数描述驱动）")
	var c4 = LogicLab.new()
	var clk: int = c4.add_component("clock", 0, 0)
	var clk_info = c4.component_info(clk)
	var ckeys = c4.param_keys(int(clk_info[0]))
	_check(ckeys.size() == 2, "时钟有 2 个参数（实际 %d）" % ckeys.size())
	_check(c4.set_param(clk, "high", 3), "设置高电平拍数")
	_check(c4.set_param(clk, "low", 2), "设置低电平拍数")
	var cvals = c4.param_values(clk)
	_check(int(cvals[0]) == 3 and int(cvals[1]) == 2, "参数值已生效")

	var reg: int = c4.add_component("register", 12, 0)
	_check(c4.set_param(reg, "width", 16), "改寄存器位宽")
	_check(c4.set_param(reg, "reset", 1), "打开复位引脚")
	var reg_info = c4.component_info(reg)
	_check(int(reg_info[1]) == 16, "位宽已改为 16")
	_check(int(reg_info[8]) == 4, "带使能/复位脚的寄存器有 4 个输入（实际 %d）" % int(reg_info[8]))
	_check(not c4.set_param(clk, "不存在的参数", 1), "未知参数被拒绝")

	print("[10] 设计规则检查")
	var c5 = LogicLab.new()
	var wide: int = c5.add_component("constant", 0, 0)
	var narrow: int = c5.add_component("not", 10, 0)
	c5.set_param(wide, "width", 8)
	c5.connect_pins(wide, 0, narrow, 0)
	var dsum = c5.drc_summary()
	_check(int(dsum[0]) >= 1, "8 位接 1 位被 DRC 抓出（错误 %d）" % int(dsum[0]))
	_check(c5.drc_details().size() == int(dsum[0]) + int(dsum[1]), "明细条数与摘要一致")

	var c6 = LogicLab.new()
	c6.load_example("half_adder")
	var dsum2 = c6.drc_summary()
	_check(int(dsum2[0]) == 0, "半加器示例没有错误（警告 %d）" % int(dsum2[1]))

	print("=== %s ===" % ("全部通过" if _fails == 0 else "%d 项失败" % _fails))
	quit(0 if _fails == 0 else 1)
