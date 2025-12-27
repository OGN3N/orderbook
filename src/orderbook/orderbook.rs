use crate::types::order::Order;
use crate::types::order::OrderId;
use crate::types::order::Side;
use crate::types::price::Price;
use std::collections::HashMap;

/// Max price is represented in cents - $100 is max price
const MAX_PRICE: u32 = 10000;
const TICK_SIZE: u32 = 1;
const LOT_SIZE: u32 = 1;
const ELEMENT_NUM: usize = MAX_PRICE as usize / TICK_SIZE as usize;

// Empty Orderbook: 1000 * 2 * 24 =  48000 bytes or 48 KB
// + 24 bytes per order

// Orderbook with 10 orders per level (20,000 orders): 10,000 * 2 * 24 + 48,000 = 528000 b or 5.28 MB
pub struct Orderbook {
    bids: Box<[Level; ELEMENT_NUM]>,
    asks: Box<[Level; ELEMENT_NUM]>,
    order_index: HashMap<OrderId, (Side, Price)>,
}

/// Level Memory: H(24) + N * 24
#[derive(Default)]
pub struct Level {
    /// Vec of 24 bytes per element
    /// Vec header (ptr: *mut Order: 8bytes, len: usize(8bytes), cap: usize(8bytes))
    /// usize on 64-bit system is 8 bytes because its addresses are pointer sized
    orders: Vec<Order>,
}

impl Orderbook {
    pub fn new() -> Self {
        Self {
            bids: Box::new(std::array::from_fn(|_| Level::default())),
            asks: Box::new(std::array::from_fn(|_| Level::default())),
            order_index: HashMap::new(),
        }
    }

    pub fn add_order(&mut self, order: Order) -> Result<(), String> {
        let order_id = order.id();
        let side = order.side();
        let price_value = order.price().value();
        let quantity_value = order.quantity().value();

        // Validation 1: Price must be multiple of tick size
        if price_value % TICK_SIZE as u32 != 0 {
            return Err(format!(
                "Price {} is not a valid tick (tick_size={})",
                price_value, TICK_SIZE
            ));
        };

        // Validation 2: Price must be in bounds
        if price_value == 0 || price_value >= MAX_PRICE {
            return Err(format!(
                "Price {} out of bounds [1, {})",
                price_value, MAX_PRICE
            ));
        }

        // Validation 3: Quantity must be multiple of lot size
        if quantity_value % LOT_SIZE as u32 != 0 {
            return Err(format!(
                "Quantity {} is not a valid lot (lot_size={})",
                quantity_value, LOT_SIZE
            ));
        };

        // Validation 4: Quantity must be positive
        if quantity_value == 0 {
            return Err("Quantity cannot be zero".to_string());
        };

        let i = price_value as usize;

        match side {
            Side::Bid => self.bids[i].add_order(order),
            Side::Ask => self.asks[i].add_order(order),
        }

        self.order_index.insert(order_id, (side, order.price()));

        Ok(())
    }
}

impl Level {
    pub fn add_order(&mut self, order: Order) {
        // O(1)
        self.orders.push(order);
    }

    pub fn cancel_order(&mut self, order_id: u64) -> Option<Order> {
        let position = self.orders.iter().position(|o| o.id() == order_id).unwrap();

        // O(n) - remove shifts elements after element is removed
        Some(self.orders.remove(position))
    }
}
