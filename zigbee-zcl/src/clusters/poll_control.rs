//! Poll Control cluster (0x0020).
//!
//! Server = sleepy end device; Client = coordinator/gateway.
//! Allows a sleepy device to check in periodically and be told to enter fast-polling mode.

use crate::attribute::{AttributeAccess, AttributeDefinition, AttributeStore};
use crate::clusters::{AttributeStoreAccess, AttributeStoreMutAccess, Cluster};
use crate::data_types::{ZclDataType, ZclValue};
use crate::{AttributeId, ClusterId, CommandId, ZclStatus};

// Attribute IDs
pub const ATTR_CHECK_IN_INTERVAL: AttributeId = AttributeId(0x0000);
pub const ATTR_LONG_POLL_INTERVAL: AttributeId = AttributeId(0x0001);
pub const ATTR_SHORT_POLL_INTERVAL: AttributeId = AttributeId(0x0002);
pub const ATTR_FAST_POLL_TIMEOUT: AttributeId = AttributeId(0x0003);
pub const ATTR_CHECK_IN_INTERVAL_MIN: AttributeId = AttributeId(0x0004);
pub const ATTR_LONG_POLL_INTERVAL_MIN: AttributeId = AttributeId(0x0005);
pub const ATTR_FAST_POLL_TIMEOUT_MAX: AttributeId = AttributeId(0x0006);

// Server→Client command IDs
pub const CMD_CHECK_IN: CommandId = CommandId(0x00);

// Client→Server command IDs
pub const CMD_CHECK_IN_RESPONSE: CommandId = CommandId(0x00);
pub const CMD_FAST_POLL_STOP: CommandId = CommandId(0x01);
pub const CMD_SET_LONG_POLL_INTERVAL: CommandId = CommandId(0x02);
pub const CMD_SET_SHORT_POLL_INTERVAL: CommandId = CommandId(0x03);

/// Poll Control cluster implementation.
pub struct PollControlCluster {
    store: AttributeStore<7>,
    fast_polling: bool,
    /// Ticks since last check-in (1 tick = 1 quarter-second = 250ms).
    ticks_since_checkin: u32,
    /// Remaining fast-poll ticks (quarter-seconds).
    fast_poll_remaining: u16,
}

impl Default for PollControlCluster {
    fn default() -> Self {
        Self::new()
    }
}

impl PollControlCluster {
    pub fn new() -> Self {
        let mut store = AttributeStore::new();
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_CHECK_IN_INTERVAL,
                data_type: ZclDataType::U32,
                access: AttributeAccess::ReadWrite,
                name: "CheckInInterval",
            },
            ZclValue::U32(14400), // 60 min in quarter-seconds
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_LONG_POLL_INTERVAL,
                data_type: ZclDataType::U32,
                access: AttributeAccess::ReadOnly,
                name: "LongPollInterval",
            },
            ZclValue::U32(24), // 6 sec in quarter-seconds
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_SHORT_POLL_INTERVAL,
                data_type: ZclDataType::U16,
                access: AttributeAccess::ReadOnly,
                name: "ShortPollInterval",
            },
            ZclValue::U16(4), // 1 sec in quarter-seconds
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_FAST_POLL_TIMEOUT,
                data_type: ZclDataType::U16,
                access: AttributeAccess::ReadWrite,
                name: "FastPollTimeout",
            },
            ZclValue::U16(40), // 10 sec in quarter-seconds
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_CHECK_IN_INTERVAL_MIN,
                data_type: ZclDataType::U32,
                access: AttributeAccess::ReadOnly,
                name: "CheckInIntervalMin",
            },
            ZclValue::U32(0),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_LONG_POLL_INTERVAL_MIN,
                data_type: ZclDataType::U32,
                access: AttributeAccess::ReadOnly,
                name: "LongPollIntervalMin",
            },
            ZclValue::U32(0),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_FAST_POLL_TIMEOUT_MAX,
                data_type: ZclDataType::U16,
                access: AttributeAccess::ReadOnly,
                name: "FastPollTimeoutMax",
            },
            ZclValue::U16(0),
        );
        Self {
            store,
            fast_polling: false,
            ticks_since_checkin: 0,
            fast_poll_remaining: 0,
        }
    }

    /// Build a CheckIn command payload (server→client, empty body).
    pub fn trigger_checkin(&self) -> heapless::Vec<u8, 64> {
        heapless::Vec::new()
    }

    /// Tick the poll control cluster (call every quarter-second = 250ms).
    ///
    /// Returns `true` when a CheckIn command should be sent to the bound client.
    pub fn tick(&mut self) -> bool {
        // Fast-poll timeout countdown
        if self.fast_polling && self.fast_poll_remaining > 0 {
            self.fast_poll_remaining = self.fast_poll_remaining.saturating_sub(1);
            if self.fast_poll_remaining == 0 {
                self.fast_polling = false;
            }
        }

        // Check-in interval countdown
        let check_in_interval = match self.store.get(ATTR_CHECK_IN_INTERVAL) {
            Some(ZclValue::U32(v)) => *v,
            _ => 14400,
        };
        if check_in_interval == 0 {
            return false; // disabled
        }

        self.ticks_since_checkin += 1;
        if self.ticks_since_checkin >= check_in_interval {
            self.ticks_since_checkin = 0;
            true // time to send CheckIn
        } else {
            false
        }
    }

    /// Enter fast-polling mode with the given timeout (in quarter-seconds).
    ///
    /// The timeout applies to this fast-poll period only; the FastPollTimeout
    /// attribute (the default period) is left unchanged.
    pub fn set_fast_polling(&mut self, timeout: u16) {
        self.fast_poll_remaining = timeout;
        self.fast_polling = timeout != 0;
    }

    /// Whether the device is currently in fast-polling mode.
    pub fn is_fast_polling(&self) -> bool {
        self.fast_polling
    }

    fn u32_attr(&self, id: AttributeId) -> u32 {
        match self.store.get(id) {
            Some(ZclValue::U32(v)) => *v,
            Some(ZclValue::U16(v)) => *v as u32,
            _ => 0,
        }
    }

    /// Range/ordering rules for the poll intervals (ZCL r8 §3.16.4).
    fn check_value(&self, id: AttributeId, value: &ZclValue) -> Result<(), ZclStatus> {
        let ok = match (id, value) {
            // 0 disables check-ins; otherwise within range, not below the
            // product minimum and not shorter than the long poll interval.
            (ATTR_CHECK_IN_INTERVAL, ZclValue::U32(v)) => {
                *v == 0
                    || (*v <= MAX_POLL_INTERVAL
                        && *v >= self.u32_attr(ATTR_CHECK_IN_INTERVAL_MIN)
                        && *v >= self.u32_attr(ATTR_LONG_POLL_INTERVAL))
            }
            (ATTR_FAST_POLL_TIMEOUT, ZclValue::U16(v)) => self.fast_poll_timeout_ok(*v),
            _ => true,
        };
        if ok {
            Ok(())
        } else {
            Err(ZclStatus::InvalidValue)
        }
    }

    /// FastPollTimeout values must be non-zero and, when the product
    /// advertises a FastPollTimeoutMax, not exceed it.
    fn fast_poll_timeout_ok(&self, v: u16) -> bool {
        let max = self.u32_attr(ATTR_FAST_POLL_TIMEOUT_MAX);
        v != 0 && (max == 0 || v as u32 <= max)
    }
}

/// Largest LongPollInterval / CheckInInterval (quarter-seconds, ZCL r8 §3.16.4).
const MAX_POLL_INTERVAL: u32 = 0x006E_0000;

/// Attribute writes go through the cluster so range/ordering rules are
/// enforced (INVALID_VALUE), not just the data type.
impl AttributeStoreMutAccess for PollControlCluster {
    fn set(&mut self, id: AttributeId, value: ZclValue) -> Result<(), ZclStatus> {
        AttributeStoreMutAccess::validate_set(self, id, &value)?;
        self.store.set(id, value)
    }
    fn set_raw(&mut self, id: AttributeId, value: ZclValue) -> Result<(), ZclStatus> {
        self.store.set_raw(id, value)
    }
    fn find(&self, id: AttributeId) -> Option<&AttributeDefinition> {
        self.store.find(id)
    }
    fn validate_set(&self, id: AttributeId, value: &ZclValue) -> Result<(), ZclStatus> {
        self.store.validate_set(id, value)?;
        self.check_value(id, value)
    }
}

impl Cluster for PollControlCluster {
    fn cluster_id(&self) -> ClusterId {
        ClusterId(0x0020)
    }

    fn handle_command(
        &mut self,
        cmd_id: CommandId,
        payload: &[u8],
    ) -> Result<heapless::Vec<u8, 64>, ZclStatus> {
        match cmd_id {
            CMD_CHECK_IN_RESPONSE => {
                if payload.len() < 3 {
                    return Err(ZclStatus::MalformedCommand);
                }
                let start_fast_polling = payload[0] != 0;
                let requested = u16::from_le_bytes([payload[1], payload[2]]);
                if start_fast_polling {
                    // §3.16.5.1: a zero timeout means "use the FastPollTimeout
                    // attribute"; a non-zero one must respect FastPollTimeoutMax.
                    let timeout = if requested == 0 {
                        self.u32_attr(ATTR_FAST_POLL_TIMEOUT) as u16
                    } else if self.fast_poll_timeout_ok(requested) {
                        requested
                    } else {
                        return Err(ZclStatus::InvalidField);
                    };
                    self.set_fast_polling(timeout);
                } else {
                    self.fast_polling = false;
                }
                Ok(heapless::Vec::new())
            }
            CMD_FAST_POLL_STOP => {
                self.fast_polling = false;
                Ok(heapless::Vec::new())
            }
            CMD_SET_LONG_POLL_INTERVAL => {
                if payload.len() < 4 {
                    return Err(ZclStatus::MalformedCommand);
                }
                let interval = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                // §3.16.5.3: within 0x04..=0x6E0000, not below
                // LongPollIntervalMin or ShortPollInterval, and not above
                // a non-zero CheckInInterval.
                let check_in = self.u32_attr(ATTR_CHECK_IN_INTERVAL);
                if !(4..=MAX_POLL_INTERVAL).contains(&interval)
                    || interval < self.u32_attr(ATTR_LONG_POLL_INTERVAL_MIN)
                    || interval < self.u32_attr(ATTR_SHORT_POLL_INTERVAL)
                    || (check_in != 0 && interval > check_in)
                {
                    return Err(ZclStatus::InvalidValue);
                }
                self.store
                    .set_raw(ATTR_LONG_POLL_INTERVAL, ZclValue::U32(interval))?;
                Ok(heapless::Vec::new())
            }
            CMD_SET_SHORT_POLL_INTERVAL => {
                if payload.len() < 2 {
                    return Err(ZclStatus::MalformedCommand);
                }
                let interval = u16::from_le_bytes([payload[0], payload[1]]);
                // §3.16.5.4: non-zero and not above LongPollInterval.
                if interval == 0 || interval as u32 > self.u32_attr(ATTR_LONG_POLL_INTERVAL) {
                    return Err(ZclStatus::InvalidValue);
                }
                self.store
                    .set_raw(ATTR_SHORT_POLL_INTERVAL, ZclValue::U16(interval))?;
                Ok(heapless::Vec::new())
            }
            _ => Err(ZclStatus::UnsupClusterCommand),
        }
    }

    fn received_commands(&self) -> heapless::Vec<u8, 32> {
        let mut v = heapless::Vec::new();
        let _ = v.push(CMD_CHECK_IN_RESPONSE.0);
        let _ = v.push(CMD_FAST_POLL_STOP.0);
        let _ = v.push(CMD_SET_LONG_POLL_INTERVAL.0);
        let _ = v.push(CMD_SET_SHORT_POLL_INTERVAL.0);
        v
    }

    fn generated_commands(&self) -> heapless::Vec<u8, 32> {
        let mut v = heapless::Vec::new();
        let _ = v.push(CMD_CHECK_IN.0);
        v
    }

    fn attributes(&self) -> &dyn AttributeStoreAccess {
        &self.store
    }

    fn attributes_mut(&mut self) -> &mut dyn AttributeStoreMutAccess {
        self
    }

    fn reset_to_factory_defaults(&mut self) {
        let _ = self
            .store
            .set_raw(ATTR_CHECK_IN_INTERVAL, ZclValue::U32(14400));
        let _ = self
            .store
            .set_raw(ATTR_FAST_POLL_TIMEOUT, ZclValue::U16(40));
        // LongPollInterval/ShortPollInterval are technically ReadOnly per
        // their access mode but are mutated by SetLongPollInterval/
        // SetShortPollInterval — restore them to their power-on defaults
        // too so a stale negotiated interval cannot survive a reset.
        let _ = self
            .store
            .set_raw(ATTR_LONG_POLL_INTERVAL, ZclValue::U32(24));
        let _ = self
            .store
            .set_raw(ATTR_SHORT_POLL_INTERVAL, ZclValue::U16(4));
        self.fast_polling = false;
        self.ticks_since_checkin = 0;
        self.fast_poll_remaining = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(c: &PollControlCluster, id: AttributeId) -> ZclValue {
        c.attributes().get(id).unwrap().clone()
    }

    #[test]
    fn check_in_response_zero_timeout_uses_fast_poll_timeout_attribute() {
        let mut c = PollControlCluster::new();
        c.handle_command(CMD_CHECK_IN_RESPONSE, &[1, 0, 0]).unwrap();
        assert!(c.is_fast_polling());
        // Default FastPollTimeout is 40 quarter-seconds: ends after 40 ticks.
        for _ in 0..39 {
            c.tick();
        }
        assert!(c.is_fast_polling());
        c.tick();
        assert!(!c.is_fast_polling());
        // A per-check-in timeout does not overwrite the attribute.
        c.handle_command(CMD_CHECK_IN_RESPONSE, &[1, 8, 0]).unwrap();
        assert_eq!(get(&c, ATTR_FAST_POLL_TIMEOUT), ZclValue::U16(40));
    }

    #[test]
    fn fast_poll_timeout_max_is_enforced() {
        let mut c = PollControlCluster::new();
        c.store
            .set_raw(ATTR_FAST_POLL_TIMEOUT_MAX, ZclValue::U16(100))
            .unwrap();
        assert_eq!(
            c.handle_command(CMD_CHECK_IN_RESPONSE, &[1, 101, 0]),
            Err(ZclStatus::InvalidField)
        );
        assert!(!c.is_fast_polling());
        assert_eq!(
            c.attributes_mut()
                .set(ATTR_FAST_POLL_TIMEOUT, ZclValue::U16(101)),
            Err(ZclStatus::InvalidValue)
        );
        assert_eq!(
            c.attributes_mut()
                .set(ATTR_FAST_POLL_TIMEOUT, ZclValue::U16(0)),
            Err(ZclStatus::InvalidValue)
        );
        c.attributes_mut()
            .set(ATTR_FAST_POLL_TIMEOUT, ZclValue::U16(100))
            .unwrap();
    }

    #[test]
    fn poll_interval_commands_validate_range_and_ordering() {
        let mut c = PollControlCluster::new();
        let long = |v: u32| v.to_le_bytes();
        for bad in [
            0u32, 3, 2,     /* < short (4) */
            14401, /* > check-in */
        ] {
            assert_eq!(
                c.handle_command(CMD_SET_LONG_POLL_INTERVAL, &long(bad)),
                Err(ZclStatus::InvalidValue),
                "{bad}"
            );
        }
        c.handle_command(CMD_SET_LONG_POLL_INTERVAL, &long(8))
            .unwrap();
        assert_eq!(get(&c, ATTR_LONG_POLL_INTERVAL), ZclValue::U32(8));
        for bad in [0u16, 9] {
            assert_eq!(
                c.handle_command(CMD_SET_SHORT_POLL_INTERVAL, &bad.to_le_bytes()),
                Err(ZclStatus::InvalidValue)
            );
        }
        c.handle_command(CMD_SET_SHORT_POLL_INTERVAL, &8u16.to_le_bytes())
            .unwrap();
        // CheckInInterval below LongPollInterval is rejected; 0 disables.
        assert_eq!(
            c.attributes_mut()
                .set(ATTR_CHECK_IN_INTERVAL, ZclValue::U32(7)),
            Err(ZclStatus::InvalidValue)
        );
        c.attributes_mut()
            .set(ATTR_CHECK_IN_INTERVAL, ZclValue::U32(0))
            .unwrap();
    }
}
