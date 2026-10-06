//! Groups cluster (0x0004).

use crate::attribute::{AttributeAccess, AttributeDefinition, AttributeStore};
use crate::clusters::{AttributeStoreAccess, AttributeStoreMutAccess, Cluster};
use crate::data_types::{ZclDataType, ZclValue};
use crate::{AttributeId, ClusterId, CommandId, ZclStatus};

// Attribute IDs
pub const ATTR_NAME_SUPPORT: AttributeId = AttributeId(0x0000);

// Command IDs (client to server)
pub const CMD_ADD_GROUP: CommandId = CommandId(0x00);
pub const CMD_VIEW_GROUP: CommandId = CommandId(0x01);
pub const CMD_GET_GROUP_MEMBERSHIP: CommandId = CommandId(0x02);
pub const CMD_REMOVE_GROUP: CommandId = CommandId(0x03);
pub const CMD_REMOVE_ALL_GROUPS: CommandId = CommandId(0x04);
pub const CMD_ADD_GROUP_IF_IDENTIFYING: CommandId = CommandId(0x05);

// Response command IDs (server to client)
pub const CMD_ADD_GROUP_RESPONSE: CommandId = CommandId(0x00);
pub const CMD_VIEW_GROUP_RESPONSE: CommandId = CommandId(0x01);
pub const CMD_GET_GROUP_MEMBERSHIP_RESPONSE: CommandId = CommandId(0x02);
pub const CMD_REMOVE_GROUP_RESPONSE: CommandId = CommandId(0x03);

/// Maximum number of groups a device can belong to.
pub const MAX_GROUPS: usize = 16;

/// Actions that the Groups cluster triggers for the APS group table.
/// The runtime or application should apply these to the APS layer.
#[derive(Debug, Clone)]
pub enum GroupAction {
    /// Group was added — call APSME-ADD-GROUP
    Added(u16),
    /// Group was removed — call APSME-REMOVE-GROUP
    Removed(u16),
    /// All groups removed — call APSME-REMOVE-ALL-GROUPS
    RemovedAll,
    /// No APS action needed
    None,
}

/// Group IDs outside 0x0001..=0xFFF7 are invalid (ZCL r8 §3.6.2.3) and are
/// answered with INVALID_VALUE.
#[inline]
pub fn valid_group_id(group_id: u16) -> bool {
    (0x0001..=0xFFF7).contains(&group_id)
}

/// Groups cluster implementation.
pub struct GroupsCluster {
    store: AttributeStore<4>,
    /// List of group IDs this endpoint belongs to.
    groups: heapless::Vec<u16, MAX_GROUPS>,
    /// Last group action — consumed by runtime to sync APS group table.
    last_action: GroupAction,
}

impl Default for GroupsCluster {
    fn default() -> Self {
        Self::new()
    }
}

impl GroupsCluster {
    pub fn new() -> Self {
        let mut store = AttributeStore::new();
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_NAME_SUPPORT,
                data_type: ZclDataType::Bitmap8,
                access: AttributeAccess::ReadOnly,
                name: "NameSupport",
            },
            ZclValue::Bitmap8(0x00), // Group names not supported
        );
        Self {
            store,
            groups: heapless::Vec::new(),
            last_action: GroupAction::None,
        }
    }

    fn add_group(&mut self, group_id: u16) -> u8 {
        if !valid_group_id(group_id) {
            self.last_action = GroupAction::None;
            return ZclStatus::InvalidValue as u8;
        }
        if self.groups.contains(&group_id) {
            self.last_action = GroupAction::None;
            return 0x8A; // DUPLICATE_EXISTS
        }
        match self.groups.push(group_id) {
            Ok(()) => {
                self.last_action = GroupAction::Added(group_id);
                ZclStatus::Success as u8
            }
            Err(_) => {
                self.last_action = GroupAction::None;
                ZclStatus::InsufficientSpace as u8
            }
        }
    }

    /// Add a group externally (called by runtime for AddGroupIfIdentifying).
    /// Does not trigger GroupAction — caller is responsible for APS table sync.
    pub fn add_group_external(&mut self, group_id: u16) -> u8 {
        if !valid_group_id(group_id) {
            return ZclStatus::InvalidValue as u8;
        }
        if self.groups.contains(&group_id) {
            return 0x8A; // DUPLICATE_EXISTS
        }
        match self.groups.push(group_id) {
            Ok(()) => ZclStatus::Success as u8,
            Err(_) => ZclStatus::InsufficientSpace as u8,
        }
    }

    fn remove_group(&mut self, group_id: u16) -> u8 {
        if !valid_group_id(group_id) {
            self.last_action = GroupAction::None;
            return ZclStatus::InvalidValue as u8;
        }
        if let Some(pos) = self.groups.iter().position(|&g| g == group_id) {
            self.groups.swap_remove(pos);
            self.last_action = GroupAction::Removed(group_id);
            ZclStatus::Success as u8
        } else {
            self.last_action = GroupAction::None;
            ZclStatus::NotFound as u8
        }
    }

    /// Take the last group action (consumed once).
    pub fn take_action(&mut self) -> GroupAction {
        core::mem::replace(&mut self.last_action, GroupAction::None)
    }
}

impl Cluster for GroupsCluster {
    fn cluster_id(&self) -> ClusterId {
        ClusterId::GROUPS
    }

    fn handle_command(
        &mut self,
        cmd_id: CommandId,
        payload: &[u8],
    ) -> Result<heapless::Vec<u8, 64>, ZclStatus> {
        match cmd_id {
            CMD_ADD_GROUP => {
                if payload.len() < 2 {
                    return Err(ZclStatus::MalformedCommand);
                }
                let group_id = u16::from_le_bytes([payload[0], payload[1]]);
                let status = self.add_group(group_id);
                let mut resp = heapless::Vec::new();
                let _ = resp.push(status);
                let b = group_id.to_le_bytes();
                let _ = resp.push(b[0]);
                let _ = resp.push(b[1]);
                Ok(resp)
            }
            CMD_VIEW_GROUP => {
                if payload.len() < 2 {
                    return Err(ZclStatus::MalformedCommand);
                }
                let group_id = u16::from_le_bytes([payload[0], payload[1]]);
                let status = if !valid_group_id(group_id) {
                    ZclStatus::InvalidValue as u8
                } else if self.groups.contains(&group_id) {
                    ZclStatus::Success as u8
                } else {
                    ZclStatus::NotFound as u8
                };
                let mut resp = heapless::Vec::new();
                let _ = resp.push(status);
                let b = group_id.to_le_bytes();
                let _ = resp.push(b[0]);
                let _ = resp.push(b[1]);
                let _ = resp.push(0); // empty group name
                Ok(resp)
            }
            CMD_GET_GROUP_MEMBERSHIP => {
                let mut resp = heapless::Vec::new();
                // Capacity
                let _ = resp.push((MAX_GROUPS - self.groups.len()) as u8);
                if payload.is_empty() || payload[0] == 0 {
                    // Return all groups
                    let _ = resp.push(self.groups.len() as u8);
                    for &gid in &self.groups {
                        let b = gid.to_le_bytes();
                        let _ = resp.push(b[0]);
                        let _ = resp.push(b[1]);
                    }
                } else {
                    let count = payload[0] as usize;
                    let mut matched: heapless::Vec<u16, MAX_GROUPS> = heapless::Vec::new();
                    let mut i = 1;
                    for _ in 0..count {
                        if i + 1 < payload.len() {
                            let gid = u16::from_le_bytes([payload[i], payload[i + 1]]);
                            if self.groups.contains(&gid) {
                                let _ = matched.push(gid);
                            }
                            i += 2;
                        }
                    }
                    let _ = resp.push(matched.len() as u8);
                    for &gid in &matched {
                        let b = gid.to_le_bytes();
                        let _ = resp.push(b[0]);
                        let _ = resp.push(b[1]);
                    }
                }
                Ok(resp)
            }
            CMD_REMOVE_GROUP => {
                if payload.len() < 2 {
                    return Err(ZclStatus::MalformedCommand);
                }
                let group_id = u16::from_le_bytes([payload[0], payload[1]]);
                let status = self.remove_group(group_id);
                let mut resp = heapless::Vec::new();
                let _ = resp.push(status);
                let b = group_id.to_le_bytes();
                let _ = resp.push(b[0]);
                let _ = resp.push(b[1]);
                Ok(resp)
            }
            CMD_REMOVE_ALL_GROUPS => {
                self.groups.clear();
                self.last_action = GroupAction::RemovedAll;
                Ok(heapless::Vec::new())
            }
            CMD_ADD_GROUP_IF_IDENTIFYING => {
                // Don't add to cluster's internal group list here.
                // The runtime handles this: checks IdentifyTime > 0, then adds
                // to both APS group table AND calls add_group_external().
                // This avoids state mismatch where cluster list has groups
                // that the APS table doesn't (when IdentifyTime == 0).
                if payload.len() < 2 {
                    return Err(ZclStatus::MalformedCommand);
                }
                // No response for this command per ZCL spec
                self.last_action = GroupAction::None;
                Ok(heapless::Vec::new())
            }
            _ => Err(ZclStatus::UnsupClusterCommand),
        }
    }

    fn attributes(&self) -> &dyn AttributeStoreAccess {
        &self.store
    }

    fn attributes_mut(&mut self) -> &mut dyn AttributeStoreMutAccess {
        &mut self.store
    }

    fn received_commands(&self) -> heapless::Vec<u8, 32> {
        heapless::Vec::from_slice(&[0x00, 0x01, 0x02, 0x03, 0x04, 0x05]).unwrap_or_default()
    }

    fn generated_commands(&self) -> heapless::Vec<u8, 32> {
        heapless::Vec::from_slice(&[0x00, 0x01, 0x02, 0x03]).unwrap_or_default()
    }

    /// `NameSupport` is read-only, and group membership mirrors the APS
    /// group table, which a Basic cluster reset MUST NOT touch (that is the
    /// job of the full BDB factory-reset procedure). Deliberate no-op.
    fn reset_to_factory_defaults(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_ids_outside_valid_range_are_invalid_value() {
        let mut g = GroupsCluster::new();
        for gid in [0x0000u16, 0xFFF8, 0xFFFF] {
            let p = gid.to_le_bytes();
            for cmd in [CMD_ADD_GROUP, CMD_VIEW_GROUP, CMD_REMOVE_GROUP] {
                let rsp = g.handle_command(cmd, &p).unwrap();
                assert_eq!(rsp[0], ZclStatus::InvalidValue as u8, "{cmd:?} {gid:#x}");
            }
            assert!(matches!(g.take_action(), GroupAction::None));
            assert_eq!(g.add_group_external(gid), ZclStatus::InvalidValue as u8);
        }
        for gid in [0x0001u16, 0xFFF7] {
            let rsp = g.handle_command(CMD_ADD_GROUP, &gid.to_le_bytes()).unwrap();
            assert_eq!(rsp[0], ZclStatus::Success as u8);
        }
    }

    #[test]
    fn name_support_is_a_bitmap8() {
        let g = GroupsCluster::new();
        assert_eq!(
            g.attributes().find(ATTR_NAME_SUPPORT).unwrap().data_type,
            ZclDataType::Bitmap8
        );
        assert_eq!(
            g.attributes().get(ATTR_NAME_SUPPORT),
            Some(&ZclValue::Bitmap8(0))
        );
    }
}
