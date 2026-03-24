//! Network Information Base (NIB).
//!
//! The NIB stores all NWK-layer configuration and state.
//! It's the NWK equivalent of the MAC PIB.

use zigbee_types::*;

/// NWK Information Base — all NWK layer state.
#[derive(Debug)]
pub struct Nib {
    // ── Network identity ────────────────────────────────
    /// Extended PAN ID of the network (64-bit)
    pub extended_pan_id: IeeeAddress,
    /// Short (16-bit) PAN ID
    pub pan_id: PanId,
    /// Own network (short) address
    pub network_address: ShortAddress,
    /// Operating channel (11-26)
    pub logical_channel: u8,

    // ── Network parameters ──────────────────────────────
    /// Stack profile: 0x02 = Zigbee PRO
    pub stack_profile: u8,
    /// Network depth of this device
    pub depth: u8,
    /// Maximum depth for the network
    pub max_depth: u8,
    /// Maximum number of child routers
    pub max_routers: u8,
    /// Maximum number of child end devices
    pub max_children: u8,
    /// Network update ID
    pub update_id: u8,

    // ── Addressing ──────────────────────────────────────
    /// Own IEEE (extended) address
    pub ieee_address: IeeeAddress,
    /// Parent's short address
    pub parent_address: ShortAddress,
    /// Short address assignment method
    pub address_assign: AddressAssignMethod,

    // ── Timing ──────────────────────────────────────────
    /// Network broadcast delivery time (in half-seconds)
    pub broadcast_delivery_time: u8,
    /// Passive ack timeout (ms)
    pub passive_ack_timeout: u16,
    /// Max broadcast retries
    pub max_broadcast_retries: u8,
    /// Transaction persistence time (ms)
    pub transaction_persistence_time: u16,

    // ── Routing ─────────────────────────────────────────
    /// Use tree routing (vs mesh-only)
    pub use_tree_routing: bool,
    /// Use source routing
    pub source_routing: bool,
    /// Route discovery retries
    pub route_discovery_retries: u8,

    // ── Security ────────────────────────────────────────
    /// Security level (0=none, 5=ENC-MIC-32, typical for Zigbee)
    pub security_level: u8,
    /// Whether NWK security is enabled
    pub security_enabled: bool,
    /// Active network key index
    pub active_key_seq_number: u8,
    /// NWK frame counter (outgoing)
    pub outgoing_frame_counter: u32,

    // ── Sequences ───────────────────────────────────────
    /// NWK sequence number
    pub sequence_number: u8,
    /// Route request ID counter
    pub route_request_id: u8,

    // ── Permit joining ──────────────────────────────────
    /// Whether new devices can join through this device
    pub permit_joining: bool,
    /// Time remaining for permit joining (0 = permanent, 0xFF = permanent)
    pub permit_joining_duration: u8,
}

/// How short addresses are assigned
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressAssignMethod {
    /// Tree-based (CSkip algorithm)
    TreeBased,
    /// Stochastic (random, check for conflicts)
    Stochastic,
}

impl Nib {
    /// Create a new NIB with default values.
    pub fn new() -> Self {
        Self {
            extended_pan_id: [0u8; 8],
            pan_id: PanId(0xFFFF),
            network_address: ShortAddress(0xFFFF),
            logical_channel: 0,
            stack_profile: 0x02, // Zigbee PRO
            depth: 0,
            max_depth: 15,
            max_routers: 5,
            max_children: 20,
            update_id: 0,
            ieee_address: [0u8; 8],
            parent_address: ShortAddress(0xFFFF),
            address_assign: AddressAssignMethod::Stochastic,
            broadcast_delivery_time: 9,
            passive_ack_timeout: 500,
            max_broadcast_retries: 3,
            transaction_persistence_time: 500,
            use_tree_routing: false,
            source_routing: false,
            route_discovery_retries: 3,
            security_level: 5, // ENC-MIC-32 (standard Zigbee)
            security_enabled: true,
            active_key_seq_number: 0,
            outgoing_frame_counter: 0,
            sequence_number: 0,
            route_request_id: 0,
            permit_joining: false,
            permit_joining_duration: 0,
        }
    }

    /// Get the next NWK sequence number (wrapping).
    pub fn next_seq(&mut self) -> u8 {
        let seq = self.sequence_number;
        self.sequence_number = self.sequence_number.wrapping_add(1);
        seq
    }

    /// Get the next route request ID.
    pub fn next_route_request_id(&mut self) -> u8 {
        let id = self.route_request_id;
        self.route_request_id = self.route_request_id.wrapping_add(1);
        id
    }

    /// Increment outgoing frame counter. Returns the pre-increment value.
    pub fn next_frame_counter(&mut self) -> u32 {
        let fc = self.outgoing_frame_counter;
        self.outgoing_frame_counter = self.outgoing_frame_counter.wrapping_add(1);
        fc
    }
}

impl Default for Nib {
    fn default() -> Self {
        Self::new()
    }
}
