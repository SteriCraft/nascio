//! Flow direction codes of the `flow_dir` layer.

/// D8 neighbour offsets `(di, dj)`, in a fixed order; the index is the flow direction code.
pub const D8: [(i64, i64); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

/// Flow direction code of a base level (sea or sink: no receiver).
pub const NO_DIR: u8 = 255;
