use soroban_sdk::{contracttype, String, Symbol};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    EventInfo,
    TokenIdCounter,
    Tier(Symbol),
    Ticket(u32),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventInfo {
    pub start_time: u64,
    pub refund_cutoff_time: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tier {
    pub name: String,
    pub base_price: i128,
    pub current_price: i128,
    pub max_supply: u32,
    pub minted: u32,
    pub active: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ticket {
    pub tier_symbol: Symbol,
    pub purchase_time: u64,
    pub price_paid: i128,
    pub is_valid: bool,
}
