#[allow(unused)]

pub struct Resolution(u64, u64);

impl Resolution {
    pub fn define(tick: u64, lot: u64) -> Self {
        Self(tick, lot)
    }
}
