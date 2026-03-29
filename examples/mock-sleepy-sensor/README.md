# Mock Sleepy End Device (SED)

Simulates a battery-powered sleepy temperature/humidity sensor that runs through
10 wake/sleep cycles on the host machine using `MockMac`. Demonstrates the
complete SED lifecycle: cold boot → join → poll → sense → report → sleep → repeat.

## What It Demonstrates

- **SED power management** — `PowerManager` with `DeepSleep` mode (30 s wake intervals), `SleepDecision` (StayAwake / LightSleep / DeepSleep)
- **NV persistence** — `RamNvStorage` saves PAN ID, short address, channel, and frame counters across cycles; warm boot restores state without rejoin
- **MAC polling** — `mlme_poll()` for indirect data from parent coordinator
- **Reportable change** — temperature threshold ±0.10 °C, humidity threshold ±0.50 %; reports sent only when exceeded
- **Poll Control cluster** — periodic check-in every 5th cycle, fast-poll mode (250 ms intervals), `CMD_FAST_POLL_STOP`
- **ZCL attribute reporting** — builds report frames with temperature (I16) + humidity (U16) and sends via `mcps_data()`

## Build & Run

```sh
cargo run -p mock-sleepy-sensor
```

## Expected Output

Runs 10 cycles with color-coded output:

1. **Cycle 1 (COLD BOOT)** — scans channels, associates with PAN `0x1A2B` on channel 15, saves network state to NV
2. **Cycles 2–10 (WARM BOOT)** — restores from NV, no rejoin
3. Each cycle: polls parent → reads simulated sensors → checks reportable change → reports or skips
4. **Every 5th cycle (CHECK-IN)** — sends Poll Control CheckIn, enters fast-poll mode (~40 quarter-second polls), then stops
5. **Sleep decision** — PowerManager chooses deep sleep (30 s), saves frame counters to NV

## Project Structure

```
mock-sleepy-sensor/
├── Cargo.toml      # Dependencies: zigbee-*, pollster
└── src/
    └── main.rs     # SED lifecycle simulation (~670 lines)
```

## Dependencies

| Crate | Purpose |
|---|---|
| `zigbee-mac` (mock) | MockMac, scan, associate, poll, data service |
| `zigbee-zcl` | TemperatureCluster, HumidityCluster, PollControlCluster |
| `zigbee-runtime` | PowerManager, PowerMode, SleepDecision, RamNvStorage |
| `zigbee-aps` | APS layer types |
| `zigbee-types` | IeeeAddress, PanId, ChannelMask |
| `pollster` | Block on async MAC/data calls |
