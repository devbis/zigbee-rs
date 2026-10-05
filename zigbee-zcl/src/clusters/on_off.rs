//! On/Off cluster (0x0006).

use crate::attribute::{AttributeAccess, AttributeDefinition, AttributeStore};
use crate::clusters::{AttributeStoreAccess, AttributeStoreMutAccess, Cluster};
use crate::data_types::{ZclDataType, ZclValue};
use crate::{AttributeId, ClusterId, CommandId, ZclStatus};

// Attribute IDs
pub const ATTR_ON_OFF: AttributeId = AttributeId(0x0000);
pub const ATTR_GLOBAL_SCENE_CONTROL: AttributeId = AttributeId(0x4000);
pub const ATTR_ON_TIME: AttributeId = AttributeId(0x4001);
pub const ATTR_OFF_WAIT_TIME: AttributeId = AttributeId(0x4002);
pub const ATTR_START_UP_ON_OFF: AttributeId = AttributeId(0x4003);

// Command IDs (client to server)
pub const CMD_OFF: CommandId = CommandId(0x00);
pub const CMD_ON: CommandId = CommandId(0x01);
pub const CMD_TOGGLE: CommandId = CommandId(0x02);
pub const CMD_OFF_WITH_EFFECT: CommandId = CommandId(0x40);
pub const CMD_ON_WITH_RECALL_GLOBAL_SCENE: CommandId = CommandId(0x41);
pub const CMD_ON_WITH_TIMED_OFF: CommandId = CommandId(0x42);

/// On/Off cluster implementation.
pub struct OnOffCluster {
    store: AttributeStore<8>,
}

impl Default for OnOffCluster {
    fn default() -> Self {
        Self::new()
    }
}

impl OnOffCluster {
    pub fn new() -> Self {
        let mut store = AttributeStore::new();
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_ON_OFF,
                data_type: ZclDataType::Bool,
                access: AttributeAccess::Reportable,
                name: "OnOff",
            },
            ZclValue::Bool(false),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_GLOBAL_SCENE_CONTROL,
                data_type: ZclDataType::Bool,
                access: AttributeAccess::ReadOnly,
                name: "GlobalSceneControl",
            },
            ZclValue::Bool(true),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_ON_TIME,
                data_type: ZclDataType::U16,
                access: AttributeAccess::ReadWrite,
                name: "OnTime",
            },
            ZclValue::U16(0),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_OFF_WAIT_TIME,
                data_type: ZclDataType::U16,
                access: AttributeAccess::ReadWrite,
                name: "OffWaitTime",
            },
            ZclValue::U16(0),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_START_UP_ON_OFF,
                data_type: ZclDataType::Enum8,
                access: AttributeAccess::ReadWrite,
                name: "StartUpOnOff",
            },
            ZclValue::Enum8(0xFF), // Previous
        );
        Self { store }
    }

    /// Current on/off state.
    pub fn is_on(&self) -> bool {
        matches!(self.store.get(ATTR_ON_OFF), Some(ZclValue::Bool(true)))
    }

    fn set_on_off(&mut self, on: bool) {
        let _ = self.store.set_raw(ATTR_ON_OFF, ZclValue::Bool(on));
    }

    fn u16_attr(&self, id: AttributeId) -> u16 {
        match self.store.get(id) {
            Some(ZclValue::U16(v)) => *v,
            _ => 0,
        }
    }

    /// Off semantics (ZCL r8 §3.8.2.3.1): OnOff = FALSE and OnTime = 0.
    fn turn_off(&mut self) {
        self.set_on_off(false);
        let _ = self.store.set_raw(ATTR_ON_TIME, ZclValue::U16(0));
    }

    /// On semantics (ZCL r8 §3.8.2.3.2): OnOff = TRUE, GlobalSceneControl =
    /// TRUE, and OffWaitTime is cleared when OnTime is 0.
    fn turn_on(&mut self) {
        self.set_on_off(true);
        let _ = self
            .store
            .set_raw(ATTR_GLOBAL_SCENE_CONTROL, ZclValue::Bool(true));
        if self.u16_attr(ATTR_ON_TIME) == 0 {
            let _ = self.store.set_raw(ATTR_OFF_WAIT_TIME, ZclValue::U16(0));
        }
    }

    /// Tick the On/Off cluster timers (call every 100ms = 1/10th second).
    ///
    /// OnTime and OffWaitTime are in 1/10th seconds per ZCL spec.
    /// When OnTime reaches 0 while the device is on, it turns off and
    /// OffWaitTime begins counting down.
    pub fn tick(&mut self) {
        self.tick_by(1);
    }

    /// Advance the timers by `elapsed_deciseconds` without replaying one call
    /// per missed 100 ms period.
    pub fn tick_by(&mut self, elapsed_deciseconds: u32) {
        if elapsed_deciseconds == 0 {
            return;
        }
        let on_time = match self.store.get(ATTR_ON_TIME) {
            Some(ZclValue::U16(v)) => *v,
            _ => 0,
        };
        let off_wait = match self.store.get(ATTR_OFF_WAIT_TIME) {
            Some(ZclValue::U16(v)) => *v,
            _ => 0,
        };

        if self.is_on() && on_time > 0 {
            let elapsed_on = elapsed_deciseconds.min(u32::from(on_time)) as u16;
            let new_on = on_time - elapsed_on;
            let _ = self.store.set_raw(ATTR_ON_TIME, ZclValue::U16(new_on));
            if new_on == 0 {
                // OnTime expired — turn off, start off-wait countdown
                self.set_on_off(false);
                let elapsed_off = elapsed_deciseconds - u32::from(elapsed_on);
                let new_wait = off_wait.saturating_sub(elapsed_off.min(u32::from(u16::MAX)) as u16);
                let _ = self
                    .store
                    .set_raw(ATTR_OFF_WAIT_TIME, ZclValue::U16(new_wait));
            }
        } else if !self.is_on() && off_wait > 0 {
            let new_wait =
                off_wait.saturating_sub(elapsed_deciseconds.min(u32::from(u16::MAX)) as u16);
            let _ = self
                .store
                .set_raw(ATTR_OFF_WAIT_TIME, ZclValue::U16(new_wait));
        }
    }

    /// Apply StartUpOnOff on device power-on (ZCL spec §3.8.2.2.5).
    ///
    /// 0x00 = Off, 0x01 = On, 0x02 = Toggle, 0xFF = Previous (no change).
    pub fn apply_startup(&mut self, previous_on: bool) {
        let startup = match self.store.get(ATTR_START_UP_ON_OFF) {
            Some(ZclValue::Enum8(v)) => *v,
            _ => 0xFF,
        };
        match startup {
            0x00 => self.set_on_off(false),
            0x01 => self.set_on_off(true),
            0x02 => self.set_on_off(!previous_on),
            _ => self.set_on_off(previous_on), // 0xFF = previous
        }
    }
}

/// Attribute writes go through the cluster so StartUpOnOff is restricted to
/// its defined values (0x00 Off, 0x01 On, 0x02 Toggle, 0xFF Previous).
impl AttributeStoreMutAccess for OnOffCluster {
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
        match (id, value) {
            (ATTR_START_UP_ON_OFF, ZclValue::Enum8(0x03..=0xFE)) => Err(ZclStatus::InvalidValue),
            _ => Ok(()),
        }
    }
}

impl Cluster for OnOffCluster {
    fn cluster_id(&self) -> ClusterId {
        ClusterId::ON_OFF
    }

    fn handle_command(
        &mut self,
        cmd_id: CommandId,
        payload: &[u8],
    ) -> Result<heapless::Vec<u8, 64>, ZclStatus> {
        match cmd_id {
            CMD_OFF => {
                self.turn_off();
                Ok(heapless::Vec::new())
            }
            CMD_ON => {
                self.turn_on();
                Ok(heapless::Vec::new())
            }
            CMD_TOGGLE => {
                if self.is_on() {
                    self.turn_off();
                } else {
                    self.turn_on();
                }
                Ok(heapless::Vec::new())
            }
            CMD_OFF_WITH_EFFECT => {
                if payload.len() < 2 {
                    return Err(ZclStatus::MalformedCommand);
                }
                // Effect ID (u8) + Effect Variant (u8) — we just turn off.
                self.store
                    .set_raw(ATTR_GLOBAL_SCENE_CONTROL, ZclValue::Bool(false))?;
                self.turn_off();
                Ok(heapless::Vec::new())
            }
            CMD_ON_WITH_RECALL_GLOBAL_SCENE => {
                // §3.8.2.3.5: discarded while GlobalSceneControl is TRUE.
                if !matches!(
                    self.store.get(ATTR_GLOBAL_SCENE_CONTROL),
                    Some(ZclValue::Bool(true))
                ) {
                    self.turn_on();
                }
                Ok(heapless::Vec::new())
            }
            CMD_ON_WITH_TIMED_OFF => {
                if payload.len() < 5 {
                    return Err(ZclStatus::MalformedCommand);
                }
                let on_off_control = payload[0];
                let on_time = u16::from_le_bytes([payload[1], payload[2]]);
                let off_wait = u16::from_le_bytes([payload[3], payload[4]]);
                // §3.8.2.3.6. Bit 0 of OnOffControl: "Accept Only When On".
                if on_off_control & 0x01 != 0 && !self.is_on() {
                    return Ok(heapless::Vec::new());
                }
                let cur_wait = self.u16_attr(ATTR_OFF_WAIT_TIME);
                if !self.is_on() && cur_wait > 0 {
                    // Delayed-off guard: only shorten the remaining wait.
                    self.store
                        .set_raw(ATTR_OFF_WAIT_TIME, ZclValue::U16(cur_wait.min(off_wait)))?;
                } else {
                    let cur_on = self.u16_attr(ATTR_ON_TIME);
                    self.turn_on();
                    self.store
                        .set_raw(ATTR_ON_TIME, ZclValue::U16(cur_on.max(on_time)))?;
                    self.store
                        .set_raw(ATTR_OFF_WAIT_TIME, ZclValue::U16(off_wait))?;
                }
                Ok(heapless::Vec::new())
            }
            _ => Err(ZclStatus::UnsupClusterCommand),
        }
    }

    fn attributes(&self) -> &dyn AttributeStoreAccess {
        &self.store
    }

    fn attributes_mut(&mut self) -> &mut dyn AttributeStoreMutAccess {
        self
    }

    fn received_commands(&self) -> heapless::Vec<u8, 32> {
        heapless::Vec::from_slice(&[0x00, 0x01, 0x02, 0x40, 0x41, 0x42]).unwrap_or_default()
    }

    /// `OnOff` and `GlobalSceneControl` are driven exclusively by this
    /// cluster's own commands (Off/On/Toggle/OffWithEffect/
    /// OnWithRecallGlobalScene), so a Basic cluster reset restores them to
    /// their factory-shipped values alongside the writable timers/config.
    fn reset_to_factory_defaults(&mut self) {
        let _ = self.store.set_raw(ATTR_ON_OFF, ZclValue::Bool(false));
        let _ = self
            .store
            .set_raw(ATTR_GLOBAL_SCENE_CONTROL, ZclValue::Bool(true));
        let _ = self.store.set_raw(ATTR_ON_TIME, ZclValue::U16(0));
        let _ = self.store.set_raw(ATTR_OFF_WAIT_TIME, ZclValue::U16(0));
        let _ = self
            .store
            .set_raw(ATTR_START_UP_ON_OFF, ZclValue::Enum8(0xFF));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_restores_nonzero_defaults_and_clears_timers() {
        let mut cluster = OnOffCluster::new();
        cluster.handle_command(CMD_ON, &[]).unwrap();
        // On With Timed Off sets GlobalSceneControl (OnOff becomes TRUE);
        // Off With Effect then clears it while OffWaitTime keeps running.
        cluster
            .handle_command(CMD_ON_WITH_TIMED_OFF, &[0x00, 0x0A, 0x00, 0x05, 0x00])
            .unwrap();
        cluster
            .handle_command(CMD_OFF_WITH_EFFECT, &[0x00, 0x00])
            .unwrap();
        assert!(!cluster.is_on());
        assert_eq!(
            cluster.attributes().get(ATTR_OFF_WAIT_TIME),
            Some(&ZclValue::U16(5))
        );
        assert_eq!(
            cluster.attributes().get(ATTR_GLOBAL_SCENE_CONTROL),
            Some(&ZclValue::Bool(false))
        );

        Cluster::reset_to_factory_defaults(&mut cluster);

        assert!(!cluster.is_on());
        assert_eq!(
            cluster.attributes().get(ATTR_GLOBAL_SCENE_CONTROL),
            Some(&ZclValue::Bool(true))
        );
        assert_eq!(
            cluster.attributes().get(ATTR_ON_TIME),
            Some(&ZclValue::U16(0))
        );
        assert_eq!(
            cluster.attributes().get(ATTR_OFF_WAIT_TIME),
            Some(&ZclValue::U16(0))
        );
        assert_eq!(
            cluster.attributes().get(ATTR_START_UP_ON_OFF),
            Some(&ZclValue::Enum8(0xFF))
        );
    }

    #[test]
    fn elapsed_tick_consumes_on_time_and_remaining_off_wait_time() {
        let mut cluster = OnOffCluster::new();
        cluster
            .handle_command(CMD_ON_WITH_TIMED_OFF, &[0x00, 0x03, 0x00, 0x05, 0x00])
            .unwrap();

        cluster.tick_by(4);

        assert!(!cluster.is_on());
        assert_eq!(
            cluster.attributes().get(ATTR_ON_TIME),
            Some(&ZclValue::U16(0))
        );
        assert_eq!(
            cluster.attributes().get(ATTR_OFF_WAIT_TIME),
            Some(&ZclValue::U16(4))
        );
    }

    fn u16(c: &OnOffCluster, id: AttributeId) -> u16 {
        c.u16_attr(id)
    }

    fn timed(c: &mut OnOffCluster, ctrl: u8, on: u16, wait: u16) {
        let mut p = [ctrl, 0, 0, 0, 0];
        p[1..3].copy_from_slice(&on.to_le_bytes());
        p[3..5].copy_from_slice(&wait.to_le_bytes());
        c.handle_command(CMD_ON_WITH_TIMED_OFF, &p).unwrap();
    }

    #[test]
    fn off_clears_on_time_and_on_clears_off_wait_when_on_time_is_zero() {
        let mut c = OnOffCluster::new();
        timed(&mut c, 0, 50, 30);
        assert_eq!(
            (u16(&c, ATTR_ON_TIME), u16(&c, ATTR_OFF_WAIT_TIME)),
            (50, 30)
        );
        c.handle_command(CMD_OFF, &[]).unwrap();
        assert!(!c.is_on());
        assert_eq!(u16(&c, ATTR_ON_TIME), 0);
        assert_eq!(u16(&c, ATTR_OFF_WAIT_TIME), 30);
        c.handle_command(CMD_ON, &[]).unwrap();
        assert_eq!(u16(&c, ATTR_OFF_WAIT_TIME), 0);
        // Toggle to off also clears OnTime.
        timed(&mut c, 0, 50, 30);
        c.handle_command(CMD_TOGGLE, &[]).unwrap();
        assert_eq!(u16(&c, ATTR_ON_TIME), 0);
    }

    #[test]
    fn on_with_timed_off_follows_the_spec_rules() {
        let mut c = OnOffCluster::new();
        // Accept-only-when-on while off: discarded.
        timed(&mut c, 1, 50, 30);
        assert!(!c.is_on());
        assert_eq!(u16(&c, ATTR_ON_TIME), 0);
        // OnTime = max(current, new).
        timed(&mut c, 0, 50, 30);
        timed(&mut c, 0, 20, 10);
        assert!(c.is_on());
        assert_eq!(
            (u16(&c, ATTR_ON_TIME), u16(&c, ATTR_OFF_WAIT_TIME)),
            (50, 10)
        );
        // Off with OffWaitTime > 0: only shorten the wait, stay off.
        c.handle_command(CMD_OFF, &[]).unwrap();
        timed(&mut c, 0, 70, 40);
        assert!(!c.is_on());
        assert_eq!(u16(&c, ATTR_OFF_WAIT_TIME), 10);
        timed(&mut c, 0, 70, 5);
        assert_eq!(u16(&c, ATTR_OFF_WAIT_TIME), 5);
    }

    #[test]
    fn start_up_on_off_rejects_reserved_values() {
        let mut c = OnOffCluster::new();
        for v in [0x00, 0x01, 0x02, 0xFF] {
            c.attributes_mut()
                .set(ATTR_START_UP_ON_OFF, ZclValue::Enum8(v))
                .unwrap();
        }
        for v in [0x03, 0x80, 0xFE] {
            assert_eq!(
                c.attributes_mut()
                    .set(ATTR_START_UP_ON_OFF, ZclValue::Enum8(v)),
                Err(ZclStatus::InvalidValue)
            );
        }
    }
}
