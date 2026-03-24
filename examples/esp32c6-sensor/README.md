# ESP32-C6 Zigbee Temperature & Humidity Sensor

A `no_std` Embassy firmware for the ESP32-C6 that implements a Zigbee 3.0 end
device with SHT31-based temperature and humidity reporting.

## Prerequisites

- **Rust nightly** with the `riscv32imac-unknown-none-elf` target:
  ```bash
  rustup toolchain install nightly
  rustup target add riscv32imac-unknown-none-elf --toolchain nightly
  ```
- **espflash** for flashing and serial monitoring:
  ```bash
  cargo install espflash
  ```
- An **ESP32-C6** development board (e.g. ESP32-C6-DevKitC-1)
- An **SHT31** breakout board (I2C, 3.3 V)

## Wiring

| SHT31 Pin | ESP32-C6 Pin |
|-----------|-------------|
| SDA       | GPIO6       |
| SCL       | GPIO7       |
| VCC       | 3.3 V       |
| GND       | GND         |

> **Tip:** The SHT31 ADDR pin determines the I2C address.
> - ADDR → GND: 0x44 (default, `sht3x::Address::Low`)
> - ADDR → VCC: 0x45 (`sht3x::Address::High`)

## Build & Flash

```bash
# Build (release recommended for size and speed)
cargo build --release

# Flash to the board and open serial monitor
cargo run --release
```

The `cargo run` command uses `espflash flash --monitor` as the runner
(configured in `.cargo/config.toml`).

## Zigbee Network

The device starts as an end device and attempts to join any open Zigbee 3.0
network. Use a coordinator (e.g. Zigbee2MQTT + CC2652 stick) with permit-join
enabled.

Once joined, the device exposes **Endpoint 1** with:

| Cluster               | ID       | Direction | Purpose                        |
|-----------------------|----------|-----------|--------------------------------|
| Basic                 | `0x0000` | Server    | Device identity                |
| Temperature Measurement | `0x0402` | Server  | Measured value in 0.01 °C      |
| Relative Humidity     | `0x0405` | Server    | Measured value in 0.01 %RH     |

Sensor readings update every 30 seconds.

## Alternative: ESP32-H2

The ESP32-H2 also has a built-in IEEE 802.15.4 radio. To target it instead,
change every `esp32c6` feature flag to `esp32h2` in both `Cargo.toml` and
`.cargo/config.toml`.

## Alternative Sensors

The firmware uses the SHT31 via the `sht3x` crate. Other I2C sensors work
with minimal changes:

| Sensor | Crate          | Measures                      | I2C Addr |
|--------|----------------|-------------------------------|----------|
| BME280 | `bme280 = "0.5"` | Temp + humidity + pressure  | 0x76     |
| SHTC3  | `shtcx = "1.0"`  | Temp + humidity (low power) | 0x70     |
| AHT20  | `aht20 = "0.3"`  | Temp + humidity (budget)    | 0x38     |
| Si7021 | `si7021 = "0.3"` | Temp + humidity             | 0x40     |

For BME280, add a Pressure Measurement cluster (`0x0403`) to the endpoint
configuration and use `zigbee_zcl::clusters::pressure::PressureCluster`.
