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

// Empty Orderbook:
// -Bids and Asks: 10,000 * 2 * 24(VH)  =  480,000 bytes or 480 KB
// -Order Index: 48 bytes(HMH)
pub struct Orderbook {
    bids: Box<[Level; ELEMENT_NUM]>,
    asks: Box<[Level; ELEMENT_NUM]>,
    // entry: OrderId: 8b + Value(S+P): 5b (padded to 8b) = 16b
    // HashMap overhead per entry: 24-32 bytes
    // all together: 40 -48 bytes per entry
    order_index: HashMap<OrderId, (Side, Price)>,
}

/// Level Memory: H(24) + N * 24
#[derive(Default, Clone)]
pub struct Level {
    /// Vec of 24 bytes per element
    /// Vec header (ptr: *mut Order: 8bytes, len: usize(8bytes), cap: usize(8bytes))
    /// usize on 64-bit system is 8 bytes because its addresses are pointer sized
    pub orders: Vec<Order>,
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

        let i = (price_value / TICK_SIZE) as usize;

        match side {
            // O(1) array access: CPU calculates base_address + (i × 24 bytes) in hardware
            Side::Bid => self.bids[i].add_order(order),
            Side::Ask => self.asks[i].add_order(order),
        }

        self.order_index.insert(order_id, (side, order.price()));

        Ok(())
    }

    pub fn cancel_order(&mut self, order_id: OrderId) -> Result<(), String> {
        let (side, price) = self
            .order_index
            .remove(&order_id)
            .ok_or_else(|| format!("Order {} not found", order_id))?;

        let i = (price.value() / TICK_SIZE) as usize;

        match side {
            Side::Bid => self.bids[i].cancel_order(order_id),
            Side::Ask => self.asks[i].cancel_order(order_id),
        };

        Ok(())
    }

    // Best bid and Best ask are O(n) in worst case -> VERY BAD
    // That is a tradeoff for adding and canceling order being O(1)

    pub fn best_bid(&self) -> Option<Price> {
        // O(n)

        // Scan from highest price (end of array) downward
        for i in (0..ELEMENT_NUM).rev() {
            if !self.bids[i].is_empty() {
                // Convert index back to price: i * TICK_SIZE
                return Some(Price::define((i as u32) * TICK_SIZE));
            }
        }
        None
    }

    pub fn best_ask(&self) -> Option<Price> {
        // Scan from lowest price (start of array) upward
        for i in 0..ELEMENT_NUM {
            if !self.asks[i].is_empty() {
                // Convert index back to price: i * TICK_SIZE
                return Some(Price::define((i as u32) * TICK_SIZE));
            }
        }
        None
    }
}

impl Level {
    pub fn add_order(&mut self, order: Order) {
        // O(1)
        self.orders.push(order);
    }

    pub fn cancel_order(&mut self, order_id: u64) -> Option<Order> {
        let i = self.orders.iter().position(|o| o.id() == order_id)?;

        // O(n) - remove shifts elements after element is removed
        Some(self.orders.remove(i))
    }

    pub fn total_quantity(&self) -> u64 {
        let q = self
            .orders
            .iter()
            .map(|o| o.quantity().value() as u64)
            .sum::<u64>();

        q
    }

    pub fn is_empty(&self) -> bool {
        if self.orders.len() == 0 {
            return true;
        }

        false
    }

    pub fn first_order(&self) -> Option<&Order> {
        self.orders.first()
    }
}
