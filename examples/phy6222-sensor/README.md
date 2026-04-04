# PHY6222 Zigbee Temperature Sensor — Pure Rust Radio!

A `no_std` Zigbee 3.0 end device firmware for the **PHY6222** (ARM Cortex-M0),
reporting temperature and humidity via ZCL clusters 0x0402 and 0x0405.

**This is the only zigbee-rs example with a 100% pure-Rust IEEE 802.15.4 radio
driver** — no vendor SDK, no binary blobs, no C FFI. All radio hardware access
is through direct register writes in Rust.

## Hardware

- **MCU:** PHY6222 (512KB flash) or PHY6252 (256KB flash) — ARM Cortex-M0, 64KB SRAM
- **Radio:** Built-in 2.4 GHz IEEE 802.15.4 + BLE (pure Rust driver)
- **Boards:** Ai-Thinker PB-03F (PHY6252, ~$1.50), Tuya THB2, TH05F, BTH01 (PHY6222)
- **Button:** GPIO15 (PROG button on PB-03F) — join/leave network
- **LEDs:** GPIO11 (red), GPIO12 (green), GPIO14 (blue) — active low on PB-03F

## Prerequisites

- Rust nightly with `thumbv6m-none-eabi` target
- Any ARM SWD debugger (J-Link, ST-Link, DAPLink, etc.)

```bash
rustup target add thumbv6m-none-eabi
```

## Vendor Library Setup

**None required!** 🎉

The PHY6222 radio driver is implemented entirely in Rust using direct register
access. No vendor SDK, no precompiled `.a` files, no environment variables
to configure.

This makes the PHY6222 the simplest example to build and the easiest to audit.

## Building

```bash
cd examples/phy6222-sensor
cargo build --release
```

That's it. No `--features stubs`, no SDK paths, no vendor blobs.

The `stubs` feature exists for CI consistency but is **not needed** — the
project builds without any external libraries.

### Binary size

The release build produces a compact firmware:

- **Flash:** ~57 KB (.text + .rodata)
- **RAM:** ~4.3 KB (.data + .bss)

## Flashing

Use any ARM SWD debugger — the PHY6222 is a standard Cortex-M0:

```bash
# With probe-rs
probe-rs run --chip PHY6222 target/thumbv6m-none-eabi/release/phy6222-sensor

# With openocd
openocd -f interface/cmsis-dap.cfg -f target/phy6222.cfg \
  -c "program target/thumbv6m-none-eabi/release/phy6222-sensor verify reset exit"

# With pyOCD
pyocd flash -t phy6222 target/thumbv6m-none-eabi/release/phy6222-sensor
```

## What It Demonstrates

- **First pure-Rust IEEE 802.15.4 radio driver** in the zigbee-rs project
- Zigbee 3.0 end device on the ultra-low-cost PHY6222 (~$1.50 boards)
- Embassy async runtime on Cortex-M0 with **real SysTick time driver**
- Proper interrupt vector table (32 entries for all PHY6222 peripherals)
- No vendor dependencies — fully auditable, reproducible builds
- Button-driven network join/leave with edge detection
- RGB LED status indication + identify blink
- ZCL Temperature Measurement + Relative Humidity + Identify clusters
- Flash NV storage — network state persists across reboots (shared `LogStructuredNv`)
- NWK Leave handler — auto-erase NV + rejoin when coordinator sends Leave
- Default reporting — temp/hum every 60–300s, battery every 300–3600s
- Real battery voltage via ADC

## Power Management — Two-Tier Sleep

The firmware implements a comprehensive two-tier sleep architecture that
achieves ~3+ years battery life on 2×AAA (~1200 mAh).

### Two-Tier Architecture

| Tier | Phase | Sleep Mode | Current | Duration |
|------|-------|-----------|---------|----------|
| 1 | Fast poll (250 ms) | Radio off + WFE | ~1.5 mA | 120 s after join |
| 2 | Slow poll (30 s) | AON system sleep | ~3 µA | Steady state |

**During fast poll**, the radio is powered down between polls and the CPU
enters WFE via Embassy's timer. This is responsive but draws ~1.5 mA.

**During slow poll** (steady state), the device enters full AON system sleep:
1. Radio powered down (`radio_sleep()`)
2. Zigbee state saved to flash NV
3. Unused GPIOs set to input + pull-down (leak prevention)
4. Flash put into deep power-down (JEDEC 0xB9, ~1 µA vs ~15 µA)
5. SRAM retention configured
6. RTC wake-up scheduled (32 kHz RC oscillator)
7. System sleep entered (~3 µA total)

On wake, the firmware detects the sleep reset, restores flash from deep
power-down, and performs a fast Zigbee state restore from NV.

### Reportable Change Thresholds

Reports are only sent when values change significantly, suppressing noise:
- Temperature: ±0.5 °C
- Humidity: ±1%
- Battery: ±2%

### Battery Life Estimate

| State | Current |
|-------|---------|
| AON system sleep (radio/flash off) | ~3 µA |
| Radio RX (poll every 30 s) | ~8 mA × 10 ms |
| Radio TX (report every 60 s) | ~10 mA × 3 ms |
| **Average (steady state)** | **~6–10 µA** |
| **Battery life (2×AAA, ~1200 mAh)** | **~3+ years** |

## Project Structure

```
phy6222-sensor/
├── .cargo/config.toml   # Target: thumbv6m-none-eabi, build-std
├── Cargo.toml            # Dependencies (no vendor libs!)
├── build.rs              # Linker script + device.x for interrupt vectors
├── device.x              # PHY6222 interrupt vector names (32 IRQs)
├── memory.x              # Flash @ 0x11001000, RAM @ 0x1FFF0000
└── src/
    ├── main.rs           # Entry point, device setup, sensor loop
    ├── time_driver.rs    # Embassy time driver (SysTick, 1ms tick, µs resolution)
    ├── vectors.rs        # Interrupt vector table + NVIC Interrupt enum
    ├── flash_nv.rs       # Flash NV via shared LogStructuredNv<FlashDriver>
    └── stubs.rs          # CI stubs (not needed for real builds)
```
