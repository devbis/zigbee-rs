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

## Battery Optimization Tips

1. **Minimize wake time.** Process events as fast as possible, then sleep.
   A typical SED wake cycle should complete in under 10 ms.

2. **Batch sensor reads with polls.** Read the sensor just before sending
   a report, so you don't need a separate wake cycle.

3. **Use appropriate poll intervals.** A door sensor that only reports on
   state change doesn't need 250 ms polls — 10 seconds is fine.

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
