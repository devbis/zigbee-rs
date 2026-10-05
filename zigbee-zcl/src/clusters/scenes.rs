//! Scenes cluster (0x0005).
//!
//! Implements the ZCL Scenes cluster with a fixed-capacity scene table.
//! Supports Add Scene, View Scene, Remove Scene, Remove All Scenes,
//! Store Scene, Recall Scene, and Get Scene Membership commands.

use crate::attribute::{AttributeAccess, AttributeDefinition, AttributeStore};
use crate::clusters::{AttributeStoreAccess, AttributeStoreMutAccess, Cluster};
use crate::data_types::{ZclDataType, ZclValue};
use crate::{AttributeId, ClusterId, CommandId, ZclStatus};

pub const ATTR_SCENE_COUNT: AttributeId = AttributeId(0x0000);
pub const ATTR_CURRENT_SCENE: AttributeId = AttributeId(0x0001);
pub const ATTR_CURRENT_GROUP: AttributeId = AttributeId(0x0002);
pub const ATTR_SCENE_VALID: AttributeId = AttributeId(0x0003);
pub const ATTR_NAME_SUPPORT: AttributeId = AttributeId(0x0004);
pub const ATTR_LAST_CONFIGURED_BY: AttributeId = AttributeId(0x0005);

// Command IDs (client → server)
pub const CMD_ADD_SCENE: CommandId = CommandId(0x00);
pub const CMD_VIEW_SCENE: CommandId = CommandId(0x01);
pub const CMD_REMOVE_SCENE: CommandId = CommandId(0x02);
pub const CMD_REMOVE_ALL_SCENES: CommandId = CommandId(0x03);
pub const CMD_STORE_SCENE: CommandId = CommandId(0x04);
pub const CMD_RECALL_SCENE: CommandId = CommandId(0x05);
pub const CMD_GET_SCENE_MEMBERSHIP: CommandId = CommandId(0x06);

/// Maximum number of scenes the table can hold.
const MAX_SCENES: usize = 16;
/// Maximum extension data per scene (cluster attribute snapshots).
pub const MAX_EXTENSION_DATA: usize = 32;

/// Extension field set storage for one scene.
pub type ExtensionData = heapless::Vec<u8, MAX_EXTENSION_DATA>;

/// Endpoint services the Scenes cluster needs but does not own (ZCL r8
/// §3.7.2.4): group membership and the scene extension field sets of the
/// other clusters on the same endpoint.
pub trait SceneContext {
    /// Whether this endpoint is a member of `group_id` (never called for
    /// group 0x0000, which is always valid).
    fn is_group_member(&self, group_id: u16) -> bool;
    /// Append the current extension field sets (`cluster id`, `length`,
    /// attribute values) of every scene-capable cluster to `out`.
    /// Returns `InsufficientSpace` when they do not fit.
    fn capture(&mut self, out: &mut ExtensionData) -> Result<(), ZclStatus>;
    /// Apply stored extension field sets with a transition time in 1/10 s.
    fn recall(&mut self, ext: &[u8], transition_ds: u16) -> Result<(), ZclStatus>;
}

/// Context used by the plain [`Cluster::handle_command`] path, which has no
/// access to the rest of the endpoint: group membership is not checked and
/// Store/Recall of non-empty scenes fail instead of claiming success.
struct NoSceneContext;

impl SceneContext for NoSceneContext {
    fn is_group_member(&self, _group_id: u16) -> bool {
        true
    }
    fn capture(&mut self, _out: &mut ExtensionData) -> Result<(), ZclStatus> {
        Err(ZclStatus::Failure)
    }
    fn recall(&mut self, ext: &[u8], _transition_ds: u16) -> Result<(), ZclStatus> {
        if ext.is_empty() {
            Ok(())
        } else {
            Err(ZclStatus::Failure)
        }
    }
}

/// A cluster whose state is part of a scene (OnOff, Level Control, Color
/// Control, ...).
pub trait SceneCapable: Cluster {
    /// Attributes of this cluster's extension field set, in wire order.
    fn scene_attributes(&self) -> &'static [AttributeId];

    /// Apply recalled `(attribute, value)` pairs. The default writes the
    /// values directly (an immediate transition).
    fn scene_apply(&mut self, id: AttributeId, value: ZclValue, _transition_ds: u16) {
        let _ = self.attributes_mut().set_raw(id, value);
    }
}

/// On/Off extension field set: OnOff.
impl SceneCapable for super::on_off::OnOffCluster {
    fn scene_attributes(&self) -> &'static [AttributeId] {
        &[super::on_off::ATTR_ON_OFF]
    }
}

/// Level Control extension field set: CurrentLevel.
impl SceneCapable for super::level_control::LevelControlCluster {
    fn scene_attributes(&self) -> &'static [AttributeId] {
        &[super::level_control::ATTR_CURRENT_LEVEL]
    }
}

/// Color Control extension field set (ZCL r8 §5.2.2.5): CurrentX, CurrentY,
/// EnhancedCurrentHue, CurrentSaturation, ColorLoopActive,
/// ColorLoopDirection, ColorLoopTime, ColorTemperatureMireds.
impl SceneCapable for super::color_control::ColorControlCluster {
    fn scene_attributes(&self) -> &'static [AttributeId] {
        use super::color_control::*;
        &[
            ATTR_CURRENT_X,
            ATTR_CURRENT_Y,
            ATTR_ENHANCED_CURRENT_HUE,
            ATTR_CURRENT_SATURATION,
            ATTR_COLOR_LOOP_ACTIVE,
            ATTR_COLOR_LOOP_DIRECTION,
            ATTR_COLOR_LOOP_TIME,
            ATTR_COLOR_TEMPERATURE_MIREDS,
        ]
    }
}

/// [`SceneContext`] over a set of scene-capable clusters on one endpoint.
pub struct SceneEndpoint<'a, 'b> {
    pub clusters: &'a mut [&'b mut dyn SceneCapable],
    /// Group membership lookup for this endpoint.
    pub is_member: &'a dyn Fn(u16) -> bool,
}

impl SceneContext for SceneEndpoint<'_, '_> {
    fn is_group_member(&self, group_id: u16) -> bool {
        (self.is_member)(group_id)
    }

    fn capture(&mut self, out: &mut ExtensionData) -> Result<(), ZclStatus> {
        for c in self.clusters.iter() {
            let mut set = [0u8; MAX_EXTENSION_DATA];
            let mut len = 0;
            for &id in c.scene_attributes() {
                // The field set is a prefix: stop at the first attribute the
                // cluster does not implement.
                let Some(v) = c.attributes().get(id) else {
                    break;
                };
                len += v
                    .try_serialize(&mut set[len..])
                    .ok_or(ZclStatus::InsufficientSpace)?;
            }
            let mut hdr = [0u8; 3];
            hdr[..2].copy_from_slice(&c.cluster_id().0.to_le_bytes());
            hdr[2] = len as u8;
            out.extend_from_slice(&hdr)
                .and_then(|_| out.extend_from_slice(&set[..len]))
                .map_err(|_| ZclStatus::InsufficientSpace)?;
        }
        Ok(())
    }

    fn recall(&mut self, mut ext: &[u8], transition_ds: u16) -> Result<(), ZclStatus> {
        while ext.len() >= 3 {
            let cid = u16::from_le_bytes([ext[0], ext[1]]);
            let len = ext[2] as usize;
            let mut data = ext.get(3..3 + len).ok_or(ZclStatus::MalformedCommand)?;
            ext = &ext[3 + len..];
            // Field sets for clusters not on this endpoint are ignored.
            let Some(c) = self.clusters.iter_mut().find(|c| c.cluster_id().0 == cid) else {
                continue;
            };
            for &id in c.scene_attributes() {
                let Some(dt) = c.attributes().find(id).map(|d| d.data_type) else {
                    break;
                };
                let Some((v, n)) = ZclValue::deserialize(dt, data) else {
                    break;
                };
                data = &data[n..];
                c.scene_apply(id, v, transition_ds);
            }
        }
        Ok(())
    }
}

/// A single scene table entry.
#[derive(Debug, Clone)]
struct SceneEntry {
    group_id: u16,
    scene_id: u8,
    transition_time: u16,
    extension_data: heapless::Vec<u8, MAX_EXTENSION_DATA>,
    active: bool,
}

impl SceneEntry {
    const fn empty() -> Self {
        Self {
            group_id: 0,
            scene_id: 0,
            transition_time: 0,
            extension_data: heapless::Vec::new(),
            active: false,
        }
    }
}

/// Scenes cluster — full implementation with scene table.
pub struct ScenesCluster {
    store: AttributeStore<8>,
    scenes: [SceneEntry; MAX_SCENES],
}

impl Default for ScenesCluster {
    fn default() -> Self {
        Self::new()
    }
}

impl ScenesCluster {
    pub fn new() -> Self {
        let mut store = AttributeStore::new();
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_SCENE_COUNT,
                data_type: ZclDataType::U8,
                access: AttributeAccess::ReadOnly,
                name: "SceneCount",
            },
            ZclValue::U8(0),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_CURRENT_SCENE,
                data_type: ZclDataType::U8,
                access: AttributeAccess::ReadOnly,
                name: "CurrentScene",
            },
            ZclValue::U8(0),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_CURRENT_GROUP,
                data_type: ZclDataType::U16,
                access: AttributeAccess::ReadOnly,
                name: "CurrentGroup",
            },
            ZclValue::U16(0),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_SCENE_VALID,
                data_type: ZclDataType::Bool,
                access: AttributeAccess::ReadOnly,
                name: "SceneValid",
            },
            ZclValue::Bool(false),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_NAME_SUPPORT,
                data_type: ZclDataType::Bitmap8,
                access: AttributeAccess::ReadOnly,
                name: "NameSupport",
            },
            ZclValue::Bitmap8(0),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_LAST_CONFIGURED_BY,
                data_type: ZclDataType::IeeeAddr,
                access: AttributeAccess::ReadOnly,
                name: "LastConfiguredBy",
            },
            ZclValue::IeeeAddr(0),
        );
        Self {
            store,
            scenes: core::array::from_fn(|_| SceneEntry::empty()),
        }
    }

    /// Number of active scenes.
    pub fn scene_count(&self) -> u8 {
        self.scenes.iter().filter(|s| s.active).count() as u8
    }

    fn update_scene_count(&mut self) {
        let count = self.scene_count();
        let _ = self.store.set_raw(ATTR_SCENE_COUNT, ZclValue::U8(count));
    }

    fn find_scene(&self, group_id: u16, scene_id: u8) -> Option<usize> {
        self.scenes
            .iter()
            .position(|s| s.active && s.group_id == group_id && s.scene_id == scene_id)
    }

    fn find_empty_slot(&self) -> Option<usize> {
        self.scenes.iter().position(|s| !s.active)
    }

    /// Handle a Scenes command with access to the endpoint's groups and
    /// scene-capable clusters.
    pub fn handle_command_with(
        &mut self,
        cmd_id: CommandId,
        payload: &[u8],
        ctx: &mut dyn SceneContext,
    ) -> Result<heapless::Vec<u8, 64>, ZclStatus> {
        if payload.len() < 2 {
            return Err(ZclStatus::MalformedCommand);
        }
        let group_id = u16::from_le_bytes([payload[0], payload[1]]);
        // §3.7.2.4: every command naming a group the endpoint is not a
        // member of fails with INVALID_FIELD.
        let group_ok = group_id == 0 || ctx.is_group_member(group_id);
        match cmd_id {
            CMD_REMOVE_ALL_SCENES => {
                let status = if group_ok {
                    for scene in &mut self.scenes {
                        if scene.group_id == group_id {
                            scene.active = false;
                        }
                    }
                    self.update_scene_count();
                    ZclStatus::Success
                } else {
                    ZclStatus::InvalidField
                };
                Ok(Self::response(status, group_id, None))
            }
            CMD_GET_SCENE_MEMBERSHIP => {
                let capacity = (MAX_SCENES - self.scene_count() as usize) as u8;
                let mut resp = heapless::Vec::new();
                let _ = resp.push(if group_ok {
                    ZclStatus::Success
                } else {
                    ZclStatus::InvalidField
                } as u8);
                let _ = resp.push(capacity);
                let _ = resp.extend_from_slice(&group_id.to_le_bytes());
                if group_ok {
                    let count_at = resp.len();
                    let _ = resp.push(0);
                    for scene in self
                        .scenes
                        .iter()
                        .filter(|s| s.active && s.group_id == group_id)
                    {
                        let _ = resp.push(scene.scene_id);
                        resp[count_at] += 1;
                    }
                }
                Ok(resp)
            }
            CMD_ADD_SCENE | CMD_VIEW_SCENE | CMD_REMOVE_SCENE | CMD_STORE_SCENE
            | CMD_RECALL_SCENE => {
                let scene_id = *payload.get(2).ok_or(ZclStatus::MalformedCommand)?;
                let status = match cmd_id {
                    // Recall Scene has no response command: report errors
                    // through the Default Response.
                    CMD_RECALL_SCENE if !group_ok => return Err(ZclStatus::InvalidField),
                    _ if !group_ok => ZclStatus::InvalidField,
                    CMD_ADD_SCENE => self.add_scene(group_id, scene_id, &payload[3..])?,
                    CMD_VIEW_SCENE => return Ok(self.view_scene(group_id, scene_id)),
                    CMD_REMOVE_SCENE => match self.find_scene(group_id, scene_id) {
                        Some(idx) => {
                            self.scenes[idx].active = false;
                            self.update_scene_count();
                            ZclStatus::Success
                        }
                        None => ZclStatus::NotFound,
                    },
                    CMD_STORE_SCENE => self.store_scene(group_id, scene_id, ctx),
                    _ => {
                        // Recall Scene has no response command; errors go
                        // out as a Default Response.
                        let tt = payload.get(3..5).map(|b| u16::from_le_bytes([b[0], b[1]]));
                        return match self.recall_scene(group_id, scene_id, tt, ctx) {
                            ZclStatus::Success => Ok(heapless::Vec::new()),
                            e => Err(e),
                        };
                    }
                };
                Ok(Self::response(status, group_id, Some(scene_id)))
            }
            _ => Err(ZclStatus::UnsupClusterCommand),
        }
    }

    /// `status(1) + group_id(2) [+ scene_id(1)]` response.
    fn response(status: ZclStatus, group_id: u16, scene_id: Option<u8>) -> heapless::Vec<u8, 64> {
        let mut resp = heapless::Vec::new();
        let _ = resp.push(status as u8);
        let _ = resp.extend_from_slice(&group_id.to_le_bytes());
        if let Some(id) = scene_id {
            let _ = resp.push(id);
        }
        resp
    }

    /// Add Scene body after group/scene id: transition_time(2) + name +
    /// extension field sets.
    fn add_scene(
        &mut self,
        group_id: u16,
        scene_id: u8,
        body: &[u8],
    ) -> Result<ZclStatus, ZclStatus> {
        let malformed = ZclStatus::MalformedCommand;
        let tt = body.get(..2).ok_or(malformed)?;
        let transition_time = u16::from_le_bytes([tt[0], tt[1]]);
        let name_len = *body.get(2).ok_or(malformed)? as usize;
        // The name (not stored: NameSupport = 0) must be complete, or the
        // extension field sets would be misparsed.
        let ext = body.get(3 + name_len..).ok_or(malformed)?;
        let Ok(extension_data) = ExtensionData::from_slice(ext) else {
            return Ok(ZclStatus::InsufficientSpace);
        };
        let Some(idx) = self
            .find_scene(group_id, scene_id)
            .or_else(|| self.find_empty_slot())
        else {
            return Ok(ZclStatus::InsufficientSpace);
        };
        self.scenes[idx] = SceneEntry {
            group_id,
            scene_id,
            transition_time,
            extension_data,
            active: true,
        };
        self.update_scene_count();
        Ok(ZclStatus::Success)
    }

    fn view_scene(&self, group_id: u16, scene_id: u8) -> heapless::Vec<u8, 64> {
        match self.find_scene(group_id, scene_id) {
            Some(idx) => {
                let s = &self.scenes[idx];
                let mut resp = Self::response(ZclStatus::Success, group_id, Some(scene_id));
                let _ = resp.extend_from_slice(&s.transition_time.to_le_bytes());
                let _ = resp.push(0); // name length = 0 (no name support)
                let _ = resp.extend_from_slice(&s.extension_data);
                resp
            }
            None => Self::response(ZclStatus::NotFound, group_id, Some(scene_id)),
        }
    }

    /// Store Scene: snapshot the endpoint's scene-capable clusters.
    fn store_scene(
        &mut self,
        group_id: u16,
        scene_id: u8,
        ctx: &mut dyn SceneContext,
    ) -> ZclStatus {
        let existing = self.find_scene(group_id, scene_id);
        let Some(idx) = existing.or_else(|| self.find_empty_slot()) else {
            return ZclStatus::InsufficientSpace;
        };
        let mut ext = ExtensionData::new();
        if let Err(status) = ctx.capture(&mut ext) {
            return status;
        }
        let transition_time = match existing {
            Some(i) => self.scenes[i].transition_time,
            None => 0,
        };
        self.scenes[idx] = SceneEntry {
            group_id,
            scene_id,
            transition_time,
            extension_data: ext,
            active: true,
        };
        self.set_current(group_id, scene_id);
        self.update_scene_count();
        ZclStatus::Success
    }

    /// Recall Scene: apply the stored extension field sets. `transition`
    /// (1/10 s) overrides the scene's own transition time unless 0xFFFF.
    fn recall_scene(
        &mut self,
        group_id: u16,
        scene_id: u8,
        transition: Option<u16>,
        ctx: &mut dyn SceneContext,
    ) -> ZclStatus {
        let Some(idx) = self.find_scene(group_id, scene_id) else {
            return ZclStatus::NotFound;
        };
        let scene = &self.scenes[idx];
        let tt = match transition {
            Some(t) if t != 0xFFFF => t,
            _ => scene.transition_time.saturating_mul(10),
        };
        if let Err(status) = ctx.recall(&scene.extension_data, tt) {
            return status;
        }
        self.set_current(group_id, scene_id);
        ZclStatus::Success
    }

    fn set_current(&mut self, group_id: u16, scene_id: u8) {
        let _ = self
            .store
            .set_raw(ATTR_CURRENT_SCENE, ZclValue::U8(scene_id));
        let _ = self
            .store
            .set_raw(ATTR_CURRENT_GROUP, ZclValue::U16(group_id));
        let _ = self.store.set_raw(ATTR_SCENE_VALID, ZclValue::Bool(true));
    }
}

impl Cluster for ScenesCluster {
    fn cluster_id(&self) -> ClusterId {
        ClusterId::SCENES
    }

    fn handle_command(
        &mut self,
        cmd_id: CommandId,
        payload: &[u8],
    ) -> Result<heapless::Vec<u8, 64>, ZclStatus> {
        self.handle_command_with(cmd_id, payload, &mut NoSceneContext)
    }

    fn attributes(&self) -> &dyn AttributeStoreAccess {
        &self.store
    }
    fn attributes_mut(&mut self) -> &mut dyn AttributeStoreMutAccess {
        &mut self.store
    }

    fn received_commands(&self) -> heapless::Vec<u8, 32> {
        heapless::Vec::from_slice(&[0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06]).unwrap_or_default()
    }

    fn generated_commands(&self) -> heapless::Vec<u8, 32> {
        heapless::Vec::from_slice(&[0x00, 0x01, 0x02, 0x03, 0x04, 0x06]).unwrap_or_default()
    }

    /// The scene table and its tracking attributes are owned entirely by
    /// this cluster (not an APS binding/group relationship), so a Basic
    /// cluster reset clears it back to the fresh-out-of-box state — the
    /// same effect as `RemoveAllScenes` for every group.
    fn reset_to_factory_defaults(&mut self) {
        for scene in &mut self.scenes {
            *scene = SceneEntry::empty();
        }
        let _ = self.store.set_raw(ATTR_SCENE_COUNT, ZclValue::U8(0));
        let _ = self.store.set_raw(ATTR_CURRENT_SCENE, ZclValue::U8(0));
        let _ = self.store.set_raw(ATTR_CURRENT_GROUP, ZclValue::U16(0));
        let _ = self.store.set_raw(ATTR_SCENE_VALID, ZclValue::Bool(false));
        let _ = self
            .store
            .set_raw(ATTR_LAST_CONFIGURED_BY, ZclValue::IeeeAddr(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clusters::level_control::{ATTR_CURRENT_LEVEL, LevelControlCluster};
    use crate::clusters::on_off::{CMD_OFF, CMD_ON, OnOffCluster};

    fn add_payload(group: u16, scene: u8, ext: &[u8]) -> heapless::Vec<u8, 128> {
        let mut p = heapless::Vec::new();
        p.extend_from_slice(&group.to_le_bytes()).unwrap();
        p.extend_from_slice(&[scene, 5, 0, 0]).unwrap(); // tt = 5 s, empty name
        p.extend_from_slice(ext).unwrap();
        p
    }

    #[test]
    fn plain_handle_command_never_claims_a_store_it_cannot_do() {
        let mut s = ScenesCluster::new();
        let rsp = s.handle_command(CMD_STORE_SCENE, &[0, 0, 1]).unwrap();
        assert_eq!(rsp[0], ZclStatus::Failure as u8);
        assert_eq!(s.scene_count(), 0);
        // A scene with extension data cannot be recalled without a context.
        let rsp = s
            .handle_command(CMD_ADD_SCENE, &add_payload(0, 2, &[6, 0, 1, 1]))
            .unwrap();
        assert_eq!(rsp[0], ZclStatus::Success as u8);
        assert_eq!(
            s.handle_command(CMD_RECALL_SCENE, &[0, 0, 2]),
            Err(ZclStatus::Failure)
        );
        assert_eq!(
            s.handle_command(CMD_RECALL_SCENE, &[0, 0, 9]),
            Err(ZclStatus::NotFound)
        );
    }

    #[test]
    fn oversized_extension_data_is_insufficient_space() {
        let mut s = ScenesCluster::new();
        let rsp = s
            .handle_command(
                CMD_ADD_SCENE,
                &add_payload(0, 1, &[0u8; MAX_EXTENSION_DATA + 1]),
            )
            .unwrap();
        assert_eq!(rsp[0], ZclStatus::InsufficientSpace as u8);
        assert_eq!(s.scene_count(), 0);
        // Truncated name is malformed.
        assert_eq!(
            s.handle_command(CMD_ADD_SCENE, &[0, 0, 1, 0, 0, 4, b'a']),
            Err(ZclStatus::MalformedCommand)
        );
    }

    #[test]
    fn store_and_recall_round_trip_through_scene_capable_clusters() {
        let mut s = ScenesCluster::new();
        let mut on_off = OnOffCluster::new();
        let mut level = LevelControlCluster::new();
        let member = |g: u16| g == 0x0010;
        on_off.handle_command(CMD_ON, &[]).unwrap();
        level
            .handle_command(CommandId(0x00), &[0x80, 0, 0])
            .unwrap();
        {
            let mut ep = SceneEndpoint {
                clusters: &mut [&mut on_off, &mut level],
                is_member: &member,
            };
            let rsp = s
                .handle_command_with(CMD_STORE_SCENE, &[0x10, 0x00, 3], &mut ep)
                .unwrap();
            assert_eq!(rsp[0], ZclStatus::Success as u8);
            // Non-member group → INVALID_FIELD.
            let rsp = s
                .handle_command_with(CMD_STORE_SCENE, &[0x20, 0x00, 3], &mut ep)
                .unwrap();
            assert_eq!(rsp[0], ZclStatus::InvalidField as u8);
            assert_eq!(
                s.handle_command_with(CMD_RECALL_SCENE, &[0x20, 0x00, 3], &mut ep),
                Err(ZclStatus::InvalidField)
            );
            let rsp = s
                .handle_command_with(CMD_GET_SCENE_MEMBERSHIP, &[0x20, 0x00], &mut ep)
                .unwrap();
            assert_eq!(rsp[0], ZclStatus::InvalidField as u8);
        }
        // View shows the captured field sets: OnOff=1, CurrentLevel=0x80.
        let view = s.handle_command(CMD_VIEW_SCENE, &[0x10, 0x00, 3]).unwrap();
        assert_eq!(&view[7..], &[0x06, 0x00, 1, 1, 0x08, 0x00, 1, 0x80]);

        on_off.handle_command(CMD_OFF, &[]).unwrap();
        level
            .handle_command(CommandId(0x00), &[0x10, 0, 0])
            .unwrap();
        let mut ep = SceneEndpoint {
            clusters: &mut [&mut on_off, &mut level],
            is_member: &member,
        };
        s.handle_command_with(CMD_RECALL_SCENE, &[0x10, 0x00, 3], &mut ep)
            .unwrap();
        assert!(on_off.is_on());
        assert_eq!(
            level.attributes().get(ATTR_CURRENT_LEVEL),
            Some(&ZclValue::U8(0x80))
        );
        assert_eq!(
            s.attributes().get(ATTR_SCENE_VALID),
            Some(&ZclValue::Bool(true))
        );
    }

    #[test]
    fn capture_exceeding_capacity_is_insufficient_space() {
        use crate::clusters::color_control::ColorControlCluster;
        let mut s = ScenesCluster::new();
        let mut a = ColorControlCluster::new();
        let mut b = ColorControlCluster::new();
        let mut c = ColorControlCluster::new();
        let member = |_: u16| true;
        // 3 × (3 header + 13 value bytes) = 48 > MAX_EXTENSION_DATA.
        let mut ep = SceneEndpoint {
            clusters: &mut [&mut a, &mut b, &mut c],
            is_member: &member,
        };
        let rsp = s
            .handle_command_with(CMD_STORE_SCENE, &[0, 0, 1], &mut ep)
            .unwrap();
        assert_eq!(rsp[0], ZclStatus::InsufficientSpace as u8);
        assert_eq!(s.scene_count(), 0);
    }
}
