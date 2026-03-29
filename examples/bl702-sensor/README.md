# BL702 Zigbee Temperature Sensor

A `no_std` Zigbee 3.0 end device firmware for the **Bouffalo Lab BL702** (RISC-V),
reporting temperature and humidity via ZCL clusters 0x0402 and 0x0405.

## Hardware

- **MCU:** BL702 — RISC-V 32-bit, 128KB SRAM, 512KB Flash
- **Radio:** Built-in IEEE 802.15.4 + BLE 5.0
- **Boards:** XT-ZB1, DT-BL10, Pine64 Pinenut, BL706 devboard
- **Button:** GPIO8 (boot button on most modules) — join/leave network
- **Debug:** UART0 serial logging

## Prerequisites

- Rust nightly with `riscv32imac-unknown-none-elf` target
- `cargo install cargo-binutils` (for `cargo objcopy`)

```bash
rustup target add riscv32imac-unknown-none-elf
```

## Vendor Library Setup

The BL702 802.15.4 radio driver uses FFI bindings to two precompiled C libraries
from the **Bouffalo IoT SDK** (`bl_iot_sdk`):

| Library          | Purpose                      |
|------------------|------------------------------|
| `liblmac154.a`   | IEEE 802.15.4 MAC layer      |
| `libbl702_rf.a`  | RF PHY / calibration         |

There are **three ways** to provide these libraries (checked in priority order by `build.rs`):

### Option 1: Full SDK path (`BL_IOT_SDK_DIR`)

Clone the Bouffalo IoT SDK and point to it:

```bash
git clone https://github.com/bouffalolab/bl_iot_sdk.git
export BL_IOT_SDK_DIR=/path/to/bl_iot_sdk
```

The build script auto-derives library paths:
- `$BL_IOT_SDK_DIR/components/network/lmac154/lib/`
- `$BL_IOT_SDK_DIR/components/platform/soc/bl702/bl702_rf/lib/`

### Option 2: Explicit library paths

Point directly to each library directory:

```bash
export LMAC154_LIB_DIR=/path/to/dir/containing/liblmac154.a
export BL702_RF_LIB_DIR=/path/to/dir/containing/libbl702_rf.a
```

### Option 3: Local `vendor_libs/` directory

Copy ABI-patched `.a` files into the project:

```bash
mkdir vendor_libs/
cp /path/to/liblmac154.a vendor_libs/
cp /path/to/libbl702_rf.a vendor_libs/
```

### ⚠️ Float ABI Mismatch

The vendor `.a` files are compiled with **rv32imfc/ilp32f** (hard-float ABI).
Rust targets **riscv32imac/ilp32** (soft-float). You must strip the ELF float-ABI
flag before linking:

```bash
python3 scripts/strip_float_abi.py liblmac154.a vendor_libs/liblmac154.a
python3 scripts/strip_float_abi.py libbl702_rf.a vendor_libs/libbl702_rf.a
```

## Building

### Stubs build (CI — no vendor libs required)

```bash
cd examples/bl702-sensor
cargo build --release --features stubs
```

The `stubs` feature provides no-op implementations of all FFI symbols, allowing
the project to compile without any vendor libraries.

### Real build (with vendor libraries)

```bash
cd examples/bl702-sensor
LMAC154_LIB_DIR=/path/to/lib cargo build --release
```

## Flashing

Use the Bouffalo `bflb-iot-tool` or BLDevCube:

```bash
bflb-iot-tool --chipname bl702 --firmware target/riscv32imac-unknown-none-elf/release/bl702-sensor
```

## What It Demonstrates

- Zigbee 3.0 end device on BL702 with Embassy async runtime
- Custom Embassy time driver using BL702 TIMER_CH0 (1 MHz tick)
- IEEE 802.15.4 radio via `lmac154` FFI bindings
- UART-based logging
- Button-driven network join/leave with edge detection
- ZCL Temperature Measurement + Relative Humidity clusters

## Project Structure

```
bl702-sensor/
├── .cargo/config.toml   # Target: riscv32imac-unknown-none-elf, build-std
├── .gitignore            # Excludes lib/ and vendor_libs/
├── Cargo.toml            # Dependencies, stubs feature flag
├── build.rs              # Vendor lib linking (3 fallback methods)
├── memory.x              # Flash @ 0x23000000, RAM @ 0x42014000
├── vendor_libs/          # (gitignored) ABI-patched .a files
└── src/
    ├── main.rs           # Entry point, device setup, sensor loop
    ├── hal.rs            # HAL impls (delay, IRQ, GPIO, memcpy)
    └── stubs.rs          # No-op FFI stubs for CI builds
```
