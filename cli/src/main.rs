//! LogicLab headless 命令行（v3 §3.2）
//!
//! 这个 crate 的存在本身就是「core 可以脱离 Godot 存活」的证明：
//! 它的依赖树里没有任何引擎类型（ADR-21）。
//! 文件读写只发生在这里——core 本身不碰文件系统，JSON 以 &str 进出。

use std::process::ExitCode;
use std::time::Instant;

use logiclab_core::{
    format_value, Board, Category, DefId, Engine, NetValue, Params, Point, Project, Session,
    NO_NET, SCHEMA_VERSION,
};

const USAGE: &str = "\
LogicLab —— 数字电路沙盒（headless 命令行）

用法:
  logiclab components                列出全部内置组件
  logiclab schema                    打印工程格式版本
  logiclab demo                      跑内置冒烟演示
  logiclab example <输出.json>        生成一份示例工程（可再用 info / run 打开）
  logiclab bench                     跑万级规模性能基准
  logiclab info <工程.json>           打印工程概况
  logiclab run <工程.json> [选项]     载入并快进仿真
       -n <拍数>                     仿真拍数（默认 100）
       -v                            逐拍打印前 32 拍的探针波形
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    };
    let rest = &args[1..];
    let result = match cmd {
        "components" => cmd_components(),
        "schema" => cmd_schema(),
        "demo" => cmd_demo(),
        "example" => cmd_example(rest),
        "bench" => cmd_bench(),
        "info" => cmd_info(rest),
        "run" => cmd_run(rest),
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("未知命令: {other}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("错误: {msg}\n");
            eprint!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// 元信息
// ---------------------------------------------------------------------------

fn cmd_components() -> Result<(), String> {
    println!("内置组件（共 {} 种）\n", DefId::ALL.len());
    let cats = [
        Category::Io,
        Category::Logic,
        Category::Routing,
        Category::Arithmetic,
        Category::Memory,
    ];
    for cat in cats {
        println!("【{}】", cat.label());
        for &d in DefId::ALL.iter().filter(|d| d.category() == cat) {
            let p = d.default_params();
            println!(
                "  {:<12} {:<14} 位宽 {:<2} 入 {} 出 {}",
                d.id(),
                d.label(),
                p.width,
                d.in_count(&p),
                d.out_count(&p)
            );
        }
        println!();
    }
    Ok(())
}

fn cmd_schema() -> Result<(), String> {
    println!("工程格式版本: {SCHEMA_VERSION}");
    Ok(())
}

// ---------------------------------------------------------------------------
// 工程查看 / 仿真
// ---------------------------------------------------------------------------

fn load(path: &str) -> Result<Project, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取 {path} 失败: {e}"))?;
    Project::from_json(&text).map_err(|e| e.to_string())
}

fn cmd_info(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("用法: logiclab info <工程.json>")?;
    let project = load(path)?;
    println!("工程: {}", project.name);
    println!("格式版本: {}", project.schema_version);
    println!("图纸数: {}", project.boards.len());
    for (i, b) in project.boards.iter().enumerate() {
        let nl = b.compile();
        println!(
            "  图纸 {i}{}: {} 实例 / {} 导线 / {} 引脚 / {} 网络",
            if i == project.main_board { "（主）" } else { "" },
            b.instances.len(),
            b.wires.len(),
            b.pin_total(),
            nl.net_count
        );
    }
    Ok(())
}

fn cmd_run(args: &[String]) -> Result<(), String> {
    let mut path: Option<&str> = None;
    let mut ticks: u64 = 100;
    let mut verbose = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-n" => {
                i += 1;
                let v = args.get(i).ok_or("-n 需要一个拍数")?;
                ticks = v.parse().map_err(|_| format!("-n 需要整数，收到 {v}"))?;
            }
            "-v" => verbose = true,
            other => path = Some(other),
        }
        i += 1;
    }
    let path = path.ok_or("用法: logiclab run <工程.json> [-n 拍数] [-v]")?;
    let project = load(path)?;
    let board = project.main().ok_or("工程里没有图纸")?;
    let nl = board.compile();

    let mut e = Engine::new();
    e.load_board(board);

    if verbose {
        println!("  tick  求值组件数");
    }
    let watch = ticks.min(32);
    for t in 0..ticks {
        e.tick();
        if verbose && t < watch {
            println!("  {:>4}  {:>6}", e.tick_count() - 1, e.last_eval_count());
        }
    }

    println!("工程: {}", project.name);
    println!("快进 {ticks} 拍 → 当前 tick {}", e.tick_count());
    println!(
        "  {} 实例 / {} 组件 / {} 网络 / {} 引脚",
        board.instances.len(),
        e.component_count(),
        e.net_count(),
        e.pin_count()
    );
    println!("  末拍求值组件数: {}", e.last_eval_count());

    let mut probes = 0;
    for (idx, inst) in board.instances.iter().enumerate() {
        if inst.def != DefId::Led {
            continue;
        }
        let pin = nl.pin_start[idx];
        let net = nl.pin_net[pin as usize];
        let v = if net == NO_NET { NetValue::ZERO } else { e.net_value(net) };
        println!(
            "  探针 #{idx} = {}",
            format_value(v, inst.params.width, false)
        );
        probes += 1;
    }
    if probes == 0 {
        println!("  （电路里没有探针组件）");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 内置演示（M0 退出标准：示例程序跑 tick 并输出波形）
// ---------------------------------------------------------------------------

fn connect(b: &mut Board, from: (u32, usize), to: (u32, usize)) {
    assert!(b.connect_pins(from, to), "连线失败: {from:?} -> {to:?}");
}

fn cmd_demo() -> Result<(), String> {
    println!("LogicLab core 冒烟演示\n");
    half_adder_table();
    println!();
    shift_register_wave();
    Ok(())
}

/// NAND 半加器：S = A XOR B（4 个 NAND），C = A AND B（1 个 NAND）。
/// 同一个引脚的扇出、以及多条线共走一列，都是自动走线必须处理的情形。
struct HalfAdder {
    board: Board,
    sw_a: u32,
    sw_b: u32,
    xor: u32,
    and_gate: u32,
}

fn half_adder_board() -> HalfAdder {
    let mut b = Board::new();
    let sw_a = b.add_instance(DefId::Switch, Params::default().width(1), 0, 0);
    let sw_b = b.add_instance(DefId::Switch, Params::default().width(1), 0, 6);
    let nand2 = || Params::default().width(1).inputs(2);
    let n1 = b.add_instance(DefId::Nand, nand2(), 6, 0);
    let n2 = b.add_instance(DefId::Nand, nand2(), 6, 3);
    let n3 = b.add_instance(DefId::Nand, nand2(), 6, 6);
    let xor = b.add_instance(DefId::Nand, nand2(), 12, 2);
    let and_gate = b.add_instance(DefId::Nand, nand2(), 12, 8);

    connect(&mut b, (sw_a, 0), (n1, 0));
    connect(&mut b, (sw_b, 0), (n1, 1));
    connect(&mut b, (sw_a, 0), (n2, 0));
    connect(&mut b, (n1, 2), (n2, 1));
    connect(&mut b, (sw_b, 0), (n3, 0));
    connect(&mut b, (n1, 2), (n3, 1));
    connect(&mut b, (n2, 2), (xor, 0));
    connect(&mut b, (n3, 2), (xor, 1));
    connect(&mut b, (n1, 2), (and_gate, 0));
    connect(&mut b, (n1, 2), (and_gate, 1));

    HalfAdder { board: b, sw_a, sw_b, xor, and_gate }
}

fn half_adder_table() {
    let ha = half_adder_board();
    let mut e = Engine::new();
    e.load_board(&ha.board);
    println!("NAND 半加器真值表（S 由 4 个 NAND 构成，C 由 1 个 NAND 构成）");
    println!("  A  B |  S  C   期望");
    let mut all_ok = true;
    for (a, bb) in [(0u32, 0u32), (0, 1), (1, 0), (1, 1)] {
        e.set_input(ha.sw_a, a);
        e.set_input(ha.sw_b, bb);
        e.run_for(6);
        let s = e.instance_output(ha.xor, 0).get(1);
        let c = e.instance_output(ha.and_gate, 0).get(1);
        let ok = s == (a ^ bb) && c == (a & bb);
        all_ok &= ok;
        println!(
            "  {a}  {bb} |  {s}  {c}   S={} C={}  {}",
            a ^ bb,
            a & bb,
            if ok { "OK" } else { "FAIL" }
        );
    }
    if !all_ok {
        eprintln!("半加器真值表不匹配！");
    }
}

/// 生成示例工程文件。顺带验证「保存 → 载入 → 仿真」这条完整链路。
fn cmd_example(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("用法: logiclab example <输出.json>")?;
    let ha = half_adder_board();
    let mut project = Project::new("示例：NAND 半加器");
    project.add_board(ha.board);
    let json = project.to_json_pretty().map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("写入 {path} 失败: {e}"))?;
    println!("已生成示例工程: {path}");
    Ok(())
}

/// 两级 D 触发器串联 → 移位寄存器，打印逐拍波形
fn shift_register_wave() {
    let mut b = Board::new();
    let clk = b.add_instance(DefId::Clock, Params::default().width(1), 0, 0);
    let d = b.add_instance(DefId::Switch, Params::default().width(1), 0, 5);
    let r1 = b.add_instance(DefId::Register, Params::default().width(1).opts(0), 4, 0);
    let r2 = b.add_instance(DefId::Register, Params::default().width(1).opts(0), 8, 0);
    connect(&mut b, (clk, 0), (r1, 1));
    connect(&mut b, (d, 0), (r1, 0));
    connect(&mut b, (clk, 0), (r2, 1));
    connect(&mut b, (r1, 2), (r2, 0));

    let mut e = Engine::new();
    e.load_board(&b);
    println!("两级 D 触发器移位寄存器（每个时钟上升沿推进一级）");
    println!("  tick  clk  D   Q1  Q2   求值");
    for t in 0..20 {
        if t == 2 {
            e.set_input(d, 1);
        }
        if t == 12 {
            e.set_input(d, 0);
        }
        e.tick();
        println!(
            "  {:>4}   {:>1}   {:>1}   {:>1}   {:>1}   {:>4}",
            e.tick_count() - 1,
            e.instance_output(clk, 0).get(1),
            e.instance_output(d, 0).get(1),
            e.instance_output(r1, 0).get(1),
            e.instance_output(r2, 0).get(1),
            e.last_eval_count()
        );
    }
}

// ---------------------------------------------------------------------------
// 规模基准
// ---------------------------------------------------------------------------

fn cmd_bench() -> Result<(), String> {
    const N: i32 = 10_000;
    let mut s = Session::new();

    // 用一次批量编辑构造整块电路：逐个放置会触发 N 次重建
    let t0 = Instant::now();
    s.edit_batch(|b| {
        b.add_instance(DefId::Clock, Params::default().width(1), 0, 0);
        // 一路时钟总线灌进一万个计数器：每个时钟沿全部翻转，脏标记在此毫无帮助，
        // 量的就是引擎本身的上限。计数器纵向排开，CLK 正好落在竖直总线上。
        b.add_wire(vec![Point::new(2, 0), Point::new(4, 0), Point::new(4, N - 1)]);
        for i in 0..N {
            b.add_instance(DefId::Counter, Params::default().width(8).opts(0), 4, i);
        }
    });
    let build_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t1 = Instant::now();
    s.sim_run_for(1000);
    let sim_ms = t1.elapsed().as_secs_f64() * 1000.0;
    // 必须在做编辑之前读——编辑会把仿真重置，读数就归零了
    let evals_per_tick = s.engine().last_eval_count();

    // 单次编辑（放置一个元件）= 快照 + 网表推导 + 重建引擎与视图
    let t2 = Instant::now();
    s.add_component(DefId::Not, 0, 40);
    let edit_ms = t2.elapsed().as_secs_f64() * 1000.0;

    // 撤销 = 换回快照 + 重建
    let t3 = Instant::now();
    s.undo();
    let undo_ms = t3.elapsed().as_secs_f64() * 1000.0;

    println!("规模基准：{N} 个计数器共用一个时钟（每拍全部活动，脏标记最无力的情况）");
    println!("  批量构造（含一次装载）  {build_ms:>8.1} ms");
    println!("  1000 拍仿真            {sim_ms:>8.1} ms  →  {:.3} ms/拍", sim_ms / 1000.0);
    println!("  每拍求值 {evals_per_tick} 个组件");
    println!("  单次编辑               {edit_ms:>8.1} ms");
    println!("  撤销一步               {undo_ms:>8.1} ms");
    if sim_ms / 1000.0 >= 5.0 {
        eprintln!("性能未达规格（要求万级元件 < 5 ms/拍）");
    }
    Ok(())
}
