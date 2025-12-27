use crate::types::price::Price;
use crate::types::quantity::Quantity;

pub type OrderId = u64;

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Bid,
    Ask,
}

/// 17 Bytes
/// This will be padded with additional 7 bytes due to the largest field alignment
/// So Order is 24 bytes
#[derive(Clone, Copy)]
pub struct Order {
    // 8 byte
    // Id will also serve as a sequencer
    id: OrderId,
    //1 byte
    side: Side,
    // 4 byte
    price: Price,
    // 4 byte
    quantity: Quantity,
}

pub struct IdCounter(u64);

impl IdCounter {
    pub fn new() -> Self {
        Self(0)
    }

    pub fn next(&mut self) -> u64 {
        let current = self.0;
        self.0 += 1;
        current
    }
}

impl Order {
    pub fn new(price: Price, quantity: Quantity, side: Side, id_counter: &mut IdCounter) -> Self {
        let id = id_counter.next();

        Order {
            id,
            price,
            quantity,
            side,
        }
    }
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn price(&self) -> Price {
        self.price
    }
    pub fn quantity(&self) -> Quantity {
        self.quantity
    }
    pub fn side(&self) -> Side {
        self.side
    }
}
