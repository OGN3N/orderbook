use crate::orderbook::{Fill, OrderbookTrait};
use crate::types::order::{Order, OrderId, Side};
use crate::types::price::Price;
use crate::types::quantity::Quantity;
use std::collections::{BTreeMap, HashMap};

/// Hybrid orderbook: Hot zone uses fixed array, cold zone uses tree
///
/// Design:
/// - Hot zone: Fixed array centered around mid-price (fast O(1) access)
/// - Cold zone: BTreeMap for sparse far-from-market prices (dynamic)
/// - Adaptive: Can shift hot zone as market moves

const MAX_PRICE: u32 = 10000;
const TICK_SIZE: u32 = 1;
const LOT_SIZE: u32 = 1;

/// Size of the hot zone array (e.g., 200 price levels = $2 range with 1 cent ticks)
/// This covers typical intraday price movement
const HOT_ZONE_SIZE: usize = 200;

/// Hot zone extends this many ticks above and below mid price
const HOT_ZONE_RADIUS: u32 = (HOT_ZONE_SIZE / 2) as u32;

pub struct Orderbook {
    // Hot zone: Fixed array for frequently-accessed prices near the spread
    hot_bids: Box<[Level; HOT_ZONE_SIZE]>,
    hot_asks: Box<[Level; HOT_ZONE_SIZE]>,

    // Cold zone: Tree for rarely-accessed prices far from spread
    cold_bids: BTreeMap<u32, Level>,
    cold_asks: BTreeMap<u32, Level>,

    // Center of hot zone (in price value, not index)
    hot_zone_center: u32,

    // Order index for O(1) cancel lookups
    order_index: HashMap<OrderId, (Side, Price)>,
}

#[derive(Default, Clone)]
pub struct Level {
    pub orders: Vec<Order>,
}

impl OrderbookTrait for Orderbook {
    fn new() -> Self {
        Self {
            hot_bids: Box::new(std::array::from_fn(|_| Level::default())),
            hot_asks: Box::new(std::array::from_fn(|_| Level::default())),
            cold_bids: BTreeMap::new(),
            cold_asks: BTreeMap::new(),
            hot_zone_center: MAX_PRICE / 2, // Start at mid-range
            order_index: HashMap::new(),
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

        // Determine if price is in hot or cold zone
        if self.is_in_hot_zone(price_value) {
            // Hot zone: O(1) array access
            let idx = self.hot_zone_index(price_value);
            match side {
                Side::Bid => self.hot_bids[idx].orders.push(order),
                Side::Ask => self.hot_asks[idx].orders.push(order),
            }
        } else {
            // Cold zone: O(log n) tree access
            match side {
                Side::Bid => {
                    self.cold_bids
                        .entry(price_value)
                        .or_insert_with(Level::default)
                        .orders
                        .push(order);
                }
                Side::Ask => {
                    self.cold_asks
                        .entry(price_value)
                        .or_insert_with(Level::default)
                        .orders
                        .push(order);
                }
            }
        }

        self.order_index.insert(order.id(), (side, order.price()));
        Ok(())
    }

    fn cancel_order(&mut self, order_id: OrderId) -> Result<(), String> {
        let (side, price) = self
            .order_index
            .remove(&order_id)
            .ok_or_else(|| format!("Order {} not found", order_id))?;

        let price_value = price.value();

        // Check hot zone first (most likely)
        if self.is_in_hot_zone(price_value) {
            let idx = self.hot_zone_index(price_value);
            let level = match side {
                Side::Bid => &mut self.hot_bids[idx],
                Side::Ask => &mut self.hot_asks[idx],
            };

            if let Some(pos) = level.orders.iter().position(|o| o.id() == order_id) {
                level.orders.remove(pos);
                return Ok(());
            }
        } else {
            // Cold zone: tree lookup
            let tree = match side {
                Side::Bid => &mut self.cold_bids,
                Side::Ask => &mut self.cold_asks,
            };

            if let Some(level) = tree.get_mut(&price_value) {
                if let Some(pos) = level.orders.iter().position(|o| o.id() == order_id) {
                    level.orders.remove(pos);

                    // Clean up empty levels in cold zone
                    if level.orders.is_empty() {
                        tree.remove(&price_value);
                    }

                    return Ok(());
                }
            }
        }

        Err(format!(
            "Order {} found in index but not in book (data inconsistency)",
            order_id
        ))
    }

    fn execute_market_order(
        &mut self,
        side: Side,
        mut quantity: Quantity,
    ) -> Result<Vec<Fill>, String> {
        let mut fills = Vec::new();

        match side {
            // Market BUY: consume asks (lowest price first)
            Side::Bid => {
                // First, consume from hot zone
                for i in 0..HOT_ZONE_SIZE {
                    if quantity.value() == 0 {
                        break;
                    }
                    if self.hot_asks[i].orders.is_empty() {
                        continue;
                    }

                    let price_value = self.hot_zone_center - HOT_ZONE_RADIUS as u32 + i as u32;
                    let price = Price::define(price_value);
                    let level_fills = Self::match_level(
                        &mut self.hot_asks[i],
                        &mut quantity,
                        price,
                        &mut self.order_index,
                    );
                    fills.extend(level_fills);
                }

                // Then, consume from cold zone if needed
                if quantity.value() > 0 {
                    let mut empty_levels = Vec::new();
                    for (&price_value, level) in self.cold_asks.iter_mut() {
                        if quantity.value() == 0 {
                            break;
                        }

                        let price = Price::define(price_value);
                        let level_fills =
                            Self::match_level(level, &mut quantity, price, &mut self.order_index);
                        fills.extend(level_fills);

                        if level.orders.is_empty() {
                            empty_levels.push(price_value);
                        }
                    }

                    // Clean up empty cold levels
                    for price_value in empty_levels {
                        self.cold_asks.remove(&price_value);
                    }
                }
            }

            // Market SELL: consume bids (highest price first)
            Side::Ask => {
                // First, consume from hot zone (highest first)
                for i in (0..HOT_ZONE_SIZE).rev() {
                    if quantity.value() == 0 {
                        break;
                    }
                    if self.hot_bids[i].orders.is_empty() {
                        continue;
                    }

                    let price_value = self.hot_zone_center - HOT_ZONE_RADIUS as u32 + i as u32;
                    let price = Price::define(price_value);
                    let level_fills = Self::match_level(
                        &mut self.hot_bids[i],
                        &mut quantity,
                        price,
                        &mut self.order_index,
                    );
                    fills.extend(level_fills);
                }

                // Then, consume from cold zone if needed
                if quantity.value() > 0 {
                    let mut empty_levels = Vec::new();
                    for (&price_value, level) in self.cold_bids.iter_mut().rev() {
                        if quantity.value() == 0 {
                            break;
                        }

                        let price = Price::define(price_value);
                        let level_fills =
                            Self::match_level(level, &mut quantity, price, &mut self.order_index);
                        fills.extend(level_fills);

                        if level.orders.is_empty() {
                            empty_levels.push(price_value);
                        }
                    }

                    // Clean up empty cold levels
                    for price_value in empty_levels {
                        self.cold_bids.remove(&price_value);
                    }
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
        // Search hot zone first (highest to lowest)
        for i in (0..HOT_ZONE_SIZE).rev() {
            if !self.hot_bids[i].orders.is_empty() {
                let price_value = self.hot_zone_center - HOT_ZONE_RADIUS as u32 + i as u32;
                return Some(Price::define(price_value));
            }
        }

        // If not in hot zone, check cold zone
        self.cold_bids
            .last_key_value()
            .map(|(&price_value, _)| Price::define(price_value))
    }

    fn best_ask(&self) -> Option<Price> {
        // Search hot zone first (lowest to highest)
        for i in 0..HOT_ZONE_SIZE {
            if !self.hot_asks[i].orders.is_empty() {
                let price_value = self.hot_zone_center - HOT_ZONE_RADIUS as u32 + i as u32;
                return Some(Price::define(price_value));
            }
        }

        // If not in hot zone, check cold zone
        self.cold_asks
            .first_key_value()
            .map(|(&price_value, _)| Price::define(price_value))
    }

    fn depth_at_price(&self, price: Price, side: Side) -> u32 {
        let price_value = price.value();

        if price_value == 0 || price_value >= MAX_PRICE {
            return 0;
        }

        if price_value % TICK_SIZE != 0 {
            return 0;
        }

        if self.is_in_hot_zone(price_value) {
            // Hot zone: O(1) lookup
            let idx = self.hot_zone_index(price_value);
            let level = match side {
                Side::Bid => &self.hot_bids[idx],
                Side::Ask => &self.hot_asks[idx],
            };
            level.total_quantity()
        } else {
            // Cold zone: O(log n) lookup
            let tree = match side {
                Side::Bid => &self.cold_bids,
                Side::Ask => &self.cold_asks,
            };
            tree.get(&price_value)
                .map(|level| level.total_quantity())
                .unwrap_or(0)
        }
    }
}

impl Orderbook {
    /// Check if a price is within the hot zone
    fn is_in_hot_zone(&self, price_value: u32) -> bool {
        let lower_bound = self.hot_zone_center.saturating_sub(HOT_ZONE_RADIUS);
        let upper_bound = self.hot_zone_center + HOT_ZONE_RADIUS;
        price_value >= lower_bound && price_value < upper_bound
    }

    /// Convert price to hot zone array index
    fn hot_zone_index(&self, price_value: u32) -> usize {
        let offset = price_value - (self.hot_zone_center - HOT_ZONE_RADIUS);
        offset as usize
    }

    /// Match orders at a single price level (FIFO)
    fn match_level(
        level: &mut Level,
        remaining_qty: &mut Quantity,
        price: Price,
        order_index: &mut HashMap<OrderId, (Side, Price)>,
    ) -> Vec<Fill> {
        let mut fills = Vec::new();
        let mut orders_to_remove = Vec::new();

        for (idx, order) in level.orders.iter().enumerate() {
            if remaining_qty.value() == 0 {
                break;
            }

            let order_qty = order.quantity().value();
            let fill_qty = remaining_qty.value().min(order_qty);

            fills.push(Fill {
                price,
                quantity: Quantity::define(fill_qty),
                maker_order_id: order.id(),
            });

            *remaining_qty = Quantity::define(remaining_qty.value() - fill_qty);

            if fill_qty == order_qty {
                orders_to_remove.push(idx);
            } else {
                panic!("Partial fills of resting orders not yet implemented");
            }
        }

        for &idx in orders_to_remove.iter().rev() {
            let removed_order = level.orders.remove(idx);
            order_index.remove(&removed_order.id());
        }

        fills
    }
}

impl Level {
    pub fn total_quantity(&self) -> u32 {
        self.orders
            .iter()
            .map(|o| o.quantity().value())
            .sum::<u32>()
    }
}
