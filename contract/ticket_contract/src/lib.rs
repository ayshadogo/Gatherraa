#![no_std]

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, contracttype, token, Address, Env, String, Symbol, Vec};
use stellar_access::ownable::{self as ownable, Ownable};
use stellar_tokens::non_fungible::{Base, NonFungibleToken};

mod storage_types;
use storage_types::{DataKey, EventInfo, Ticket, Tier};

// Dynamic pricing constants
const PRICE_INCREASE_BPS: i128 = 500; // 5% increase per tier threshold

#[contract]
pub struct SoulboundTicketContract;

#[contractimpl]
impl SoulboundTicketContract {
    pub fn initialize(
        e: &Env,
        admin: Address,
        name: String,
        symbol: String,
        uri: String,
        start_time: u64,
        refund_cutoff_time: u64,
    ) {
        if e.storage().instance().has(&DataKey::Admin) {
            panic!("already initialized");
        }

        // Init Event Info
        let event_info = EventInfo {
            start_time,
            refund_cutoff_time,
        };
        e.storage().instance().set(&DataKey::EventInfo, &event_info);
        e.storage().instance().set(&DataKey::Admin, &admin);

        // Init Token Counter
        e.storage().instance().set(&DataKey::TokenIdCounter, &0u32);

        // Init Token Metadata via OpenZeppelin Base
        Base::set_metadata(e, uri, name, symbol);
        ownable::set_owner(e, &admin);
    }

    // Add a new ticket tier
    pub fn add_tier(e: &Env, tier_symbol: Symbol, name: String, base_price: i128, max_supply: u32) {
        let admin: Address = e.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let key = DataKey::Tier(tier_symbol.clone());
        if e.storage().persistent().has(&key) {
            panic!("Tier already exists");
        }

        let tier = Tier {
            name,
            base_price,
            current_price: base_price,
            max_supply,
            minted: 0,
            active: true,
        };

        e.storage().persistent().set(&key, &tier);
    }

    // Dynamic pricing query
    pub fn get_ticket_price(e: &Env, tier_symbol: Symbol) -> i128 {
        let key = DataKey::Tier(tier_symbol);
        let tier: Tier = e.storage().persistent().get(&key).unwrap();
        // Dynamic pricing logic: base_price * (1 + (minted / (max_supply / 5)) * 5%)
        let thresholds_passed = tier.minted / (tier.max_supply.max(1) / 5).max(1);
        let increase = tier.base_price * PRICE_INCREASE_BPS * (thresholds_passed as i128) / 10000;
        tier.base_price + increase
    }

    // Batch Minting for Organizer
    pub fn batch_mint(e: &Env, to: Address, tier_symbol: Symbol, amount: u32) {
        let admin: Address = e.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let key = DataKey::Tier(tier_symbol.clone());
        let mut tier: Tier = e
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic!("Tier not found"));

        if tier.minted + amount > tier.max_supply {
            panic!("Exceeds tier max supply");
        }

        for _ in 0..amount {
            // custom sequential increment
            let mut counter: u32 = e
                .storage()
                .instance()
                .get(&DataKey::TokenIdCounter)
                .unwrap();
            counter += 1;
            let token_id = counter;
            e.storage()
                .instance()
                .set(&DataKey::TokenIdCounter, &counter);

            Base::sequential_mint(e, &to);

            let ticket = Ticket {
                tier_symbol: tier_symbol.clone(),
                purchase_time: e.ledger().timestamp(),
                price_paid: 0, // Admin mints are free
                is_valid: true,
            };
            e.storage()
                .persistent()
                .set(&DataKey::Ticket(token_id), &ticket);
        }

        tier.minted += amount;
        e.storage().persistent().set(&key, &tier);
    }

    // Purchase a ticket
    pub fn purchase(e: &Env, buyer: Address, payment_token: Address, tier_symbol: Symbol) {
        buyer.require_auth();

        let key = DataKey::Tier(tier_symbol.clone());
        let mut tier: Tier = e
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic!("Tier not found"));

        if !tier.active {
            panic!("Tier is not active");
        }
        if tier.minted >= tier.max_supply {
            panic!("Tier sold out");
        }

        let price = Self::get_ticket_price(e, tier_symbol.clone());

        // Process payment
        let admin: Address = e.storage().instance().get(&DataKey::Admin).unwrap();
        let token_client = token::Client::new(e, &payment_token);
        token_client.transfer(&buyer, &admin, &price);

        // Mint Token
        let mut counter: u32 = e
            .storage()
            .instance()
            .get(&DataKey::TokenIdCounter)
            .unwrap();
        counter += 1;
        let token_id = counter;
        e.storage()
            .instance()
            .set(&DataKey::TokenIdCounter, &counter);

        Base::sequential_mint(e, &buyer);

        let ticket = Ticket {
            tier_symbol: tier_symbol.clone(),
            purchase_time: e.ledger().timestamp(),
            price_paid: price,
            is_valid: true,
        };
        e.storage()
            .persistent()
            .set(&DataKey::Ticket(token_id), &ticket);

        tier.minted += 1;
        e.storage().persistent().set(&key, &tier);
    }

    // Refund a ticket
    pub fn refund(e: &Env, owner: Address, payment_token: Address, token_id: u32) {
        owner.require_auth();

        let current_owner = Self::owner_of(e, token_id);
        if owner != current_owner {
            panic!("Not the ticket owner");
        }

        let event_info: EventInfo = e.storage().instance().get(&DataKey::EventInfo).unwrap();
        if e.ledger().timestamp() > event_info.refund_cutoff_time {
            panic!("Refund window closed");
        }

        let mut ticket: Ticket = e
            .storage()
            .persistent()
            .get(&DataKey::Ticket(token_id))
            .unwrap();
        if !ticket.is_valid {
            panic!("Ticket already invalidated");
        }

        // Process refund
        let admin: Address = e.storage().instance().get(&DataKey::Admin).unwrap();
        let token_client = token::Client::new(e, &payment_token);
        token_client.transfer(&admin, &owner, &ticket.price_paid);

        // Invalidate and Burn
        ticket.is_valid = false;
        e.storage()
            .persistent()
            .set(&DataKey::Ticket(token_id), &ticket);
        Base::burn(e, &owner, token_id);
    }

    // Ticket Validation
    pub fn validate_ticket(e: &Env, token_id: u32) -> bool {
        let key = DataKey::Ticket(token_id);
        if !e.storage().persistent().has(&key) {
            return false;
        }
        let ticket: Ticket = e.storage().persistent().get(&key).unwrap();
        ticket.is_valid
    }

    // View functions logic
    pub fn get_ticket(e: &Env, token_id: u32) -> Ticket {
        e.storage()
            .persistent()
            .get(&DataKey::Ticket(token_id))
            .unwrap()
    }
}

// Implement SEP-0054 via OpenZeppelin Interface
#[contractimpl]
impl NonFungibleToken for SoulboundTicketContract {
    type ContractType = Base;

    fn balance(e: &Env, owner: Address) -> u32 {
        Self::ContractType::balance(e, &owner)
    }

    fn owner_of(e: &Env, token_id: u32) -> Address {
        Self::ContractType::owner_of(e, token_id)
    }

    // Soulbound restrictions overrides
    fn transfer(_e: &Env, _from: Address, _to: Address, _token_id: u32) {
        panic!("Soulbound: Tickets cannot be transferred");
    }

    fn transfer_from(_e: &Env, _spender: Address, _from: Address, _to: Address, _token_id: u32) {
        panic!("Soulbound: Tickets cannot be transferred");
    }

    fn approve(
        _e: &Env,
        _approver: Address,
        _approved: Address,
        _token_id: u32,
        _live_until_ledger: u32,
    ) {
        panic!("Soulbound: Approval disabled for non-transferable tokens");
    }

    fn approve_for_all(_e: &Env, _owner: Address, _operator: Address, _live_until_ledger: u32) {
        panic!("Soulbound: Approval disabled for non-transferable tokens");
    }

    fn get_approved(_e: &Env, _token_id: u32) -> Option<Address> {
        None
    }

    fn is_approved_for_all(_e: &Env, _owner: Address, _operator: Address) -> bool {
        false
    }

    // Metadata
    fn name(e: &Env) -> String {
        Self::ContractType::name(e)
    }

    fn symbol(e: &Env) -> String {
        Self::ContractType::symbol(e)
    }

    fn token_uri(e: &Env, token_id: u32) -> String {
        Self::ContractType::token_uri(e, token_id)
    }
}

// Ownable Utils
#[contractimpl]
impl Ownable for SoulboundTicketContract {
    fn get_owner(e: &Env) -> Option<Address> {
        ownable::get_owner(e)
    }

    fn transfer_ownership(e: &Env, new_owner: Address, live_until_ledger: u32) {
        ownable::transfer_ownership(e, &new_owner, live_until_ledger);
    }

    fn accept_ownership(e: &Env) {
        ownable::accept_ownership(e);
    }

    fn renounce_ownership(e: &Env) {
        ownable::renounce_ownership(e);
    }
}
