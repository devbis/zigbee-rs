# nRF52840 Zigbee Weather Sensor

Embassy-based firmware that reads a BME280 over I2C and exposes
temperature, humidity, and pressure via Zigbee ZCL clusters.

## Prerequisites

- Rust nightly with the `thumbv7em-none-eabihf` target:
  ```
  rustup target add thumbv7em-none-eabihf
  ```
- **probe-rs** for flashing and defmt log output:
  ```
  cargo install probe-rs-tools
  ```
- nRF52840-DK (or any board with J-Link / CMSIS-DAP debug probe)

## Wiring (BME280)

| BME280 pin | nRF52840 pin |
|------------|--------------|
| SDA        | P0.26        |
| SCL        | P0.27        |
| VCC        | 3.3 V        |
| GND        | GND          |

## Build & Flash

```sh
cargo build --release
cargo run --release   # flashes via probe-rs and shows defmt logs
```

## Alternative Sensors

| Sensor | Crate   | I2C address | Notes                         |
|--------|---------|-------------|-------------------------------|
| SHT31  | `sht3x` | 0x44        | Temp + humidity, no pressure  |
| SHT40  | `sht4x` | 0x44        | Higher accuracy successor     |
| SHTC3  | `shtcx` | 0x70        | Ultra-low-power, fast wakeup  |
| BMP280 | `bme280`| 0x76        | Temp + pressure (no humidity) |

To switch sensors, replace the `bme280` dependency in `Cargo.toml` with the
appropriate crate and adjust the driver initialisation in `src/main.rs`.
