# TLSR8258 Hardware Lab

This is not an application example. It preserves the hardware bring-up and
regression firmware used to prove raw radio, association, flash, startup, and
retention power management on TLSR8258.

The large `src/main.rs` intentionally contains legacy direct-MMIO radio/MAC
and manual Zigbee paths while those diagnostics are still useful. New
applications should use:

- `examples/telink-tlsr8258-sensor`
- `examples/telink-tlsr8258-router`
- `tlsr8258-hal`
- `tlsr8258-rt`

## Lab modes

```bash
./scripts/tlsr8258.sh build diag-beacon
./scripts/tlsr8258.sh build diag-assoc
./scripts/tlsr8258.sh build diag-smoke
./scripts/tlsr8258.sh build diag-pm
./scripts/tlsr8258.sh build lab-sensor
```

`sensor` is the legacy manual-stack regression image. It is retained only for
comparison with earlier hardware captures.

Every lab layout (`memory.x`, `memory-pm.x`, `memory-retention.x`) links
against the production TB-04 application boundary at `0x70000`, and the
post-link check fails any `.bin` that reaches it. Flashing a lab image therefore
cannot overwrite the APS (`0x70000`), child-table (`0x72000`) or security
(`0x74000`) journals or the factory data at `0x76000`.
