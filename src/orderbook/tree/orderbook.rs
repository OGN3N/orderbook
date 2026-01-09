use crate::orderbook::{Fill, OrderbookTrait};
use crate::types::order::{Order, OrderId, Side};
use crate::types::price::Price;
use crate::types::quantity::Quantity;
use std::collections::{BTreeMap, HashMap};

/// Max price is represented in cents - $100 is max price
const MAX_PRICE: u32 = 10000;
const TICK_SIZE: u32 = 1;
const LOT_SIZE: u32 = 1;
const ELEMENT_NUM: usize = MAX_PRICE as usize / TICK_SIZE as usize;
pub struct Orderbook {
    bids: BTreeMap<u32, Level>,
    asks: BTreeMap<u32, Level>,
}
#[derive(Default, Clone)]
pub struct Level {
    /// Vec of 24 bytes per element
    /// Vec header (ptr: *mut Order: 8bytes, len: usize(8bytes), cap: usize(8bytes))
    /// usize on 64-bit system is 8 bytes because its addresses are pointer sized
    pub orders: Vec<Order>,
}
impl OrderbookTrait for Orderbook {
    fn new() -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
        }
    }

    fn add_order(&mut self, order: Order) -> Result<(), String> {
        let side = order.side();
        let price_value = order.price().value();
        let quantity_value = order.quantity().value();

        // Validation 1: Price must be multiple of tick size
        if price_value % TICK_SIZE != 0 {
            return Err(format!(
                "Price {} is not a valid tick (tick_size={})",
                price_value, TICK_SIZE
            ));
        }

        // Validation 2: Price must be in bounds
        if price_value == 0 || price_value >= MAX_PRICE {
            return Err(format!(
                "Price {} out of bounds [1, {})",
                price_value, MAX_PRICE
            ));
        }

        // Validation 3: Quantity must be multiple of lot size
        if quantity_value % LOT_SIZE != 0 {
            return Err(format!(
                "Quantity {} is not a valid lot (lot_size={})",
                quantity_value, LOT_SIZE
            ));
        }

        // Validation 4: Quantity must be positive
        if quantity_value == 0 {
            return Err("Quantity cannot be zero".to_string());
        }

        // Add order to appropriate side
        // Use entry API to insert or modify in place
        match side {
            Side::Bid => {
                self.bids
                    .entry(price_value)
                    .or_insert_with(Level::default)
                    .orders
                    .push(order);
            }
            Side::Ask => {
                self.asks
                    .entry(price_value)
                    .or_insert_with(Level::default)
                    .orders
                    .push(order);
            }
        }

        Ok(())
    }
}
