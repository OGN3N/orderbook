use crate::types::order::Order;

/// Max price is represented in cents - $100 is max price
const MAX_PRICE: u32 = 10000;
const TICK_SIZE: u8 = 1;
const ELEMENT_NUM: usize = MAX_PRICE as usize / TICK_SIZE as usize;


// Empty Orderbook: 1000 * 2 * 24 =  48000 bytes or 48 KB
// + 24 bytes per order

// Orderbook with 10 orders per level (20,000 orders): 10,000 * 2 * 24 + 48,000 = 528000 b or 5.28 MB
pub struct Orderbook {
    bids: Box<[Level; ELEMENT_NUM]>,
    asks: Box<[Level; ELEMENT_NUM]>,
}

/// Level Memory: H(24) + N * 24
#[derive(Default)]
pub struct Level {
    /// Vec of 24 bytes per element
    /// Vec header (ptr: *mut Order: 8b, len: usize, cap: usize)
    orders: Vec<Order>,
}

impl Orderbook {
    pub fn new() -> Self {
        Self {
            bids: Box::new(std::array::from_fn(|_| Level::default())),
            asks: Box::new(std::array::from_fn(|_| Level::default())),
        }
    }
}
