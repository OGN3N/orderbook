use crate::types::order::{Order, OrderId, Side};
use crate::types::price::Price;
use crate::types::quantity::Quantity;

/// Represents a trade execution (fill)
#[derive(Debug, Clone)]
pub struct Fill {
    pub price: Price,
    pub quantity: Quantity,
    pub maker_order_id: OrderId,
}

/// Common trait that all orderbook implementations must implement
/// This allows benchmarking different implementations uniformly
pub trait OrderbookTrait {
    /// Create a new empty orderbook
    fn new() -> Self;

    /// Add a limit order to the book
    /// Returns error if order is invalid (bad price/quantity, out of bounds, etc.)
    fn add_order(&mut self, order: Order) -> Result<(), String>;

    /// Cancel an order by ID
    /// Returns error if order not found
    fn cancel_order(&mut self, order_id: OrderId) -> Result<(), String>;

    /// Execute a market order, consuming liquidity from the book
    /// Returns fills that occurred, or error if insufficient liquidity
    fn execute_market_order(&mut self, side: Side, quantity: Quantity)
    -> Result<Vec<Fill>, String>;

    /// Get the best (highest) bid price
    fn best_bid(&self) -> Option<Price>;

    /// Get the best (lowest) ask price
    fn best_ask(&self) -> Option<Price>;

    /// Get total quantity available at a specific price level
    fn depth_at_price(&self, price: Price, side: Side) -> u32;

    /// Get the mid price (average of best bid and best ask)
    fn mid_price(&self) -> Option<Price> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(Price::define((bid.value() + ask.value()) / 2)),
            _ => None,
        }
    }
}

#[allow(non_snake_case)]
pub mod SoA;
pub mod fixed_tick;
pub mod hybrid;
pub mod tree;
