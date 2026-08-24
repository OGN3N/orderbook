//! Scenario 4.2d: Stable-Depth Mixed Operations
//!
//! Measures add, successful cancel, and successful market-order latency while
//! keeping the resting book at exactly 1,000 orders at every round boundary.
//!
//! Run with: cargo run --release --example scenario_steady_state

use orderbook::analysis::{CsvExporter, ResultRow};
use orderbook::orderbook::fixed_tick::orderbook::Orderbook as FixedTickOrderbook;
use orderbook::orderbook::hybrid::orderbook::Orderbook as HybridOrderbook;
use orderbook::orderbook::tree::orderbook::Orderbook as TreeOrderbook;
use orderbook::orderbook::OrderbookTrait;
use orderbook::orderbook::SoA::orderbook::Orderbook as SoAOrderbook;
use orderbook::perf::latency::{LatencyTracker, Percentiles};
use orderbook::perf::{cycles_to_ns, get_cpu_frequency};
use orderbook::types::order::{IdCounter, Order, OrderId, Side};
use orderbook::types::price::Price;
use orderbook::types::quantity::Quantity;
use rand::prelude::*;
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::collections::{HashMap, VecDeque};

const MIN_PRICE: u32 = 1;
const MID_PRICE: u32 = 5_000;
const MAX_PRICE_EXCLUSIVE: u32 = 10_000;
const PRICE_LEVELS_PER_SIDE: u32 = 50;
const PRICE_LEVEL_COUNT: usize = PRICE_LEVELS_PER_SIDE as usize;
const BID_MIN_PRICE: u32 = MID_PRICE - PRICE_LEVELS_PER_SIDE;
const ASK_MAX_PRICE_EXCLUSIVE: u32 = MID_PRICE + PRICE_LEVELS_PER_SIDE;

const ORDER_QUANTITY: u32 = 100;
const MARKET_QUANTITY: u32 = 300;
const FILLS_PER_MARKET: usize = (MARKET_QUANTITY / ORDER_QUANTITY) as usize;

const INITIAL_ORDERS_PER_SIDE: usize = 500;
const INITIAL_ORDERS: usize = INITIAL_ORDERS_PER_SIDE * 2;

// One exact stationary round. Adds and removals balance both globally and on
// each side: 6 adds - 3 cancels - 3 market fills = 0 orders per side.
const ADDS_PER_SIDE_PER_ROUND: usize = 6;
const CANCELS_PER_SIDE_PER_ROUND: usize = 3;
const MARKET_ORDERS_PER_SIDE_PER_ROUND: usize = 1;
const ADDS_PER_ROUND: usize = ADDS_PER_SIDE_PER_ROUND * 2;
const CANCELS_PER_ROUND: usize = CANCELS_PER_SIDE_PER_ROUND * 2;
const MARKET_ORDERS_PER_ROUND: usize = MARKET_ORDERS_PER_SIDE_PER_ROUND * 2;
const OPERATIONS_PER_ROUND: usize = ADDS_PER_ROUND + CANCELS_PER_ROUND + MARKET_ORDERS_PER_ROUND;

const DEFAULT_MARKET_SAMPLES: usize = 1_000_000;
const MARKET_SAMPLES_ENV: &str = "ORDERBOOK_STEADY_STATE_MARKET_SAMPLES";
const MEASURED_ROUNDS_PER_BOOK: usize = 5_000;
const WARMUP_ROUNDS_PER_BOOK: usize = 100;
const BASE_SEED: u64 = 42;

#[derive(Clone, Copy)]
enum Operation {
    Add(Side),
    Cancel(Side),
    Market(Side),
}

const ROUND_OPERATIONS: [Operation; OPERATIONS_PER_ROUND] = [
    Operation::Add(Side::Bid),
    Operation::Add(Side::Bid),
    Operation::Add(Side::Bid),
    Operation::Add(Side::Bid),
    Operation::Add(Side::Bid),
    Operation::Add(Side::Bid),
    Operation::Add(Side::Ask),
    Operation::Add(Side::Ask),
    Operation::Add(Side::Ask),
    Operation::Add(Side::Ask),
    Operation::Add(Side::Ask),
    Operation::Add(Side::Ask),
    Operation::Cancel(Side::Bid),
    Operation::Cancel(Side::Bid),
    Operation::Cancel(Side::Bid),
    Operation::Cancel(Side::Ask),
    Operation::Cancel(Side::Ask),
    Operation::Cancel(Side::Ask),
    Operation::Market(Side::Bid),
    Operation::Market(Side::Ask),
];

// ============================================================================
// Scenario 4.2d: Stable-Depth Mixed Operations
// ============================================================================
//
// PURPOSE:
// Measure all three order-book operations in one continuously changing but
// stationary synthetic workload.
//
// EXACT 20-OPERATION ROUND:
// - 12 adds: six bids + six asks, each for quantity 100.
// - Six successful cancels: three bids + three asks.
// - Two successful market orders: one buy + one sell, each for quantity 300.
//
// Each market order consumes exactly three 100-quantity makers. Consequently,
// every round adds 12 orders and removes six by cancellation plus six by trade.
// It begins and ends with exactly 500 bids and 500 asks.
//
// PRICE MODEL:
// - Bids are uniform in [4950, 5000).
// - Asks are uniform in [5000, 5050).
// The resting book therefore cannot cross. All prices are within Hybrid's
// [4900, 5100) hot array, so this scenario intentionally measures its hot path.
//
// BATCHING AND WARM-UP:
// The default run aggregates 100 fresh logical books. Each is prefilled to
// 1,000 orders and receives 100 untimed mixed-operation warm-up rounds before
// its measured rounds. Fresh batches bound allocator/history correlation while
// the mixed warm-up exercises add, cancel, and market paths.
//
// TIMING MODEL:
// Only the order-book API call is timed. RNG, Order construction, operation
// scheduling, live-ID bookkeeping, Result checking, fill validation, and book
// validation are outside the timed region. Vec allocation performed by a
// market-order API call is timed; destruction of its returned fills is not.
//
// This is a chosen single-threaded synthetic mix, not a claim about a universal
// exchange workload. It does not model networking, concurrency, or arrival
// timing. Cycle counts are the primary measurements.
// ============================================================================

fn main() {
    println!("=== Scenario 4.2d: Stable-Depth Mixed Operations ===\n");

    let cpu_ghz = get_cpu_frequency();
    println!("CPU frequency: {:.3} GHz", cpu_ghz);
    print_cpu_model();

    let config = BenchmarkConfig::from_environment();
    print_parameters(&config);

    println!("--- Fixed-Tick Array ---");
    let fixed = run_steady_state::<FixedTickOrderbook>(BASE_SEED, &config);
    print_results(&fixed, cpu_ghz);

    println!("\n--- Structure-of-Arrays (SoA) ---");
    let soa = run_steady_state::<SoAOrderbook>(BASE_SEED, &config);
    print_results(&soa, cpu_ghz);

    println!("\n--- Hybrid (Hot/Cold) ---");
    let hybrid = run_steady_state::<HybridOrderbook>(BASE_SEED, &config);
    print_results(&hybrid, cpu_ghz);

    println!("\n--- Tree-Based ---");
    let tree = run_steady_state::<TreeOrderbook>(BASE_SEED, &config);
    print_results(&tree, cpu_ghz);

    println!("\n--- Comparison (p50 latency in cycles) ---");
    print_comparison(&fixed, &soa, &hybrid, &tree);

    println!("\n--- Tail Latency (p99/p50 ratio) ---");
    print_tail_analysis(&fixed, &soa, &hybrid, &tree);

    println!("\n--- Workload-Weighted Mean Latency ---");
    print_weighted_means(&fixed, &soa, &hybrid, &tree, cpu_ghz);

    export_csv(&config, cpu_ghz, &fixed, &soa, &hybrid, &tree);
}

#[derive(Clone, Copy)]
struct BenchmarkConfig {
    market_samples: usize,
    measured_rounds: usize,
    book_batches: usize,
}

impl BenchmarkConfig {
    fn from_environment() -> Self {
        let market_samples = match std::env::var(MARKET_SAMPLES_ENV) {
            Ok(value) => value
                .parse::<usize>()
                .ok()
                .filter(|samples| *samples >= MARKET_ORDERS_PER_ROUND)
                .filter(|samples| samples % MARKET_ORDERS_PER_ROUND == 0)
                .unwrap_or_else(|| {
                    panic!(
                        "{MARKET_SAMPLES_ENV} must be a positive even integer of at least 2, got {value:?}"
                    )
                }),
            Err(std::env::VarError::NotPresent) => DEFAULT_MARKET_SAMPLES,
            Err(error) => panic!("could not read {MARKET_SAMPLES_ENV}: {error}"),
        };

        let measured_rounds = market_samples / MARKET_ORDERS_PER_ROUND;
        let book_batches = measured_rounds.div_ceil(MEASURED_ROUNDS_PER_BOOK);

        Self {
            market_samples,
            measured_rounds,
            book_batches,
        }
    }

    fn adds(self) -> usize {
        checked_measurement_count(self.measured_rounds, ADDS_PER_ROUND, "add")
    }

    fn cancels(self) -> usize {
        checked_measurement_count(self.measured_rounds, CANCELS_PER_ROUND, "cancel")
    }

    fn markets(self) -> usize {
        self.market_samples
    }

    fn total_operations(self) -> usize {
        checked_measurement_count(
            self.measured_rounds,
            OPERATIONS_PER_ROUND,
            "total operation",
        )
    }

    fn rounds_in_batch(self, batch_index: usize) -> usize {
        let completed_rounds = batch_index
            .checked_mul(MEASURED_ROUNDS_PER_BOOK)
            .expect("steady-state batch index overflow");
        self.measured_rounds
            .saturating_sub(completed_rounds)
            .min(MEASURED_ROUNDS_PER_BOOK)
    }
}

fn checked_measurement_count(rounds: usize, operations_per_round: usize, label: &str) -> usize {
    rounds
        .checked_mul(operations_per_round)
        .unwrap_or_else(|| panic!("steady-state {label} measurement count overflows usize"))
}

fn print_cpu_model() {
    #[cfg(target_os = "linux")]
    {
        if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in cpuinfo.lines() {
                if line.starts_with("model name") {
                    if let Some(model) = line.split(':').nth(1) {
                        println!("CPU model: {}", model.trim());
                        break;
                    }
                }
            }
        }
    }
}

fn print_parameters(config: &BenchmarkConfig) {
    println!("\nStable-Depth Parameters:");
    println!(
        "  Round-boundary book depth: {} orders ({} bids + {} asks)",
        INITIAL_ORDERS, INITIAL_ORDERS_PER_SIDE, INITIAL_ORDERS_PER_SIDE
    );
    println!("  Bid prices: [{}, {})", BID_MIN_PRICE, MID_PRICE);
    println!("  Ask prices: [{}, {})", MID_PRICE, ASK_MAX_PRICE_EXCLUSIVE);
    println!("  Resting order quantity: {}", ORDER_QUANTITY);
    println!(
        "  Market order quantity: {} (exactly {} makers)",
        MARKET_QUANTITY, FILLS_PER_MARKET
    );
    println!(
        "  Exact round: {} adds + {} cancels + {} market orders = {} operations",
        ADDS_PER_ROUND, CANCELS_PER_ROUND, MARKET_ORDERS_PER_ROUND, OPERATIONS_PER_ROUND
    );
    println!("  Operation mix: 60% add / 30% cancel / 10% market");
    println!("  Measured rounds: {}", config.measured_rounds);
    println!(
        "  Book batches: {} (up to {} measured + {} untimed warm-up rounds each)",
        config.book_batches, MEASURED_ROUNDS_PER_BOOK, WARMUP_ROUNDS_PER_BOOK
    );
    println!("  Measured operations: {}", config.total_operations());
    println!(
        "  Measurements: {} adds, {} successful cancels, {} successful market orders",
        config.adds(),
        config.cancels(),
        config.markets()
    );
    println!("  Hybrid path: 100% hot-zone prices");
    println!("  Timing: order-book API call only; validation is untimed");
    if config.market_samples < DEFAULT_MARKET_SAMPLES {
        println!(
            "  WARNING: reduced-sample smoke run; extreme tail percentiles are not publication-quality"
        );
    }
    println!();
}

struct SteadyStateResults {
    add_order: Percentiles,
    cancel_order: Percentiles,
    market_order: Percentiles,
}

struct ScenarioTrackers {
    add_order: LatencyTracker,
    cancel_order: LatencyTracker,
    market_order: LatencyTracker,
}

impl ScenarioTrackers {
    fn new(config: &BenchmarkConfig) -> Self {
        Self {
            add_order: LatencyTracker::new(config.adds()),
            cancel_order: LatencyTracker::new(config.cancels()),
            market_order: LatencyTracker::new(config.markets()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ActiveOrder {
    id: OrderId,
    price: u32,
}

struct ActiveSide {
    orders: Vec<ActiveOrder>,
    positions: HashMap<OrderId, usize>,
}

impl ActiveSide {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            orders: Vec::with_capacity(capacity),
            positions: HashMap::with_capacity(capacity),
        }
    }

    fn insert(&mut self, order: ActiveOrder) {
        let index = self.orders.len();
        assert!(
            self.positions.insert(order.id, index).is_none(),
            "active order ID inserted twice"
        );
        self.orders.push(order);
    }

    fn random_id(&self, rng: &mut StdRng) -> OrderId {
        assert!(!self.orders.is_empty(), "cannot cancel from an empty side");
        self.orders[rng.random_range(0..self.orders.len())].id
    }

    fn remove(&mut self, order_id: OrderId) -> ActiveOrder {
        let index = self
            .positions
            .remove(&order_id)
            .unwrap_or_else(|| panic!("order {order_id} is not active"));
        let removed = self.orders.swap_remove(index);
        assert_eq!(removed.id, order_id);

        if index < self.orders.len() {
            let moved = self.orders[index];
            let previous_index = self
                .positions
                .insert(moved.id, index)
                .expect("moved active order is missing from the index");
            assert_eq!(
                previous_index,
                self.orders.len(),
                "moved active order had an incorrect old index"
            );
        }

        removed
    }

    fn len(&self) -> usize {
        self.orders.len()
    }
}

struct BookModel {
    bids: ActiveSide,
    asks: ActiveSide,
    bid_fifo: [VecDeque<OrderId>; PRICE_LEVEL_COUNT],
    ask_fifo: [VecDeque<OrderId>; PRICE_LEVEL_COUNT],
    bid_depth: Vec<u32>,
    ask_depth: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
struct ExpectedFill {
    maker_order_id: OrderId,
    price: u32,
}

impl BookModel {
    fn new() -> Self {
        Self {
            bids: ActiveSide::with_capacity(INITIAL_ORDERS_PER_SIDE + ADDS_PER_SIDE_PER_ROUND),
            asks: ActiveSide::with_capacity(INITIAL_ORDERS_PER_SIDE + ADDS_PER_SIDE_PER_ROUND),
            bid_fifo: std::array::from_fn(|_| VecDeque::new()),
            ask_fifo: std::array::from_fn(|_| VecDeque::new()),
            bid_depth: vec![0; MAX_PRICE_EXCLUSIVE as usize],
            ask_depth: vec![0; MAX_PRICE_EXCLUSIVE as usize],
        }
    }

    fn insert(&mut self, side: Side, order_id: OrderId, price: u32) {
        let active = ActiveOrder {
            id: order_id,
            price,
        };
        match side {
            Side::Bid => {
                self.bids.insert(active);
                self.bid_fifo[(price - BID_MIN_PRICE) as usize].push_back(order_id);
                self.bid_depth[price as usize] += ORDER_QUANTITY;
            }
            Side::Ask => {
                self.asks.insert(active);
                self.ask_fifo[(price - MID_PRICE) as usize].push_back(order_id);
                self.ask_depth[price as usize] += ORDER_QUANTITY;
            }
        }
    }

    fn random_id(&self, side: Side, rng: &mut StdRng) -> OrderId {
        match side {
            Side::Bid => self.bids.random_id(rng),
            Side::Ask => self.asks.random_id(rng),
        }
    }

    fn cancel(&mut self, side: Side, order_id: OrderId) -> ActiveOrder {
        let removed = match side {
            Side::Bid => self.bids.remove(order_id),
            Side::Ask => self.asks.remove(order_id),
        };

        let fifo = match side {
            Side::Bid => &mut self.bid_fifo[(removed.price - BID_MIN_PRICE) as usize],
            Side::Ask => &mut self.ask_fifo[(removed.price - MID_PRICE) as usize],
        };
        let fifo_position = fifo
            .iter()
            .position(|&id| id == order_id)
            .expect("cancelled order is missing from its FIFO queue");
        assert_eq!(fifo.remove(fifo_position), Some(order_id));

        self.decrement_depth(side, removed.price);
        removed
    }

    fn take_expected_market_fills(
        &mut self,
        market_side: Side,
    ) -> [ExpectedFill; FILLS_PER_MARKET] {
        let maker_side = match market_side {
            Side::Bid => Side::Ask,
            Side::Ask => Side::Bid,
        };

        std::array::from_fn(|_| {
            let (price, order_id) = self
                .pop_best_fifo(maker_side)
                .expect("shadow book has insufficient market liquidity");
            let removed = match maker_side {
                Side::Bid => self.bids.remove(order_id),
                Side::Ask => self.asks.remove(order_id),
            };
            assert_eq!(removed.price, price);
            self.decrement_depth(maker_side, price);
            ExpectedFill {
                maker_order_id: order_id,
                price,
            }
        })
    }

    fn pop_best_fifo(&mut self, maker_side: Side) -> Option<(u32, OrderId)> {
        match maker_side {
            Side::Bid => {
                for index in (0..PRICE_LEVEL_COUNT).rev() {
                    if let Some(order_id) = self.bid_fifo[index].pop_front() {
                        return Some((BID_MIN_PRICE + index as u32, order_id));
                    }
                }
            }
            Side::Ask => {
                for index in 0..PRICE_LEVEL_COUNT {
                    if let Some(order_id) = self.ask_fifo[index].pop_front() {
                        return Some((MID_PRICE + index as u32, order_id));
                    }
                }
            }
        }
        None
    }

    fn decrement_depth(&mut self, side: Side, price: u32) {
        let depth = match side {
            Side::Bid => &mut self.bid_depth[price as usize],
            Side::Ask => &mut self.ask_depth[price as usize],
        };
        *depth = depth
            .checked_sub(ORDER_QUANTITY)
            .expect("model depth underflow");
    }

    fn depth(&self, side: Side, price: u32) -> u32 {
        match side {
            Side::Bid => self.bid_depth[price as usize],
            Side::Ask => self.ask_depth[price as usize],
        }
    }

    fn assert_round_boundary_depth(&self) {
        assert_eq!(self.bids.len(), INITIAL_ORDERS_PER_SIDE);
        assert_eq!(self.asks.len(), INITIAL_ORDERS_PER_SIDE);
    }
}

fn run_steady_state<O: OrderbookTrait>(seed: u64, config: &BenchmarkConfig) -> SteadyStateResults {
    let mut trackers = ScenarioTrackers::new(config);

    for batch_index in 0..config.book_batches {
        let batch_seed = seed.wrapping_add(batch_index as u64);
        let mut rng = StdRng::seed_from_u64(batch_seed);
        let mut book = O::new();
        let mut id_counter = IdCounter::new();
        let mut model = BookModel::new();

        prefill_book(&mut book, &mut model, &mut id_counter, &mut rng);

        for _ in 0..WARMUP_ROUNDS_PER_BOOK {
            run_round(&mut book, &mut model, &mut id_counter, &mut rng, None);
        }

        for _ in 0..config.rounds_in_batch(batch_index) {
            run_round(
                &mut book,
                &mut model,
                &mut id_counter,
                &mut rng,
                Some(&mut trackers),
            );
        }

        // Full public-API validation runs only after all measurements for this
        // logical book, so it cannot warm data used by a later timed round.
        assert_book_matches_model(&book, &model);
    }

    assert_eq!(trackers.add_order.len(), config.adds());
    assert_eq!(trackers.cancel_order.len(), config.cancels());
    assert_eq!(trackers.market_order.len(), config.markets());

    SteadyStateResults {
        add_order: trackers
            .add_order
            .precentiles()
            .expect("add_order tracker has no samples"),
        cancel_order: trackers
            .cancel_order
            .precentiles()
            .expect("cancel_order tracker has no samples"),
        market_order: trackers
            .market_order
            .precentiles()
            .expect("market_order tracker has no samples"),
    }
}

fn prefill_book<O: OrderbookTrait>(
    book: &mut O,
    model: &mut BookModel,
    id_counter: &mut IdCounter,
    rng: &mut StdRng,
) {
    for _ in 0..INITIAL_ORDERS_PER_SIDE {
        add_order(book, model, id_counter, rng, Side::Bid, None);
        add_order(book, model, id_counter, rng, Side::Ask, None);
    }
    model.assert_round_boundary_depth();
}

fn run_round<O: OrderbookTrait>(
    book: &mut O,
    model: &mut BookModel,
    id_counter: &mut IdCounter,
    rng: &mut StdRng,
    mut trackers: Option<&mut ScenarioTrackers>,
) {
    model.assert_round_boundary_depth();
    let mut operations = ROUND_OPERATIONS;
    operations.shuffle(rng);

    for operation in operations {
        match operation {
            Operation::Add(side) => {
                let tracker = trackers.as_deref_mut().map(|t| &mut t.add_order);
                add_order(book, model, id_counter, rng, side, tracker);
            }
            Operation::Cancel(side) => {
                let tracker = trackers.as_deref_mut().map(|t| &mut t.cancel_order);
                cancel_order(book, model, rng, side, tracker);
            }
            Operation::Market(side) => {
                let tracker = trackers.as_deref_mut().map(|t| &mut t.market_order);
                execute_market_order(book, model, side, tracker);
            }
        }
    }

    model.assert_round_boundary_depth();
}

fn add_order<O: OrderbookTrait>(
    book: &mut O,
    model: &mut BookModel,
    id_counter: &mut IdCounter,
    rng: &mut StdRng,
    side: Side,
    tracker: Option<&mut LatencyTracker>,
) {
    let price = random_price(side, rng);
    let order = Order::new(
        Price::define(price),
        Quantity::define(ORDER_QUANTITY),
        side,
        id_counter,
    );
    let order_id = order.id();

    let result = match tracker {
        Some(tracker) => tracker.record(|| book.add_order(order)),
        None => book.add_order(order),
    };
    result.unwrap_or_else(|error| panic!("steady-state add_order failed: {error}"));
    model.insert(side, order_id, price);
}

fn cancel_order<O: OrderbookTrait>(
    book: &mut O,
    model: &mut BookModel,
    rng: &mut StdRng,
    side: Side,
    tracker: Option<&mut LatencyTracker>,
) {
    let order_id = model.random_id(side, rng);
    let result = match tracker {
        Some(tracker) => tracker.record(|| book.cancel_order(order_id)),
        None => book.cancel_order(order_id),
    };
    result.unwrap_or_else(|error| panic!("steady-state cancel_order failed: {error}"));
    model.cancel(side, order_id);
}

fn execute_market_order<O: OrderbookTrait>(
    book: &mut O,
    model: &mut BookModel,
    market_side: Side,
    tracker: Option<&mut LatencyTracker>,
) {
    // Advance the independent FIFO/price-priority oracle before observing the
    // implementation result. A bad implementation therefore cannot redefine
    // the expected state or alter later cancellation choices.
    let expected_fills = model.take_expected_market_fills(market_side);
    let quantity = Quantity::define(MARKET_QUANTITY);
    let result = match tracker {
        Some(tracker) => tracker.record(|| book.execute_market_order(market_side, quantity)),
        None => book.execute_market_order(market_side, quantity),
    };
    let fills = result.unwrap_or_else(|error| panic!("steady-state market order failed: {error}"));
    assert_eq!(
        fills.len(),
        FILLS_PER_MARKET,
        "market order must consume exactly {FILLS_PER_MARKET} makers"
    );
    for (fill, expected) in fills.iter().zip(expected_fills) {
        assert_eq!(
            fill.quantity.value(),
            ORDER_QUANTITY,
            "market order must fully consume one resting order per fill"
        );
        assert_eq!(
            fill.maker_order_id, expected.maker_order_id,
            "market order violated price priority or FIFO maker order"
        );
        assert_eq!(
            fill.price.value(),
            expected.price,
            "market order returned an incorrect fill price"
        );
    }
}

fn random_price(side: Side, rng: &mut StdRng) -> u32 {
    match side {
        Side::Bid => rng.random_range(BID_MIN_PRICE..MID_PRICE),
        Side::Ask => rng.random_range(MID_PRICE..ASK_MAX_PRICE_EXCLUSIVE),
    }
}

fn assert_book_matches_model<O: OrderbookTrait>(book: &O, model: &BookModel) {
    model.assert_round_boundary_depth();

    for price in MIN_PRICE..MAX_PRICE_EXCLUSIVE {
        for side in [Side::Bid, Side::Ask] {
            assert_eq!(
                book.depth_at_price(Price::define(price), side),
                model.depth(side, price),
                "book depth disagrees with model at price {price} on side {side:?}"
            );
        }
    }

    let expected_best_bid = (MIN_PRICE..MID_PRICE)
        .rev()
        .find(|&price| model.depth(Side::Bid, price) > 0)
        .map(Price::define);
    let expected_best_ask = (MID_PRICE..MAX_PRICE_EXCLUSIVE)
        .find(|&price| model.depth(Side::Ask, price) > 0)
        .map(Price::define);

    assert_eq!(book.best_bid(), expected_best_bid);
    assert_eq!(book.best_ask(), expected_best_ask);

    if let (Some(best_bid), Some(best_ask)) = (book.best_bid(), book.best_ask()) {
        assert!(
            best_bid.value() < best_ask.value(),
            "steady-state book must never be crossed"
        );
    }
}

fn print_results(results: &SteadyStateResults, cpu_ghz: f64) {
    print_operation("add_order()", &results.add_order, cpu_ghz);
    println!();
    print_operation("cancel_order()", &results.cancel_order, cpu_ghz);
    println!();
    print_operation("execute_market_order()", &results.market_order, cpu_ghz);
}

fn print_operation(name: &str, p: &Percentiles, cpu_ghz: f64) {
    println!("{name}:");
    for (label, cycles) in [
        ("p50", p.p50),
        ("p99", p.p99),
        ("p99.9", p.p999),
        ("p99.99", p.p9999),
        ("Max", p.max),
    ] {
        println!(
            "  {:<6} {:>8} cycles  ({:>8.1} ns)",
            format!("{label}:"),
            cycles,
            cycles_to_ns(cycles, cpu_ghz)
        );
    }
}

fn print_comparison(
    fixed: &SteadyStateResults,
    soa: &SteadyStateResults,
    hybrid: &SteadyStateResults,
    tree: &SteadyStateResults,
) {
    println!(
        "{:<15} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Operation", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<75}", "");

    for (operation, fixed_p, soa_p, hybrid_p, tree_p) in [
        (
            "add_order",
            &fixed.add_order,
            &soa.add_order,
            &hybrid.add_order,
            &tree.add_order,
        ),
        (
            "cancel_order",
            &fixed.cancel_order,
            &soa.cancel_order,
            &hybrid.cancel_order,
            &tree.cancel_order,
        ),
        (
            "market_order",
            &fixed.market_order,
            &soa.market_order,
            &hybrid.market_order,
            &tree.market_order,
        ),
    ] {
        println!(
            "{:<15} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
            operation, fixed_p.p50, soa_p.p50, hybrid_p.p50, tree_p.p50
        );
    }
}

fn print_tail_analysis(
    fixed: &SteadyStateResults,
    soa: &SteadyStateResults,
    hybrid: &SteadyStateResults,
    tree: &SteadyStateResults,
) {
    println!(
        "{:<15} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Operation", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<75}", "");

    for (operation, fixed_p, soa_p, hybrid_p, tree_p) in [
        (
            "add_order",
            &fixed.add_order,
            &soa.add_order,
            &hybrid.add_order,
            &tree.add_order,
        ),
        (
            "cancel_order",
            &fixed.cancel_order,
            &soa.cancel_order,
            &hybrid.cancel_order,
            &tree.cancel_order,
        ),
        (
            "market_order",
            &fixed.market_order,
            &soa.market_order,
            &hybrid.market_order,
            &tree.market_order,
        ),
    ] {
        println!(
            "{:<15} | {:>10.1}x | {:>10.1}x | {:>10.1}x | {:>10.1}x",
            operation,
            tail_ratio(fixed_p),
            tail_ratio(soa_p),
            tail_ratio(hybrid_p),
            tail_ratio(tree_p)
        );
    }
    println!("\nLower p99/p50 means more predictable common-tail latency.");
}

fn tail_ratio(p: &Percentiles) -> f64 {
    if p.p50 == 0 {
        0.0
    } else {
        p.p99 as f64 / p.p50 as f64
    }
}

fn print_weighted_means(
    fixed: &SteadyStateResults,
    soa: &SteadyStateResults,
    hybrid: &SteadyStateResults,
    tree: &SteadyStateResults,
    cpu_ghz: f64,
) {
    for (name, results) in [
        ("Fixed-Tick", fixed),
        ("SoA", soa),
        ("Hybrid", hybrid),
        ("Tree", tree),
    ] {
        let mean_cycles = workload_weighted_mean(results);
        println!(
            "  {:<12}: {:>9.1} cycles / {:>8.1} ns",
            name,
            mean_cycles,
            mean_cycles / cpu_ghz
        );
    }
    println!(
        "  Uses measured means with the exact 60/30/10 operation mix; it is not an average of medians."
    );
}

fn workload_weighted_mean(results: &SteadyStateResults) -> f64 {
    (results.add_order.mean * ADDS_PER_ROUND as f64
        + results.cancel_order.mean * CANCELS_PER_ROUND as f64
        + results.market_order.mean * MARKET_ORDERS_PER_ROUND as f64)
        / OPERATIONS_PER_ROUND as f64
}

fn export_csv(
    config: &BenchmarkConfig,
    cpu_ghz: f64,
    fixed: &SteadyStateResults,
    soa: &SteadyStateResults,
    hybrid: &SteadyStateResults,
    tree: &SteadyStateResults,
) {
    let output_name = if config.market_samples == DEFAULT_MARKET_SAMPLES {
        "scenario_steady_state".to_owned()
    } else {
        format!("scenario_steady_state_smoke_{}", config.market_samples)
    };
    let implementations = [
        ("fixed_tick", fixed),
        ("soa", soa),
        ("hybrid", hybrid),
        ("tree", tree),
    ];

    match CsvExporter::create(&output_name) {
        Ok(mut csv) => {
            for (implementation, results) in implementations {
                for (operation, percentiles) in [
                    ("add_order", &results.add_order),
                    ("cancel_order", &results.cancel_order),
                    ("market_order_qty_300", &results.market_order),
                ] {
                    if let Err(error) = csv.append(&ResultRow {
                        scenario: "scenario_steady_state",
                        implementation,
                        operation,
                        cpu_ghz,
                        percentiles,
                    }) {
                        eprintln!("Warning: could not append steady-state CSV row: {error}");
                    }
                }
            }
            if let Err(error) = csv.flush() {
                eprintln!("Warning: could not flush steady-state CSV: {error}");
            }
        }
        Err(error) => eprintln!("Warning: could not write CSV: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_template_has_exact_stationary_mix() {
        let mut adds = [0_usize; 2];
        let mut cancels = [0_usize; 2];
        let mut markets = [0_usize; 2];

        for operation in ROUND_OPERATIONS {
            let (counts, side) = match operation {
                Operation::Add(side) => (&mut adds, side),
                Operation::Cancel(side) => (&mut cancels, side),
                Operation::Market(side) => (&mut markets, side),
            };
            counts[side_index(side)] += 1;
        }

        assert_eq!(adds, [6, 6]);
        assert_eq!(cancels, [3, 3]);
        assert_eq!(markets, [1, 1]);
        assert_eq!(ADDS_PER_ROUND, CANCELS_PER_ROUND + 2 * FILLS_PER_MARKET);
    }

    #[test]
    fn active_side_swap_remove_keeps_indices_valid() {
        let mut active = ActiveSide::with_capacity(4);
        for id in 10..14 {
            active.insert(ActiveOrder { id, price: 4_999 });
        }

        assert_eq!(active.remove(11).id, 11);
        assert_eq!(active.remove(13).id, 13);
        assert_eq!(active.remove(10).id, 10);
        assert_eq!(active.remove(12).id, 12);
        assert_eq!(active.len(), 0);
    }

    #[test]
    fn shuffled_rounds_keep_real_book_and_oracle_in_sync() {
        let config = BenchmarkConfig {
            market_samples: 6,
            measured_rounds: 3,
            book_batches: 1,
        };
        let mut book = FixedTickOrderbook::new();
        let mut model = BookModel::new();
        let mut id_counter = IdCounter::new();
        let mut rng = StdRng::seed_from_u64(7);
        let mut trackers = ScenarioTrackers::new(&config);

        prefill_book(&mut book, &mut model, &mut id_counter, &mut rng);
        for _ in 0..config.measured_rounds {
            run_round(
                &mut book,
                &mut model,
                &mut id_counter,
                &mut rng,
                Some(&mut trackers),
            );
        }

        assert_eq!(trackers.add_order.len(), 36);
        assert_eq!(trackers.cancel_order.len(), 18);
        assert_eq!(trackers.market_order.len(), 6);
        assert_book_matches_model(&book, &model);
    }

    fn side_index(side: Side) -> usize {
        match side {
            Side::Bid => 0,
            Side::Ask => 1,
        }
    }
}
