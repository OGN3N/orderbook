#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Quantity(u32);

impl Quantity {
    pub fn define(quantity: u32) -> Self {
        Self(quantity)
    }
}
