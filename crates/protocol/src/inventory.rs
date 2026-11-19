//! The player's inventory in walk mode: [`SLOTS`] slots of at most [`STACK`] blocks of one
//! kind (author's decisions). The server holds it and sends it whole
//! ([`crate::messages::ServerMsg::Inventory`]); in god mode it is not used (unlimited blocks).

use serde::{Deserialize, Serialize};

/// Slots of the bar.
pub const SLOTS: usize = 10;
/// Most blocks of one slot.
pub const STACK: u8 = 32;

/// One slot: a block of the session's table and how many (1 to [`STACK`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    pub block: u16,
    pub count: u8,
}

/// The slots, left to right.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    pub slots: [Option<Stack>; SLOTS],
}

impl Inventory {
    /// Adds one block: to the first slot of that block not full, else to the first empty
    /// slot. False when there is no room (the block is lost).
    pub fn add(&mut self, block: u16) -> bool {
        if let Some(s) = self
            .slots
            .iter_mut()
            .flatten()
            .find(|s| s.block == block && s.count < STACK)
        {
            s.count += 1;
            return true;
        }
        match self.slots.iter_mut().find(|s| s.is_none()) {
            Some(s) => {
                *s = Some(Stack { block, count: 1 });
                true
            }
            None => false,
        }
    }

    /// Takes one block of `block` from slot `slot`; false if the slot does not hold it.
    pub fn take(&mut self, slot: usize, block: u16) -> bool {
        let Some(cell) = self.slots.get_mut(slot) else {
            return false;
        };
        match cell {
            Some(s) if s.block == block && s.count > 0 => {
                s.count -= 1;
                if s.count == 0 {
                    *cell = None;
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_stack_overflow_and_are_lost_when_full() {
        let mut inv = Inventory::default();
        for _ in 0..STACK {
            assert!(inv.add(4));
        }
        assert_eq!(
            inv.slots[0],
            Some(Stack {
                block: 4,
                count: STACK
            })
        );
        // A full stack goes on in the next empty slot.
        assert!(inv.add(4));
        assert_eq!(inv.slots[1], Some(Stack { block: 4, count: 1 }));
        assert!(inv.add(7));
        assert_eq!(inv.slots[2], Some(Stack { block: 7, count: 1 }));
        // Taking empties a slot, which the next new block fills.
        assert!(!inv.take(2, 4), "slot 2 holds block 7");
        assert!(inv.take(2, 7));
        assert_eq!(inv.slots[2], None);
        assert!(!inv.take(2, 7) && !inv.take(SLOTS, 7));
        assert!(inv.add(9));
        assert_eq!(inv.slots[2], Some(Stack { block: 9, count: 1 }));
        // Room left in slot 1 for block 4; the rest filled with other blocks.
        for b in 10..17 {
            assert!(inv.add(b));
        }
        assert!(!inv.add(99), "no room: lost");
        assert!(inv.add(4), "slot 1 still has room");
    }
}
