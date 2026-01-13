use crate::orderbook::{Fill, OrderbookTrait};
use crate::types::order::{Order, OrderId, Side};
use crate::types::price::Price;
use crate::types::quantity::Quantity;
use std::collections::{BTreeMap, HashMap};

/// Max price is represented in cents - $100 is max price
const MAX_PRICE: u32 = 10000;
const TICK_SIZE: u32 = 1;
const LOT_SIZE: u32 = 1;
pub struct Orderbook {
    bids: BTreeMap<u32, Level>,
    asks: BTreeMap<u32, Level>,
    order_index: HashMap<OrderId, (Side, Price)>,
}
#[derive(Default, Clone)]
pub struct Level {
    pub orders: Vec<Order>,
}

impl OrderbookTrait for Orderbook {
    fn new() -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
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

        // Track order in index for O(1) lookup during cancellation
        self.order_index.insert(order.id(), (side, order.price()));

        Ok(())
    }

    fn cancel_order(&mut self, order_id: OrderId) -> Result<(), String> {
        // O(1) lookup in HashMap to find price level
        let (side, price) = self
            .order_index
            .remove(&order_id)
            .ok_or_else(|| format!("Order {} not found", order_id))?;

        let price_value = price.value();

        // O(log n) lookup in BTreeMap to get the level
        let tree = match side {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
        };

        if let Some(level) = tree.get_mut(&price_value) {
            // O(n) search within the level to find and remove the order
            if let Some(pos) = level.orders.iter().position(|o| o.id() == order_id) {
                level.orders.remove(pos);

                // Clean up empty price levels to keep tree sparse
                if level.orders.is_empty() {
                    tree.remove(&price_value);
                }

                return Ok(());
            }
        }

        // Order was in index but not in tree (data inconsistency)
        Err(format!(
            "Order {} found in index but not in tree (data inconsistency)",
            order_id
        ))
    }

    fn execute_market_order(
        &mut self,
        side: Side,
        mut quantity: Quantity,
    ) -> Result<Vec<Fill>, String> {
        let mut fills = Vec::new();
        let mut empty_levels = Vec::new();

        match side {
            // Market BUY: consume asks (lowest price first)
            Side::Bid => {
                // BTreeMap iter() returns keys in ascending order (lowest to highest)
                for (&price_value, level) in self.asks.iter_mut() {
                    if quantity.value() == 0 {
                        break;
                    }

                    let price = Price::define(price_value);
                    let level_fills =
                        Self::match_level(level, &mut quantity, price, &mut self.order_index);
                    fills.extend(level_fills);

                    // Track empty levels for cleanup
                    if level.orders.is_empty() {
                        empty_levels.push(price_value);
                    }
                }

                // Clean up empty price levels
                for price_value in empty_levels {
                    self.asks.remove(&price_value);
                }
            }

            // Market SELL: consume bids (highest price first)
            Side::Ask => {
                // BTreeMap iter().rev() returns keys in descending order (highest to lowest)
                for (&price_value, level) in self.bids.iter_mut().rev() {
                    if quantity.value() == 0 {
                        break;
                    }

                    let price = Price::define(price_value);
                    let level_fills =
                        Self::match_level(level, &mut quantity, price, &mut self.order_index);
                    fills.extend(level_fills);

                    // Track empty levels for cleanup
                    if level.orders.is_empty() {
                        empty_levels.push(price_value);
                    }
                }

                // Clean up empty price levels
                for price_value in empty_levels {
                    self.bids.remove(&price_value);
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
        // BTreeMap's last_key_value() returns highest key in O(log n)
        self.bids
            .last_key_value()
            .map(|(&price_value, _)| Price::define(price_value))
    }

    fn best_ask(&self) -> Option<Price> {
        // BTreeMap's first_key_value() returns lowest key in O(log n)
        self.asks
            .first_key_value()
            .map(|(&price_value, _)| Price::define(price_value))
    }

    fn depth_at_price(&self, price: Price, side: Side) -> u32 {
        let price_value = price.value();

        // Check bounds
        if price_value == 0 || price_value >= MAX_PRICE {
            return 0;
        }

        // Check tick alignment
        if price_value % TICK_SIZE != 0 {
            return 0;
        }

        // O(log n) lookup in BTreeMap
        let tree = match side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
        };

        tree.get(&price_value)
            .map(|level| level.total_quantity())
            .unwrap_or(0)
    }
}

impl Orderbook {
    /// Match orders at a single price level (FIFO)
    /// Modifies remaining_qty as orders are filled
    /// Removes filled orders from the level and order_index
    /// Returns vector of fills that occurred
    fn match_level(
        level: &mut Level,
        remaining_qty: &mut Quantity,
        price: Price,
        order_index: &mut HashMap<OrderId, (Side, Price)>,
    ) -> Vec<Fill> {
        let mut fills = Vec::new();
        let mut orders_to_remove = Vec::new();

        // Process orders in FIFO order (first in Vec = earliest order)
        for (idx, order) in level.orders.iter().enumerate() {
            if remaining_qty.value() == 0 {
                break; // Market order fully filled
            }

            let order_qty = order.quantity().value();
            let fill_qty = remaining_qty.value().min(order_qty);

            // Create fill
            fills.push(Fill {
                price,
                quantity: Quantity::define(fill_qty),
                maker_order_id: order.id(),
            });

            // Update remaining quantity
            *remaining_qty = Quantity::define(remaining_qty.value() - fill_qty);

            // If order fully filled, mark for removal
            if fill_qty == order_qty {
                orders_to_remove.push(idx);
            } else {
                // Partial fill of resting order - not implemented yet
                panic!("Partial fills of resting orders not yet implemented");
            }
        }

        // Remove filled orders in reverse order (to maintain indices)
        for &idx in orders_to_remove.iter().rev() {
            let removed_order = level.orders.remove(idx);
            order_index.remove(&removed_order.id());
        }

        fills
    }
}

impl Level {
    /// Calculate total quantity at this price level
    pub fn total_quantity(&self) -> u32 {
        self.orders
            .iter()
            .map(|o| o.quantity().value())
            .sum::<u32>()
    }
}
