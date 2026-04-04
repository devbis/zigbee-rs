# Power Management

Battery-powered Zigbee devices spend most of their life asleep. The
`zigbee-runtime` crate provides a `PowerManager` that decides *when* to sleep,
*how long* to sleep, and *what kind* of sleep to use — while still meeting
Zigbee's poll and reporting deadlines.

---

## PowerMode

Every device declares its power strategy through the `PowerMode` enum
(`zigbee_runtime::power::PowerMode`):

```rust
pub enum PowerMode {
    /// Always on — router or mains-powered end device.
    AlwaysOn,

    /// Sleepy End Device — periodic wake for polling.
    Sleepy {
        /// Poll interval in milliseconds.
        poll_interval_ms: u32,
        /// How long to stay awake after activity (ms).
        wake_duration_ms: u32,
    },

    /// Deep sleep — wake only on timer or external event.
    DeepSleep {
        /// Wake interval in seconds.
        wake_interval_s: u32,
    },
}
```

| Mode | Typical Use | Radio | CPU | RAM |
|------|------------|-------|-----|-----|
| `AlwaysOn` | Routers, mains-powered EDs | On | On | Retained |
| `Sleepy` | Battery sensors, remotes | Off between polls | Halted | Retained |
| `DeepSleep` | Ultra-low-power sensors | Off | Off | Off (RTC only) |

Set the power mode when you build your device:

```rust
use zigbee_runtime::power::{PowerManager, PowerMode};

let pm = PowerManager::new(PowerMode::Sleepy {
    poll_interval_ms: 7_500,   // poll parent every 7.5 s
    wake_duration_ms: 200,     // stay awake 200 ms after activity
});
```

---

## SleepDecision

Each iteration of the event loop calls `PowerManager::decide(now_ms)`. The
manager returns one of three verdicts:

```rust
pub enum SleepDecision {
    /// Stay awake — pending work.
    StayAwake,
    /// Light sleep for the given duration (ms). CPU halted, RAM retained.
    LightSleep(u32),
    /// Deep sleep for the given duration (ms). Only RTC + wake sources active.
    DeepSleep(u32),
}
```

### Decision Logic

The decision tree inside `decide()` works as follows:

1. **Pending work?** — If `pending_tx` or `pending_reports` is set, always
   return `StayAwake`. Outgoing frames and attribute reports must be sent
   before the CPU is halted.

2. **AlwaysOn** — Always `StayAwake`. Routers never sleep.

3. **Sleepy** —
   - If less than `wake_duration_ms` has elapsed since the last activity
     (Rx/Tx, sensor read, user input), stay awake.
   - If a MAC poll is overdue (`since_poll >= poll_interval_ms`), stay awake
     to send the poll immediately.
   - Otherwise, enter `LightSleep` for the time remaining until the next
     poll is due.

4. **DeepSleep** —
   - If the last activity was within the last 1 second, stay awake (brief
     grace period for completing any post-wake work).
   - Otherwise, enter `DeepSleep` for `wake_interval_s × 1000` ms.

```rust
let decision = pm.decide(now_ms);
match decision {
    SleepDecision::StayAwake => { /* process events */ }
    SleepDecision::LightSleep(ms) => mac.sleep(ms),
    SleepDecision::DeepSleep(ms)  => mac.deep_sleep(ms),
}
```

---

## Sleepy End Device (SED) Behavior

A Sleepy End Device is a Zigbee device that spends most of its time with the
radio off. Its parent router buffers incoming frames and releases them when the
SED sends a MAC Data Request (poll).

### Poll Interval

The poll interval determines how often the SED wakes to check for buffered
data. Use `PowerManager::should_poll(now_ms)` to decide when to send a poll:

```rust
if pm.should_poll(now_ms) {
    mac.send_data_request(parent_addr);
    pm.record_poll(now_ms);
}
```

Typical poll intervals:

| Application | Poll Interval | Battery Impact |
|-------------|--------------|----------------|
| Light switch | 250–500 ms | High responsiveness, shorter battery |
| Door sensor | 5–10 s | Moderate |
| Temperature sensor | 30–60 s | Very low power |

### Activity Tracking

Call `record_activity()` whenever something interesting happens — a frame is
received, a sensor is read, or a user presses a button. This resets the
wake-duration timer and prevents premature sleep:

```rust
pm.record_activity(now_ms);  // keep CPU awake for at least wake_duration_ms
```

The `set_pending_tx()` and `set_pending_reports()` methods act as hard locks
that prevent sleep entirely until the work is done:

```rust
pm.set_pending_tx(true);       // acquired before queueing a frame
// ... send the frame ...
pm.set_pending_tx(false);      // release after MAC confirms transmission
```

---

## How MAC Backends Implement Sleep

The `PowerManager` itself does not touch hardware — it only *decides*. The
actual sleep/wake is performed by the MAC backend:

| Platform | Light Sleep | Deep Sleep |
|----------|-----------|------------|
| ESP32-C6/H2 | `esp_light_sleep_start()` | `esp_deep_sleep()` — only RTC memory retained |
| nRF52840 | `__WFE` (System ON, RAM retained) | System OFF (wake via GPIO/RTC) |
| PHY6222 | `radio_sleep()` + WFE (~1.5 mA) | AON system sleep (~3 µA, RTC wake) |
| BL702 | PDS (Power Down Sleep) | HBN (Hibernate) — wake via RTC |

The runtime event loop integrates the power manager like this (simplified):

```rust
loop {
    // 1. Process all pending events
    process_mac_events(&mut pm);
    process_zcl_reports(&mut pm);

    // 2. Ask the power manager what to do
    let decision = pm.decide(now_ms());

    match decision {
        SleepDecision::StayAwake => continue,
        SleepDecision::LightSleep(ms) => {
            mac.enter_light_sleep(ms);
            // CPU resumes here after wake
        }
        SleepDecision::DeepSleep(ms) => {
            nv.persist_state();          // save everything before deep sleep
            mac.enter_deep_sleep(ms);
            // After deep sleep, device resets — execution restarts from main()
        }
    }
}
```

> **Important:** Before entering `DeepSleep`, all critical state must be
> persisted to NV storage — deep sleep usually causes a full CPU reset and RAM
> is lost. See [NV Storage](./nv-storage.md) for details.

---

## Platform-Specific Power Optimizations

### nRF52840

The nRF52840 sensor example applies several hardware-level optimizations beyond
the basic sleep/wake cycle:

**DC-DC converter** — The nRF52840 has internal LDO regulators that can be
replaced by an on-chip DC-DC converter for ~40% lower current draw. Both
`reg0` (main 1.3 V supply) and `reg1` (radio 1.8 V supply) are enabled:

```rust
config.dcdc = embassy_nrf::config::DcdcConfig {
    reg0: true,
    reg0_voltage: None, // keep UICR default
    reg1: true,
};
```

**TX power reduction** — Default TX power is reduced from +8 dBm to 0 dBm,
saving ~50% TX current while still providing adequate range for home use:

```rust
mac.set_tx_power(0); // 0 dBm — good range, saves ~50% TX current vs +8 dBm
```

**Internal RC oscillator** — The HFCLK source is set to the internal RC
oscillator instead of the external crystal. The radio hardware automatically
requests the XTAL when it needs high accuracy (during TX/RX), then releases
it. This saves ~250 µA when the radio is idle:

```rust
config.hfclk_source = embassy_nrf::config::HfclkSource::Internal;
```

**RAM bank power-down** — Unused RAM banks are powered down during sleep,
saving additional current. On the nRF52840-DK, ~190 KB of unused RAM can be
powered off.

**Polling and reporting** — The sensor uses a two-phase polling scheme:
- Fast poll: 250 ms for 120 seconds after joining/activity (responsive)
- Slow poll: 30 seconds during steady state (low power)
- Report interval: 60 seconds

### PHY6222

The PHY6222 sensor implements a **two-tier sleep architecture** that combines
light sleep during fast polling with deep AON system sleep during slow polling.

**Tier 1 — Light sleep (fast poll, ~1.5 mA):**
During fast polling (first 120 seconds after join/activity), the radio is
turned off between polls and the CPU enters WFE via Embassy's `Timer::after()`:

```rust
device.mac_mut().radio_sleep();
Timer::after(Duration::from_millis(poll_ms)).await;
device.mac_mut().radio_wake();
```

**Tier 2 — AON system sleep (slow poll, ~3 µA):**
During slow polling (30-second intervals), the device enters full system sleep:

```rust
// Turn off radio
device.mac_mut().radio_sleep();
// Save Zigbee state to flash NV
device.save_state(&mut nv);
// Prepare peripherals for minimum leakage
phy6222_hal::gpio::prepare_for_sleep(1 << pins::BTN);
// Flash to deep power-down (~1µA vs ~15µA standby)
phy6222_hal::flash::enter_deep_sleep();
// Configure SRAM retention and RTC wake
phy6222_hal::sleep::set_ram_retention(phy6222_hal::regs::RET_SRAM0);
phy6222_hal::sleep::config_rtc_wakeup(
    phy6222_hal::sleep::ms_to_rtc_ticks(poll_ms as u32),
);
phy6222_hal::sleep::enter_system_sleep();
```

On wake, the firmware detects the system-sleep reset, restores flash from deep
power-down, and does a fast restore of the Zigbee network state from NV.

**Flash deep power-down** — JEDEC commands `0xB9` (enter) and `0xAB` (release)
reduce flash standby current from ~15 µA to ~1 µA:

```rust
phy6222_hal::flash::enter_deep_sleep();   // JEDEC 0xB9
phy6222_hal::flash::release_deep_sleep(); // JEDEC 0xAB on wake
```

**GPIO leak prevention** — Before system sleep, all unused GPIO pins are
configured as inputs with pull-down resistors to prevent floating-pin leakage.
Only essential pins (e.g., the button) retain their pull-up:

```rust
phy6222_hal::gpio::prepare_for_sleep(1 << pins::BTN);
```

**Radio sleep/wake** — The MAC driver provides `radio_sleep()` and
`radio_wake()` methods that power down the radio transceiver between polls,
saving ~5–8 mA:

```rust
device.mac_mut().radio_sleep();
// ... sleep ...
device.mac_mut().radio_wake();
```

The `phy6222-hal::sleep` module provides the full AON domain API:

| Function | Purpose |
|----------|---------|
| `config_rtc_wakeup(ticks)` | Set RTC compare channel 0 for timed wake |
| `set_ram_retention(banks)` | Select SRAM banks to retain during sleep |
| `enter_system_sleep()` | Enter AON system sleep (~3 µA, does not return) |
| `was_sleep_reset()` | Check if current boot was a wake from system sleep |
| `clear_sleep_flag()` | Clear the sleep-wake flag after detection |
| `ms_to_rtc_ticks(ms)` | Convert milliseconds to 32 kHz RC ticks |

---

## Reportable Change Thresholds

Both the nRF52840 and PHY6222 sensor examples configure **reportable change
thresholds** in the ZCL Reporting Configuration to suppress unnecessary
transmissions. A report is sent only when the attribute value changes by more
than the threshold *or* the maximum reporting interval expires:

| Attribute | Min Interval | Max Interval | Reportable Change |
|-----------|-------------|-------------|-------------------|
| Temperature (0x0402) | 60 s | 300 s | ±0.5 °C (50 centidegrees) |
| Humidity (0x0405) | 60 s | 300 s | ±1% (100 centi-%) |
| Battery (0x0001) | 300 s | 3600 s | ±2% (4 in 0.5% units) |

This means a device that sits at constant temperature will only report every
5 minutes (max interval), and tiny fluctuations (e.g., ±0.1 °C) are
suppressed entirely. This can reduce TX events by 80–90% in stable
environments.

---

## Power Budget Estimates

### nRF52840 (CR2032, 230 mAh)

| State | Current | Duty Cycle | Average |
|-------|---------|------------|---------|
| System ON idle (DC-DC, internal RC, RAM power-down) | ~3 µA | ~99.8% | ~3.0 µA |
| Radio RX (poll, 0 dBm) | ~5 mA | ~0.03% (10 ms / 30 s) | ~1.7 µA |
| Radio TX (report, 0 dBm) | ~5 mA | ~0.005% (3 ms / 60 s) | ~0.25 µA |
| Sensor read | ~1 mA | ~0.003% | ~0.03 µA |
| **Total average** | | | **~5 µA** |
| **Estimated battery life (CR2032)** | | | **~5+ years** |

> With reportable change thresholds suppressing most TX events, practical
> battery life approaches the self-discharge limit of the CR2032.

### PHY6222 (2×AAA, ~1200 mAh)

| State | Current | Duty Cycle | Average |
|-------|---------|------------|---------|
| AON system sleep (radio off, flash off, GPIO prepared) | ~3 µA | ~99.8% | ~3.0 µA |
| Flash standby (deep power-down) | ~1 µA | — | included above |
| Radio RX (poll) | ~8 mA | ~0.03% (10 ms / 30 s) | ~2.7 µA |
| Radio TX (report) | ~10 mA | ~0.005% (3 ms / 60 s) | ~0.5 µA |
| Fast poll phase (WFE, ~1.5 mA) | ~1.5 mA | ~1.5% (120 s / 2 hr) | ~22 µA |
| **Total average (steady state)** | | | **~6–35 µA** |
| **Estimated battery life (2×AAA)** | | | **~3+ years** |

> The fast-poll phase (first 120 seconds after joining or button press) draws
> ~1.5 mA but lasts only briefly. In steady state with 30-second slow polls
> and AON system sleep, the average drops below 10 µA.

---

## Battery Optimization Tips

1. **Minimize wake time.** Process events as fast as possible, then sleep.
   A typical SED wake cycle should complete in under 10 ms.

2. **Batch sensor reads with polls.** Read the sensor just before sending
   a report, so you don't need a separate wake cycle.

3. **Use appropriate poll intervals.** A door sensor that only reports on
   state change doesn't need 250 ms polls — 30 seconds is fine.

4. **Prefer DeepSleep for long idle periods.** If the device only reports
   every 5 minutes, deep sleep (with NV persistence) uses orders of
   magnitude less power than light sleep.

5. **Disable unused peripherals.** Turn off ADC, I²C, and SPI buses before
   sleeping — stray current through pull-ups adds up.

6. **Use reporting intervals instead of polling.** Configure the server-side
   minimum/maximum reporting intervals in the ZCL Reporting Configuration so
   the device only wakes when it has something new to say.

7. **Keep the network key frame counter in NV.** Frame counters must
   survive reboots. If a device resets its counter to zero, the network
   will reject its frames as replays.

8. **Enable DC-DC converters (nRF52840).** Switching from the internal LDO
   to the DC-DC converter saves ~40% idle current.

9. **Reduce TX power.** For home automation, 0 dBm provides plenty of range
   while halving TX current compared to +8 dBm.

10. **Use reportable change thresholds.** Adding a minimum change threshold
    (e.g., ±0.5 °C for temperature) eliminates unnecessary transmissions
    caused by sensor noise or small fluctuations.

11. **Power down flash (PHY6222).** Put external or on-chip flash into deep
    power-down mode before system sleep — saves ~14 µA.

12. **Prepare GPIOs for sleep (PHY6222).** Set unused pins to input with
    pull-down to prevent floating-pin leakage current.
