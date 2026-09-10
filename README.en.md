# LogicLab

English | [简体中文](README.md)

A digital circuit sandbox: modelled on the sandbox mode of *Turing Complete*, but built with a "half game, half professional tool" positioning and a bit more rigour.

> **⚠️ Note**: This project will remain in a testing phase for the long term, and no stable release is planned for the foreseeable future (versions advance as **Checkpoints**). Iteration may involve **breaking changes**, and **backward compatibility is not guaranteed** — after an upgrade, existing project files or circuits may stop working. Do not use it in production.

- **Pure-Rust core** + Godot 4 shell (GDExtension bridge)
- **Tick-based discrete simulation**: every component output is delayed by exactly 1 tick, so signals can be watched propagating wire by wire
- **Hierarchical encapsulation**: select a patch of circuitry and wrap it into a custom component; double-click an instance to edit its internals (the heart of TC gameplay)
- **Waveform viewer + VCD export**: hover a wire and press W to watch it; recorded traces drop straight into GTKWave
- **Virtual peripherals**: seven-segment displays and a memory-mapped dot-matrix screen (the framebuffer lives in the core, so undo/load resets it automatically)
- **Built for large designs**: ~0.4 ms per tick for 10,000 components; dirty marking means idle circuits evaluate nothing
- **x64 and ARM64**, single-file, no installation
- Ships with a headless CLI that runs simulations **without Godot**

## Quick Start

### Run the prebuilt binary

Grab the single-file exe for your architecture from [Releases](../../releases) and double-click it.

### Build from source

Prerequisites: a stable Rust toolchain, Godot 4.7.x, and MSVC (Windows).

```bash
# 1) Core and CLI (Godot is not needed at all)
cargo test
cargo run --release -p logiclab-cli -- bench      # performance benchmark at 10k scale
cargo run -p logiclab-cli -- demo                 # half-adder truth table + shift-register waveform
cargo run -p logiclab-cli -- example out.json     # generate a sample project
cargo run -p logiclab-cli -- components           # list every built-in component

# 2) GDExtension bridge
cd bridge
cargo build --release --target x86_64-pc-windows-msvc
cargo build --release --target aarch64-pc-windows-msvc
# copy the artifacts into godot/:
#   x86_64  -> godot/logiclab_bridge.dll
#   aarch64 -> godot/logiclab_bridge_arm64.dll

# 3) Run
godot --path godot
```

### Regression tests

```bash
# headless also loads the GDExtension, so this is the fastest full-stack check
godot --headless --path godot --script res://tests/smoke.gd
```

## Controls

| Action | How |
|---|---|
| Load an example | Example dropdown at the left of the top "Drawings" bar |
| Place a component / sub-circuit | Click an entry in the left library, then click the canvas; sub-circuits come from the top "Drawings" bar |
| Wire | Drag from one pin to another (auto-routed, never mis-connects) |
| Select / drag | Left-click a component / hold and drag (a multi-selection moves together) |
| Multi-select / box-select | Shift + left-click to toggle / drag a box on empty canvas |
| Flip a switch | Click the switch component |
| Rotate / delete | Press R / Delete while selected |
| Enter a sub-circuit | Double-click an instance, or press E while selected |
| Go up one level | Press B, or click the breadcrumb at the top |
| New / rename / delete drawing | Buttons in the top navigation bar |
| Encapsulate selection | Select some components → "Encapsulate selection" at the top → give it a name |
| Inspect a waveform | Hover a wire and press W; the top "Waveform" button opens the panel |
| Pan / zoom / center | Right- or middle-drag / wheel / C |
| Step / run | Space / the bottom bar (speed slider on the right) |
| Edit parameters | Select a component and edit in the right-hand panel |
| Design rule check | Button on the left (multi-driver / width / dangling / combinational loop / circular reference) |

## Architecture

```
Godot shell (GDScript)   interaction / custom-drawn rendering / panels — hard-codes no component knowledge
      ↓ method calls, flat arrays
bridge (gdext, thin)     command translation + CircuitView → PackedInt64Array
      ↓ plain Rust calls
core (zero engine deps)  data model / elaboration / tick engine / netlist derivation / serialization / view / DRC / examples
      ↓
cli (headless)           a non-Godot consumer of the very same core
```

**Hard rule: nothing in the core dependency tree is an engine type.** So `cargo test` needs no Godot,
and swapping the shell (or dropping Godot entirely) leaves the core untouched.

### core modules

| File | Responsibility |
|---|---|
| `values.rs` | Widths {1,4,8,16,32}, per-bit unknown flags, wrapping arithmetic |
| `defs.rs` | **Single source of component truth**: kinds / params / pins / evaluation / editable-param descriptors |
| `board.rs` | Editing model, netlist derivation (interval-merged index), auto-routing (grid Dijkstra), hit testing |
| `elaborate.rs` | Hierarchy elaboration: many drawings → one flat netlist; interface ordering, net merging, self-reference truncation |
| `engine.rs` | Double-buffered tick, dirty propagation, CSR adjacency |
| `view.rs` | Shell-agnostic `CircuitView` (swapping shells means rewriting one renderer) |
| `save.rs` | Project JSON + schema version (the core never touches the filesystem) |
| `drc.rs` | Design rule checks (whole project + circular references) |
| `session.rs` | The single facade for editing/simulation: undo, edit-resets-sim, drawing navigation, encapsulation, waveforms |
| `examples.rs` | Built-in example circuits (including a hierarchical one) |

## Hierarchy (custom components)

One drawing is one sub-circuit definition; the `input_pin` / `output_pin` components inside it are its interface:

1. Build and debug the circuit on the root drawing first;
2. Box-select or Shift-click that patch;
3. Hit "Encapsulate selection" and give it a name — wires crossing the boundary are **turned into interface pins and reconnected automatically**;
4. From then on it is an ordinary component: instantiate it repeatedly, or encapsulate it into something larger.

Interface pins are ordered by **component name** (ties broken by instance index), so the same drawing always
yields the same pin order. Before simulation starts, the whole hierarchy tree is compressed into a single flat
netlist: shells produce no runtime components, and shell pins merge into the same nets as the interface pins.
**The hot path has no idea hierarchy exists** — how many levels a 10,000-component design nests costs nothing per tick.

## Simulation semantics

- Every component output is **delayed by exactly 1 tick** (flip-flops included: a clock edge read at t appears at t+1)
- Wires are zero-delay; one net has one value
- Two-phase update: evaluate everything into a staging area first, then commit atomically, so results are order-independent
- An unconnected input reads as 0
- Arithmetic always wraps; the logic layer is all-integer, with no floating point

Note: **combinational feedback (an SR latch, say) simulates fine but is reported as a DRC error**. Feedback itself is
legitimate as long as a sequential element sits on the loop; a purely combinational loop corresponds to oscillation in
real hardware and is also a blocker for Verilog export, so it is treated as an error.

## Performance

| Metric | Value |
|---|---|
| 10,001 components, all active every tick | ~0.41 ms/tick |
| Single edit on a 10k circuit (including elaboration and engine rebuild) | ~13.5 ms |
| Components evaluated per tick on an idle circuit | 0 |

## Roadmap

Work follows the milestones of [spec v4](docs/logic-sandbox-spec-v4.md); item-by-item status lives in [docs/ROADMAP.md](docs/ROADMAP.md).

| Milestone | Scope | Status |
|---|---|---|
| **M0** Pure-Rust core | Data model / tick engine / netlist derivation / save files / undo / CLI | ✅ Done |
| **M1** Minimal Godot shell | Editing interaction / custom-drawn rendering / speed control / annotations | ✅ Done |
| **M2** Full editor + complete library | All components / encapsulation / net labels / multiple drawings / DRC / 10k-scale performance | ✅ Done |
| **M3** Professional quartet + checkpoint | Waveforms and VCD ✅ · critical path ✅ · testbench ⬜ · CLI regression ⬜ · checkpoint ⬜ | 🚧 In progress |
| **M4** Verilog export | Structured export / single-clock constraint / peripheral black boxes; acceptance = iverilog simulation + Yosys synthesis both pass | ⬜ Planned |
| **M5** Machine building and peripherals | Keyboard / UART / assembler + ISA / ROM flashing / reference CPU tutorial | ⬜ Planned |
| **M6+** | FSM editor, tri-state buses, truth-table round-trip, SVG export, i18n | ⬜ Later |

The end goal is to tick all seven acceptance criteria of spec §15. The two sharpest are: **a self-built CPU + UART
running a real program with conditional jumps**, and **any exported Verilog being accepted by both iverilog and Yosys**.

## On AI-assisted development

This project makes **heavy use of AI-assisted development**, and the division of labour is stated plainly:

| Stage | Led by |
|---|---|
| Product positioning and spec | A human (performance first, ARM64 must run, TC sandbox as the target to match, the Checkpoint versioning scheme) |
| Architecture and decisions | Discussed between human and AI, decided by the human (core and shell fully separated, edit-resets-simulation, elaboration kept on the editing side) |
| Implementation | An AI coding agent (Rust core, GDExtension bridge, GDScript shell, tests and docs) |
| Acceptance | Executable tests and benchmarks (core unit tests, headless smoke tests, performance benchmark all green) |

There is no intention to dress this up as something else. What makes a project in its testing phase worth a look
is not who typed the code, but whether its tests go green and whether its numbers reproduce — both are in this
repository, ready to be checked by anyone.

## License

[MIT](LICENSE) © 2026 Orlean-Crispy
