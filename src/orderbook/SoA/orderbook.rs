use crate::orderbook::{Fill, OrderbookTrait};
use crate::types::order::{Order, OrderId, Side};
use crate::types::price::Price;
use crate::types::quantity::Quantity;
use std::collections::HashMap;

/// Max price is represented in cents - $100 is max price
const MAX_PRICE: u32 = 10000;
const TICK_SIZE: u32 = 1;
const LOT_SIZE: u32 = 1;
const ELEMENT_NUM: usize = MAX_PRICE as usize / TICK_SIZE as usize;

// Structure-of-Arrays (SoA) Orderbook
// Same fixed-tick array structure, but each Level uses SoA instead of AoS
pub struct Orderbook {
    bids: Box<[LevelSoA; ELEMENT_NUM]>,
    asks: Box<[LevelSoA; ELEMENT_NUM]>,
    order_index: HashMap<OrderId, (Side, Price)>,
}

/// Level using Structure-of-Arrays (SoA) approach
/// Instead of Vec<Order> (AoS), we have separate arrays for each field
///
/// Memory layout comparison:
/// AoS: [id₁|side₁|price₁|qty₁|pad][id₂|side₂|price₂|qty₂|pad]... (24 bytes per order)
/// SoA: ids:[id₁|id₂|id₃...] sides:[s₁|s₂|s₃...] prices:[p₁|p₂|p₃...] quantities:[q₁|q₂|q₃...]
///
/// Cache line utilization (64 bytes):
/// - ids: 8 IDs per cache line (8 bytes each)
/// - sides: 64 sides per cache line (1 byte each)
/// - prices: 16 prices per cache line (4 bytes each)
/// - quantities: 16 quantities per cache line (4 bytes each)
///   vs AoS: only 2-3 complete orders per cache line
#[derive(Default, Clone)]
pub struct LevelSoA {
    /// Vec header: 24 bytes, then N × 8 bytes for IDs
    ids: Vec<u64>,
    /// Vec header: 24 bytes, then N × 1 byte for sides
    sides: Vec<Side>,
    /// Vec header: 24 bytes, then N × 4 bytes for prices
    prices: Vec<Price>,
    /// Vec header: 24 bytes, then N × 4 bytes for quantities
    quantities: Vec<Quantity>,
}

impl OrderbookTrait for Orderbook {
    fn new() -> Self {
        Self {
            bids: Box::new(std::array::from_fn(|_| LevelSoA::default())),
            asks: Box::new(std::array::from_fn(|_| LevelSoA::default())),
            order_index: HashMap::new(),
        }
    }

    fn add_order(&mut self, order: Order) -> Result<(), String> {
        let order_id = order.id();
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

        let i = (price_value / TICK_SIZE) as usize;

        match side {
            Side::Bid => self.bids[i].add_order(order),
            Side::Ask => self.asks[i].add_order(order),
        }

        self.order_index.insert(order_id, (side, order.price()));

        Ok(())
    }

    fn cancel_order(&mut self, order_id: OrderId) -> Result<(), String> {
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

    fn execute_market_order(
        &mut self,
        side: Side,
        mut quantity: Quantity,
    ) -> Result<Vec<Fill>, String> {
        let mut fills = Vec::new();

        match side {
            Side::Bid => {
                for i in 0..ELEMENT_NUM {
                    if quantity.value() == 0 {
                        break;
                    }
                    if self.asks[i].is_empty() {
                        continue;
                    }
                    let price = Price::define((i as u32) * TICK_SIZE);
                    let level_fills =
                        self.asks[i].match_orders(&mut quantity, price, &mut self.order_index);
                    fills.extend(level_fills);
                }
            }
            Side::Ask => {
                for i in (0..ELEMENT_NUM).rev() {
                    if quantity.value() == 0 {
                        break;
                    }
                    if self.bids[i].is_empty() {
                        continue;
                    }
                    let price = Price::define((i as u32) * TICK_SIZE);
                    let level_fills =
                        self.bids[i].match_orders(&mut quantity, price, &mut self.order_index);
                    fills.extend(level_fills);
                }
            }
        }

        if quantity.value() > 0 {
            return Err(format!(
                "Market order partially filled: {} remaining",
                quantity.value()
            ));
        }

        Ok(fills)
    }

    fn best_bid(&self) -> Option<Price> {
        for i in (0..ELEMENT_NUM).rev() {
            if !self.bids[i].is_empty() {
                return Some(Price::define((i as u32) * TICK_SIZE));
            }
        }
        None
    }

    fn best_ask(&self) -> Option<Price> {
        for i in 0..ELEMENT_NUM {
            if !self.asks[i].is_empty() {
                return Some(Price::define((i as u32) * TICK_SIZE));
            }
        }
        None
    }

    fn depth_at_price(&self, price: Price, side: Side) -> u32 {
        let price_value = price.value();

        if price_value == 0 || price_value >= MAX_PRICE {
            return 0;
        }

        if price_value % TICK_SIZE != 0 {
            return 0;
        }

        let index = (price_value / TICK_SIZE) as usize;

        match side {
            Side::Bid => self.bids[index].total_quantity(),
            Side::Ask => self.asks[index].total_quantity(),
        }
    }
}

impl LevelSoA {
    /// Add order to this level - appends to all arrays
    pub fn add_order(&mut self, order: Order) {
        self.ids.push(order.id());
        self.sides.push(order.side());
        self.prices.push(order.price());
        self.quantities.push(order.quantity());
    }

    /// Cancel order by ID - requires searching all IDs
    /// THIS IS WHERE SoA WINS: Only loads ID array (8 IDs per cache line)
    /// vs AoS: loads full Order structs (2-3 per cache line)
    pub fn cancel_order(&mut self, order_id: OrderId) -> Option<Order> {
        // Find position - only searches ID array (better cache utilization!)
        let pos = self.ids.iter().position(|&id| id == order_id)?;

        // Remove from all arrays
        let _id = self.ids.remove(pos);  
        let side = self.sides.remove(pos);
        let price = self.prices.remove(pos);
        let quantity = self.quantities.remove(pos);

        // Reconstruct Order for return
        Some(Order::new(
            price,
            quantity,
            side,
            &mut crate::types::order::IdCounter::new(),
        ))
    }

    /// Total quantity at this level
    /// THIS IS WHERE SoA WINS BIG: Only loads quantity array (16 per cache line)
    /// vs AoS: loads full Order structs (2-3 per cache line) = ~6x worse
    pub fn total_quantity(&self) -> u32 {
        self.quantities.iter().map(|q| q.value()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Match orders FIFO - consumes liquidity from this level
    /// THIS IS WHERE AoS WINS: Need all fields, so 4 separate array accesses
    /// vs AoS: 1 array access gets all fields
    pub fn match_orders(
        &mut self,
        remaining_qty: &mut Quantity,
        price: Price,
        order_index: &mut HashMap<OrderId, (Side, Price)>,
    ) -> Vec<Fill> {
        let mut fills = Vec::new();
        let mut orders_to_remove = Vec::new();

        for idx in 0..self.ids.len() {
            if remaining_qty.value() == 0 {
                break;
            }

            // SoA: Need to access 3 separate arrays (id, quantity, ...)
            let order_id = self.ids[idx];
            let order_qty = self.quantities[idx].value();
            let fill_qty = remaining_qty.value().min(order_qty);

            fills.push(Fill {
                price,
                quantity: Quantity::define(fill_qty),
                maker_order_id: order_id,
            });

            *remaining_qty = Quantity::define(remaining_qty.value() - fill_qty);

            if fill_qty == order_qty {
                orders_to_remove.push(idx);
            } else {
                panic!("Partial fills of resting orders not yet implemented");
            }
        }

        // Remove filled orders from all arrays
        for &idx in orders_to_remove.iter().rev() {
            let removed_id = self.ids.remove(idx);
            self.sides.remove(idx);
            self.prices.remove(idx);
            self.quantities.remove(idx);
            order_index.remove(&removed_id);
        }

        fills
    }
}
