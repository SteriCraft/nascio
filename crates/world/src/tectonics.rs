//! Codes of the tectonic layers.

/// Crust types.
pub const OCEANIC: u8 = 0;
pub const CONTINENTAL: u8 = 1;

/// Bits of the `history_flags` layer.
pub mod flags {
    pub const OROGENY: u8 = 1 << 0;
    pub const ARC: u8 = 1 << 1;
    pub const RIFT: u8 = 1 << 2;
    pub const HOTSPOT: u8 = 1 << 3;
    pub const OPHIOLITE: u8 = 1 << 4;
    pub const TRENCH: u8 = 1 << 5;
    pub const RIDGE: u8 = 1 << 6;
}
