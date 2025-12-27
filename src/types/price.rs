#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Price(u32);

impl Price {
    pub fn define(price: u32) -> Self {
        Self(price)
    }
}
