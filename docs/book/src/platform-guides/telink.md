# Telink B91 & TLSR8258

Telink's B91 (RISC-V) and TLSR8258 (tc32 ISA) are popular SoCs in commercial
Zigbee products. The zigbee-rs Telink backend supports both chips through a
single `TelinkMac` driver.

**TLSR8258** uses a **pure-Rust** 802.15.4 radio driver — all 12 radio
functions use direct register access with no vendor blob or FFI. This makes
TLSR8258 the second pure-Rust radio platform in zigbee-rs (after PHY6222).

**B91** still uses FFI bindings to the Telink driver library (`libdrivers_b91.a`)
for radio access.

## Hardware Overview

### Telink B91 (TLSR9218)

| Spec | Value |
|------|-------|
| **Core** | RISC-V 32-bit, up to 96 MHz |
| **Flash** | 512 KB |
| **SRAM** | 256 KB |
| **Radio** | BLE 5.0 + IEEE 802.15.4 |
| **Target** | `riscv32imc-unknown-none-elf` |
| **I/O** | UART ×2, SPI, I2C, ADC, PWM, USB |

### Telink TLSR8258

| Spec | Value |
|------|-------|
| **Core** | tc32 (Telink custom ISA) |
| **Flash** | 512 KB |
| **SRAM** | 64 KB |
| **Radio** | BLE + IEEE 802.15.4 |
| **Cargo target** | `thumbv6m-none-eabi` (stand-in for tc32) |
| **Real toolchain** | [modern-tc32](https://github.com/modern-tc32) (custom Rust + LLVM for tc32) |
| **Radio driver** | 🦀 **Pure Rust** — direct register access, no vendor library |

> The TLSR8258 uses Telink's proprietary **tc32** instruction set. For
> `cargo check`/`cargo build`, we use `thumbv6m-none-eabi` as a compilation
> stand-in. Real production builds use the
> [modern-tc32](https://github.com/modern-tc32) toolchain, which provides a
> custom Rust compiler with native `tc32-unknown-none-elf` target support.

### Common Products Using These Chips

- **TLSR8258:** Sonoff SNZB-02/SNZB-03/SNZB-04, many Tuya Zigbee sensors,
  IKEA TRÅDFRI devices
- **B91:** Next-generation Telink Zigbee 3.0 modules, TL321x/TL721x variants

### Memory Maps

**B91:**
```
FLASH : ORIGIN = 0x20000000, LENGTH = 512K
RAM   : ORIGIN = 0x00000000, LENGTH = 256K
```

**TLSR8258:**
```
FLASH : ORIGIN = 0x00000000, LENGTH = 512K
RAM   : ORIGIN = 0x00840000, LENGTH = 64K
```

## Current Status

### TLSR8258 — Pure Rust Radio ✅

The TLSR8258 backend is **fully functional** with a pure-Rust 802.15.4 radio
driver. All radio control is done via volatile memory-mapped register access
at `0x800000+` — no `libdrivers_8258.a` or vendor SDK needed.

The pure-Rust driver replaces all 12 FFI functions that were previously
stubbed:

| Function | Implementation |
|----------|---------------|
| Channel set | Direct RF frequency register write |
| TX power | PA register lookup table |
| TX/RX | DMA-based with hardware packet format |
| CCA | RSSI measurement via RF status register |
| ED scan | Energy detection via RSSI averaging |
| IRQ handling | RF IRQ mask/status registers |
| Radio sleep | Disable RF + DMA + IRQ (~5-8 mA saved) |
| CPU suspend | Timer-wake suspend mode (~3 µA) |

**Power management** is integrated into the driver:
- **Radio sleep** — disables RF transceiver, DMA channels, and RF IRQs
  between polls, saving ~5-8 mA
- **CPU suspend** — enters tc32 suspend mode with timer wake, drawing only
  ~3 µA (vs ~1.5 mA for WFI idle)
- **Two-tier sleep** — WFI for fast poll intervals, CPU suspend for slow
  poll intervals (identical architecture to PHY6222's AON sleep)

### B91 — FFI Stubs

> ⚡ The B91 backend compiles and produces valid RISC-V machine code. Real RF
> operation requires linking `libdrivers_b91.a` from the Telink Zigbee SDK.

The B91 backend is architecturally complete:
- Full `MacDriver` trait implementation with CSMA-CA, ED scan, indirect TX queue
- FFI bindings to Telink RF driver library
- Frame construction, PIB management, frame-pending bit for SED support
- Real time drivers reading hardware system timer at 0x140200
- GPIO register-mapped I/O, RF ISR routing, WFI-based sleep
- Example firmware with GPIO, LED, button handling, and sensor reporting

What's needed for B91 real RF operation:
- Telink Zigbee SDK (`tl_zigbee_sdk`)
- Driver library (`libdrivers_b91.a`)

## Prerequisites

### Rust Toolchain

**For B91:**
```bash
rustup default nightly
rustup update nightly
rustup target add riscv32imc-unknown-none-elf
rustup component add rust-src
```

**For TLSR8258 (CI builds):**
```bash
rustup default nightly
rustup update nightly
rustup target add thumbv6m-none-eabi
rustup component add rust-src
```

**For TLSR8258 (real tc32 firmware):**

Install the [modern-tc32](https://github.com/modern-tc32/examples_rust) toolchain,
which provides a custom Rust compiler with native `tc32-unknown-none-elf` target.

### Telink SDK (for B91 real RF)

Download the [Telink Zigbee SDK](http://wiki.telink-semi.cn/wiki/IDE-and-Tools/)
(only needed for B91 — TLSR8258 uses pure-Rust radio driver):

```bash
export TELINK_SDK_DIR=/path/to/tl_zigbee_sdk
```

## Building

### Telink B91

**With stubs (CI mode):**
```bash
cd examples/telink-b91-sensor
cargo build --release --features stubs
```

**With Telink SDK (real radio):**
```bash
cd examples/telink-b91-sensor
TELINK_SDK_DIR=/path/to/sdk cargo build --release
```

### Telink TLSR8258

**CI build (thumbv6m stand-in — no tc32 toolchain needed):**
```bash
cd examples/telink-tlsr8258-sensor
cargo build --release
```

The radio driver uses pure-Rust register access, so no FFI stubs or vendor
libraries are needed. This verifies the Rust code compiles but does NOT
produce flashable tc32 firmware.

**Real tc32 firmware (with modern-tc32 toolchain):**
```bash
# Install the TC32 Rust toolchain
# See: https://github.com/modern-tc32/examples_rust

export TC32_TOOLCHAIN=/path/to/toolchains/tc32-stage1
export TC32_SDK_DIR=/path/to/tl_zigbee_sdk
export TC32_LLVM_BIN=$TC32_TOOLCHAIN/llvm/bin

cd examples/telink-tlsr8258-sensor
$TC32_TOOLCHAIN/bin/cargo build --release
```

This produces a real `tc32-unknown-none-elf` binary flashable to TLSR8258
hardware. The `build.rs` automatically compiles Telink SDK C sources with
`clang --target=tc32` and links `libsoft-fp.a`.

### CI Build Commands

From `.github/workflows/ci.yml`:

**B91:**
```bash
# Toolchain: nightly with riscv32imc-unknown-none-elf + rust-src + llvm-tools
cd examples/telink-b91-sensor
cargo build --release --features stubs

# Firmware artifacts
OBJCOPY=$(find $(rustc --print sysroot) -name llvm-objcopy | head -1)
$OBJCOPY -O binary $ELF ${ELF}.bin
$OBJCOPY -O ihex   $ELF ${ELF}.hex
```

**TLSR8258:**
```bash
# Toolchain: nightly with thumbv6m-none-eabi + rust-src + llvm-tools
cd examples/telink-tlsr8258-sensor
cargo build --release    # no --features stubs needed!

OBJCOPY=$(find $(rustc --print sysroot) -name llvm-objcopy | head -1)
$OBJCOPY -O binary $ELF ${ELF}.bin
$OBJCOPY -O ihex   $ELF ${ELF}.hex
```

### Build Scripts

**B91 `build.rs`:**
```rust
// Links libdrivers_b91.a when TELINK_SDK_DIR is set
if let Ok(sdk_dir) = std::env::var("TELINK_SDK_DIR") {
    let lib_path = format!("{}/platform/lib", sdk_dir);
    println!("cargo:rustc-link-search=native={}", lib_path);
    println!("cargo:rustc-link-lib=static=drivers_b91");
}
```

**TLSR8258 `build.rs`:**
```rust
// In CI mode (thumbv6m stand-in): just links memory.x
// In modern-tc32 mode: compiles Telink SDK C sources with clang --target=tc32,
// links libsoft-fp.a from the SDK.
// No libdrivers_8258.a needed — radio uses pure-Rust register access.
```

### `.cargo/config.toml`

**B91:**
```toml
[build]
target = "riscv32imc-unknown-none-elf"

[unstable]
build-std = ["core", "alloc"]
```

**TLSR8258 (CI):**
```toml
[build]
# tc32 stand-in — real builds use modern-tc32 toolchain
target = "thumbv6m-none-eabi"

[unstable]
build-std = ["core", "alloc"]
```

> With modern-tc32, the real target `tc32-unknown-none-elf` is used instead.

## Flashing

### B91 — Telink BDT (Burning & Debug Tool)

1. Connect via Telink's Swire debug interface
2. Use the Telink BDT GUI to flash the `.bin` file
3. Alternatively, use Telink's command-line `tl_check_fw` + `tl_bulk_pgm` tools

### TLSR8258 — Telink BDT or OTA

For commercial products (Sonoff SNZB-02 etc.), OTA updates through Zigbee
are the typical approach. For development:

1. Use Telink BDT via Swire debug pins
2. Flash the `.bin` to address 0x0000

### J-Link (B91 only)

Some B91 development boards support SWD debug via J-Link:

```bash
# If supported by your board:
probe-rs run --chip TLSR9218 target/riscv32imc-unknown-none-elf/release/telink-b91-sensor
```

## MAC Backend Notes

Both B91 and TLSR8258 share a single MAC backend in `zigbee-mac/src/telink/`:

```
zigbee-mac/src/telink/
├── mod.rs      # TelinkMac struct, MacDriver trait impl
└── driver.rs   # TelinkDriver — pure-Rust register access (TLSR8258) / FFI (B91)
```

### Feature Flag

```toml
# Same feature for both B91 and TLSR8258
zigbee-mac = { features = ["telink"] }
```

### Architecture

```
MacDriver trait methods
       │
       ▼
TelinkMac (mod.rs)
  ├── PIB state (addresses, channel, config)
  ├── Frame construction
  └── TelinkDriver (driver.rs)
         ├── Direct register access (TLSR8258) — volatile MMIO at 0x800000+
         │     ├── RF frequency/channel/power registers
         │     ├── DMA-based TX/RX with hardware packet format
         │     ├── CCA via RSSI, ED scan, LQI
         │     └── Radio sleep (disable RF+DMA+IRQ) / CPU suspend (~3 µA)
         ├── FFI → tl_zigbee_sdk MAC PHY (B91 only)
         ├── TX completion: rf_tx_irq_handler() → TX_SIGNAL
         └── RX completion: rf_rx_irq_handler() → RX_SIGNAL
```

### Packet Format

**TX buffer layout:**
```
[0..3]  dmaLen   (u32, LE — DMA header)
[4]     rfLen    (payload length + 2 for CRC)
[5..]   payload  (802.15.4 MAC frame)
```

**RX buffer layout:**
```
[0..3]  dmaLen      (u32, DMA transfer length)
[4]     rssi        (raw RSSI byte)
[5..11] reserved    (7 bytes)
[12]    payloadLen  (802.15.4 PSDU length)
[13..]  payload     (MAC frame)
```

### Radio Features

- 2.4 GHz IEEE 802.15.4 compliant
- Hardware CRC generation and checking
- Configurable TX power (chip-dependent power table)
- RSSI / LQI measurement
- Energy Detection (ED) scan
- CCA (Clear Channel Assessment) with configurable threshold
- DMA-based TX/RX with hardware packet format

## Example Walkthrough

### B91 Sensor

The `telink-b91-sensor` example is a Zigbee 3.0 end device for the B91
development board with GPIO-based button and LED control.

**Pin assignments (B91 devboard):**
- GPIO2 — Button (join/leave)
- GPIO3 — Green LED
- GPIO4 — Blue LED

**Device setup:**
```rust
let mac = TelinkMac::new();

let mut device = ZigbeeDevice::builder(mac)
    .device_type(DeviceType::EndDevice)
    .manufacturer("Zigbee-RS")
    .model("B91-Sensor")
    .endpoint(1, PROFILE_HOME_AUTOMATION, 0x0302, |ep| {
        ep.cluster_server(0x0000)
            .cluster_server(0x0402)
            .cluster_server(0x0405)
    })
    .build();
```

### TLSR8258 Sensor

The `telink-tlsr8258-sensor` example targets TLSR8258-based products (Sonoff
SNZB-02 etc.). It uses the **pure-Rust radio driver** — no vendor SDK or FFI
stubs are needed. The code structure is similar to the B91 example, but the
TLSR8258 version includes integrated power management:

- **Radio sleep** between polls (disable RF + DMA + IRQ, ~5-8 mA saved)
- **CPU suspend** during slow poll intervals (~3 µA with timer wake)
- **Two-tier sleep**: WFI for fast polls, CPU suspend for slow polls

**Time driver note:** Both examples include a working Embassy time driver
that reads the hardware system timer (TLSR8258: register 0x740, B91: register
0x140200). The 32-bit timer is extended to 64-bit with wraparound detection.
The `schedule_wake()` alarm is not yet wired to a hardware compare interrupt,
so Embassy uses polling mode.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Linker error: undefined `rf_*` | Telink SDK not linked (B91 only) | Set `TELINK_SDK_DIR` or use `--features stubs` |
| `portable-atomic` errors | Missing feature flag | Ensure `features = ["unsafe-assume-single-core"]` |
| TLSR8258 real build fails | modern-tc32 toolchain needed | Install from [modern-tc32](https://github.com/modern-tc32/examples_rust) |
| B91 wrong target | Using `riscv32imac` | B91 CI uses `riscv32imc-unknown-none-elf` (no atomics) |
| No debug output | No logger registered | Use Telink UART or BDT for debug output |
| BDT can't connect | Swire not connected | Check debug interface wiring |

## Roadmap

To bring the B91 backend to full RF operation:

1. ~~**Embassy time driver** — implement using Telink system timer~~ ✅
2. **Link real SDK** — test B91 with `tl_zigbee_sdk` driver libraries
3. ~~**Interrupt wiring** — connect RF IRQ handler to Embassy signals~~ ✅
4. **B91 HAL crate** — community `embassy-telink-b91` effort
5. ~~**TLSR8258 Rust target** — explore custom target JSON for tc32 ISA~~ ✅
6. ~~**TLSR8258 pure-Rust radio** — replace all FFI with register access~~ ✅
7. ~~**TLSR8258 power management** — radio sleep + CPU suspend~~ ✅
8. ~~**modern-tc32 toolchain** — real tc32 builds with custom Rust compiler~~ ✅

## Building for Real TLSR8258 Hardware

### modern-tc32 Toolchain

The [modern-tc32](https://github.com/modern-tc32) project provides a complete
Rust toolchain for native tc32 builds:

- **Custom Rust compiler** with `tc32-unknown-none-elf` target
- **LLVM backend** with TC32 support (`clang --target=tc32`)
- **Prebuilt `core`/`alloc`** for the TC32 target

Setup: see [modern-tc32/examples_rust](https://github.com/modern-tc32/examples_rust)

### tc32 ISA Compatibility Discovery

Through binary analysis, we discovered that **tc32 is Thumb-1 with Telink
extensions**:

- ~92% of tc32 instructions have **identical binary encoding** to ARM Thumb-1
- The ~8% tc32-only opcodes (`tmcsr`, `tmrss`, `treti`) are used only in
  startup assembly, IRQ entry/exit, and power management — not in application code
- Rust/LLVM `thumbv6m` codegen produces **100% valid tc32 machine code**
  (verified: 1720 instructions, 0 unknown opcodes)

This means Rust can produce native TLSR8258 firmware.

### Custom Target Spec

A custom target JSON is provided at `targets/tc32-none-eabi.json`. It uses
the `thumbv6m` LLVM backend but overrides the linker to `tc32-elf-ld`.

With the modern-tc32 toolchain, you can build directly:

```bash
cd examples/telink-tlsr8258-sensor
$TC32_TOOLCHAIN/bin/cargo build --release
```

Or use the legacy custom target approach:

```bash
# Build with the custom tc32 target (requires tc32-elf-ld in PATH)
cd examples/telink-tlsr8258-sensor
cargo +nightly build --release \
    --target ../../targets/tc32-none-eabi.json \
    -Z build-std=core,alloc -Z json-target-spec
```

### Build with modern-tc32

The recommended approach for real TLSR8258 firmware:

```bash
# Install modern-tc32 toolchain
# See: https://github.com/modern-tc32/examples_rust

export TC32_TOOLCHAIN=/path/to/toolchains/tc32-stage1
export TC32_SDK_DIR=/path/to/tl_zigbee_sdk
export TC32_LLVM_BIN=$TC32_TOOLCHAIN/llvm/bin

cd examples/telink-tlsr8258-sensor
$TC32_TOOLCHAIN/bin/cargo build --release
```

The `build.rs` automatically:
1. Compiles Telink SDK C sources with `clang --target=tc32`
2. Links `libsoft-fp.a` from the SDK
3. Handles startup code and linker script
4. Creates a flashable binary

### Legacy: Build Script

A helper script `build-tc32.sh` is also available for manual builds:
1. Compiles Rust code with the tc32 target
2. Assembles tc32 startup code (`cstartup_8258.S`)
3. Links everything with `tc32-elf-ld`
4. Creates a flashable `.bin` with `tc32-elf-objcopy`

```bash
cd examples/telink-tlsr8258-sensor
TELINK_SDK_DIR=/path/to/tl_zigbee_sdk ./build-tc32.sh
```

### Prerequisites

- [modern-tc32 toolchain](https://github.com/modern-tc32/examples_rust) (recommended)
  **or** Telink tc32-elf-gcc toolchain (from Telink IDE or SDK)
- `TC32_SDK_DIR` environment variable pointing to `tl_zigbee_sdk`
  (only needed for soft-float math library and startup code)
- Rust nightly with `rust-src` component (for legacy build path only)

### Alternative: Static Library Approach

If you prefer to integrate Rust into an existing Telink C project:

```bash
# Build Rust as a static library
cargo +nightly build --release --target thumbv6m-none-eabi \
    -Z build-std=core,alloc --crate-type staticlib
```

Then link the resulting `.a` into your tc32-gcc C project. The C side
handles hardware initialization and calls `zigbee_init()` / `zigbee_tick()`
from the Rust library.
