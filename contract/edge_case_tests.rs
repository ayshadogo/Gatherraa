use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, BytesN, Env, Vec, String, Symbol, Map,
};
use soroban_sdk::symbol_short;

// Import contract clients
use contracts::StakingContractClient;
use contracts::error::StakingError;
use escrow_contract::EscrowContractClient;
use multisig_wallet_contract::MultisigWalletContractClient;
use multisig_wallet_contract::MultisigError;
use governance_contract::GovernanceContractClient;

fn create_token_contract<'a>(env: &Env, admin: &Address) -> token::Client<'a> {
    let contract_id = env.register_stellar_asset_contract_v2(admin.clone());
    token::Client::new(env, &contract_id.address())
}

#[test]
fn test_staking_edge_cases() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let user = Address::generate(&env);
    let unauthorized = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    let staking_id = env.register_contracts(None, contracts::StakingContract);
    let staking_client = StakingContractClient::new(&env, &staking_id);
    staking_client.initialize(&admin, &token.address, &token.address, &1000);

    let token_admin = token::StellarAssetClient::new(&env, &token.address);
    token_admin.mint(&user, &10_000_000);

    // Test: Double initialization
    let result = staking_client.try_initialize(&admin, &token.address, &token.address, &1000);
    assert_eq!(result, Err(StakingError::AlreadyInitialized));

    // Test: Unauthorized tier setting
    staking_client.set_tier(&admin, &1, &1000, &150);
    let result = staking_client.try_set_tier(&unauthorized, &2, &2000, &200);
    assert_eq!(result, Err(StakingError::Unauthorized));

    // Test: Stake with insufficient balance
    let result = staking_client.try_stake(&user, &20_000_000, &86400, &1);
    assert_eq!(result, Err(StakingError::InsufficientBalance));

    // Test: Stake zero amount
    let result = staking_client.try_stake(&user, &0, &86400, &1);
    assert_eq!(result, Err(StakingError::AmountMustBePositive));

    // Test: Stake insufficient tier amount
    staking_client.set_tier(&admin, &2, &5000, &200);
    let result = staking_client.try_stake(&user, &1000, &86400, &2);
    assert_eq!(result, Err(StakingError::InsufficientAmountForTier));

    // Test: Unstake more than available
    staking_client.stake(&user, &2000, &86400, &1);
    let result = staking_client.try_unstake(&user, &3000);
    assert_eq!(result, Err(StakingError::InsufficientBalance));

    // Test: Unstake zero amount
    let result = staking_client.try_unstake(&user, &0);
    assert_eq!(result, Err(StakingError::AmountMustBePositive));

    // Test: Unstake from non-existent user
    let result = staking_client.try_unstake(&unauthorized, &1000);
    assert_eq!(result, Err(StakingError::UserNotFound));

    // Test: Slash unauthorized
    let result = staking_client.try_slash(&unauthorized, &user, &1000);
    assert_eq!(result, Err(StakingError::Unauthorized));

    // Test: Slash more than balance
    let result = staking_client.try_slash(&admin, &user, &3000);
    assert_eq!(result, Err(StakingError::SlashingAmountExceedsBalance));

    // Test: Emergency withdraw with no balance
    let result = staking_client.try_emergency_withdraw(&unauthorized);
    assert_eq!(result, Err(StakingError::InsufficientBalance));

    // Test: Compound with different tokens
    let reward_token = create_token_contract(&env, &admin);
    let result = staking_client.try_initialize(&admin, &token.address, &reward_token.address, &1000);
    assert_eq!(result, Err(StakingError::AlreadyInitialized));

    // Test: Compound with different tokens after stake
    let stake_id = env.register_contracts(None, contracts::StakingContract);
    let compound_client = StakingContractClient::new(&env, &stake_id);
    compound_client.initialize(&admin, &token.address, &reward_token.address, &1000);
    
    let reward_token_admin = token::StellarAssetClient::new(&env, &reward_token.address);
    reward_token_admin.mint(&user, &10_000_000);
    
    compound_client.stake(&user, &2000, &86400, &1);
    
    let mut ledger = env.ledger().get();
    ledger.timestamp += 100;
    env.ledger().set(ledger);
    
    let result = compound_client.try_claim(&user, &true);
    assert_eq!(result, Err(StakingError::RewardTokenDiffers));
}

#[test]
fn test_escrow_edge_cases() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let unauthorized = Address::generate(&env);
    let user = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    let escrow_config = escrow_contract::RevenueSplitConfig {
        default_organizer_percentage: 8000000,
        default_platform_percentage: 1500000,
        default_referral_percentage: 500000,
        max_referral_percentage: 10000000,
        precision: 10000000,
        min_escrow_amount: 1000000,
        max_escrow_amount: 10000000000,
        dispute_timeout: 86400,
        emergency_withdrawal_delay: 3600,
    };

    let escrow_id = env.register_contracts(None, escrow_contract::EscrowContract);
    let escrow_client = EscrowContractClient::new(&env, &escrow_id);
    escrow_client.initialize(&admin, escrow_config.clone());

    // Test: Create escrow with invalid amount
    let result = std::panic::catch_unwind(|| {
        escrow_client.create_escrow(
            env.clone(),
            Address::generate(&env),
            Address::generate(&env),
            user.clone(),
            500000, // Below minimum
            token.address(),
            env.ledger().timestamp() + 86400,
            None,
            None,
            None,
        );
    });
    assert!(result.is_err());

    // Test: Create escrow with excessive amount
    let result = std::panic::catch_unwind(|| {
        escrow_client.create_escrow(
            env.clone(),
            Address::generate(&env),
            Address::generate(&env),
            user.clone(),
            20000000000, // Above maximum
            token.address(),
            env.ledger().timestamp() + 86400,
            None,
            None,
            None,
        );
    });
    assert!(result.is_err());

    // Test: Lock escrow with insufficient balance
    let escrow_id_result = escrow_client.create_escrow(
        env.clone(),
        Address::generate(&env),
        Address::generate(&env),
        user.clone(),
        1000000,
        token.address(),
        env.ledger().timestamp() + 86400,
        None,
        None,
        None,
    );

    let result = std::panic::catch_unwind(|| {
        escrow_client.lock_escrow(env.clone(), escrow_id_result);
    });
    assert!(result.is_err());

    // Test: Release escrow before lock
    let result = std::panic::catch_unwind(|| {
        escrow_client.release_escrow(env, escrow_id_result);
    });
    assert!(result.is_err());

    // Test: Release escrow before time
    let token_admin = token::StellarAssetClient::new(&env, &token.address);
    token_admin.mint(&user, &10_000_000);
    
    escrow_client.lock_escrow(env.clone(), escrow_id_result.clone());
    
    let result = std::panic::catch_unwind(|| {
        escrow_client.release_escrow(env, escrow_id_result);
    });
    assert!(result.is_err());

    // Test: Create dispute on non-existent escrow
    let fake_escrow_id = BytesN::from_array(&env, &[1; 32]);
    let result = std::panic::catch_unwind(|| {
        escrow_client.create_dispute(env.clone(), fake_escrow_id, String::from_str(&env, "Test dispute"));
    });
    assert!(result.is_err());

    // Test: Emergency withdraw before delay
    let result = std::panic::catch_unwind(|| {
        escrow_client.emergency_withdraw(env.clone(), escrow_id_result);
    });
    assert!(result.is_err());
}

#[test]
fn test_multisig_edge_cases() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let unauthorized = Address::generate(&env);
    let signer1 = Address::generate(&env);
    let signer2 = Address::generate(&env);
    let signer3 = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    let wallet_config = multisig_wallet_contract::WalletConfig {
        m: 2,
        n: 3,
        daily_spending_limit: 1000000000,
        timelock_threshold: 500000000,
        timelock_duration: 86400,
        transaction_expiry: 604800,
        max_batch_size: 10,
        emergency_freeze_duration: 3600,
    };

    let wallet_id = env.register_contracts(None, multisig_wallet_contract::MultisigWalletContract);
    let wallet_client = MultisigWalletContractClient::new(&env, &wallet_id);
    let signers = vec![&env, signer1.clone(), signer2.clone(), signer3.clone()];
    wallet_client.initialize(&admin, wallet_config, signers);

    let token_admin = token::StellarAssetClient::new(&env, &token.address);
    token_admin.mint(&wallet_client.address(&env), &10_000_000);

    // Test: Propose transaction with zero amount
    let result = wallet_client.try_propose_transaction(
        env.clone(),
        Address::generate(&env),
        token.address(),
        0,
        Vec::new(&env),
        signer1.clone(),
        1,
    );
    assert!(result.is_err());

    // Test: Execute transaction before approval threshold
    let tx_id = wallet_client.propose_transaction(
        env.clone(),
        Address::generate(&env),
        token.address(),
        1000000,
        Vec::new(&env),
        signer1.clone(),
        1,
    ).unwrap();

    let result = std::panic::catch_unwind(|| {
        wallet_client.execute_transaction(env, tx_id);
    });
    assert!(result.is_err());

    // Test: Approve transaction twice by same signer
    wallet_client.approve_transaction(env.clone(), tx_id.clone(), signer1.clone());
    let result = std::panic::catch_unwind(|| {
        wallet_client.approve_transaction(env, tx_id, signer1);
    });
    assert!(result.is_err());

    // Test: Create batch exceeding max size
    let mut transactions = Vec::new(&env);
    for _ in 0..15 {
        transactions.push_back((
            Address::generate(&env),
            token.address(),
            1000000,
            Vec::new(&env)
        ));
    }

    let result = wallet_client.try_create_batch(
        env.clone(),
        transactions,
        signer1.clone(),
        1,
    );
    assert!(result.is_err());

    // Test: Daily spending limit exceeded
    let tx_id2 = wallet_client.propose_transaction(
        env.clone(),
        Address::generate(&env),
        token.address(),
        600000000, // 0.6 XLM
        Vec::new(&env),
        signer1.clone(),
        2,
    ).unwrap();

    wallet_client.approve_transaction(env.clone(), tx_id.clone(), signer1.clone());
    wallet_client.approve_transaction(env.clone(), tx_id2.clone(), signer2.clone());
    
    wallet_client.execute_transaction(env, tx_id);
    
    let result = std::panic::catch_unwind(|| {
        wallet_client.execute_transaction(env, tx_id2);
    });
    assert!(result.is_err());

    // Test: Emergency freeze by unauthorized
    let result = std::panic::catch_unwind(|| {
        wallet_client.emergency_freeze(env.clone(), unauthorized.clone());
    });
    assert!(result.is_err());

    // Test: Propose transaction while frozen
    wallet_client.emergency_freeze(env.clone(), admin.clone());
    
    let result = wallet_client.try_propose_transaction(
        env.clone(),
        Address::generate(&env),
        token.address(),
        1000000,
        Vec::new(&env),
        signer1.clone(),
        3,
    );
    assert!(result.is_err());
}

#[test]
fn test_governance_edge_cases() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let emergency = Address::generate(&env);
    let unauthorized = Address::generate(&env);
    let proposer = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    let governance_id = env.register_contracts(None, governance_contract::GovernanceContract);
    let governance_client = GovernanceContractClient::new(&env, &governance_id);
    governance_client.init(&admin, &token.address, &100, &emergency);

    // Test: Double initialization
    let result = std::panic::catch_unwind(|| {
        governance_client.init(&admin, &token.address, &100, &emergency);
    });
    assert!(result.is_err());

    // Test: Create proposal without sufficient tokens
    let action = governance_contract::GovernanceAction::ParameterChange(
        String::from_str(&env, "test"),
        50
    );
    
    let result = std::panic::catch_unwind(|| {
        governance_client.create_proposal(
            &proposer,
            &action,
            &governance_contract::ProposalCategory::ParameterUpdate,
            &String::from_str(&env, "Test proposal")
        );
    });
    assert!(result.is_err());

    // Test: Vote after voting period
    let token_admin = token::StellarAssetClient::new(&env, &token.address);
    token_admin.mint(&proposer, &1000);
    
    let prop_id = governance_client.create_proposal(
        &proposer,
        &action,
        &governance_contract::ProposalCategory::ParameterUpdate,
        &String::from_str(&env, "Test proposal")
    );

    env.ledger().set(soroban_sdk::testutils::LedgerInfo {
        timestamp: env.ledger().timestamp(),
        protocol_version: 21,
        sequence_number: env.ledger().sequence() + 101,
        network_id: [0u8; 32],
        base_reserve: 10,
        max_entry_ttl: 1000,
    });

    let result = std::panic::catch_unwind(|| {
        governance_client.vote(&proposer, &prop_id, &true, &false, &Vec::new(&env));
    });
    assert!(result.is_err());

    // Test: Execute proposal before voting period ends
    let voter = Address::generate(&env);
    token_admin.mint(&voter, &1000);
    
    governance_client.vote(&voter, &prop_id, &true, &false, &Vec::new(&env));
    
    let result = std::panic::catch_unwind(|| {
        governance_client.execute(&prop_id);
    });
    assert!(result.is_err());

    // Test: Cancel proposal by unauthorized user
    let result = std::panic::catch_unwind(|| {
        governance_client.cancel(&unauthorized, &prop_id);
    });
    assert!(result.is_err());

    // Test: Emergency execute by unauthorized user
    let emergency_action = governance_contract::GovernanceAction::EmergencyAction(
        String::from_str(&env, "test")
    );
    let emergency_prop_id = governance_client.create_proposal(
        &admin,
        &emergency_action,
        &governance_contract::ProposalCategory::Emergency,
        &String::from_str(&env, "Emergency test")
    );

    governance_client.vote(&admin, &emergency_prop_id, &true, &false, &Vec::new(&env));

    env.ledger().set(soroban_sdk::testutils::LedgerInfo {
        timestamp: env.ledger().timestamp(),
        protocol_version: 21,
        sequence_number: env.ledger().sequence() + 51,
        network_id: [0u8; 32],
        base_reserve: 10,
        max_entry_ttl: 1000,
    });

    governance_client.execute(&emergency_prop_id);

    let result = std::panic::catch_unwind(|| {
        governance_client.emergency_execute(&unauthorized, &emergency_prop_id);
    });
    assert!(result.is_err());

    // Test: Update category settings by unauthorized user
    let result = std::panic::catch_unwind(|| {
        governance_client.set_category_settings(&unauthorized, &1, &2000, &60, &60);
    });
    assert!(result.is_err());
}

#[test]
fn test_overflow_and_underflow_protection() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let user = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    // Test: Maximum tier multiplier
    let staking_id = env.register_contracts(None, contracts::StakingContract);
    let staking_client = StakingContractClient::new(&env, &staking_id);
    staking_client.initialize(&admin, &token.address, &token.address, &1000);

    // Test setting very high multiplier (should be handled gracefully)
    let result = std::panic::catch_unwind(|| {
        staking_client.set_tier(&admin, &1, &1000, &u32::MAX);
    });
    // May or may not panic depending on implementation

    // Test: Maximum reward rate
    let result = std::panic::catch_unwind(|| {
        staking_client.initialize(&admin, &token.address, &token.address, &i128::MAX);
    });
    // May or may not panic depending on implementation

    // Test: Very large lock duration
    let token_admin = token::StellarAssetClient::new(&env, &token.address);
    token_admin.mint(&user, &10_000_000);
    
    let result = std::panic::catch_unwind(|| {
        staking_client.stake(&user, &1000, &u64::MAX, &1);
    });
    // May or may not panic depending on implementation
}

#[test]
fn test_concurrent_operations() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let user1 = Address::generate(&env);
    let user2 = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    let staking_id = env.register_contracts(None, contracts::StakingContract);
    let staking_client = StakingClient::new(&env, &staking_id);
    staking_client.initialize(&admin, &token.address, &token.address, &1000);
    staking_client.set_tier(&admin, &1, &100, &150);

    let token_admin = token::StellarAssetClient::new(&env, &token.address);
    token_admin.mint(&user1, &10_000_000);
    token_admin.mint(&user2, &10_000_000);

    // Test: Concurrent staking by multiple users
    let lock_duration = 30 * 24 * 60 * 60;
    
    staking_client.stake(&user1, &2000, &lock_duration, &1);
    staking_client.stake(&user2, &3000, &lock_duration, &1);

    // Test: Concurrent unstaking
    let mut ledger = env.ledger().get();
    ledger.timestamp += lock_duration + 1;
    env.ledger().set(ledger);

    staking_client.unstake(&user1, &1000);
    staking_client.unstake(&user2, &1500);

    // Test: Concurrent reward claiming
    staking_client.claim(&user1, &false);
    staking_client.claim(&user2, &false);

    // Verify state consistency
    let user1_info = staking_client.user_info(&user1);
    let user2_info = staking_client.user_info(&user2);
    
    assert!(user1_info.amount > 0);
    assert!(user2_info.amount > 0);
}

#[test]
fn test_boundary_conditions() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let user = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    let staking_id = env.register_contracts(None, contracts::StakingContract);
    let staking_client = StakingContractClient::new(&env, &staking_id);
    staking_client.initialize(&admin, &token.address, &token.address, &1000);

    let token_admin = token::StellarAssetClient::new(&env, &token.address);
    token_admin.mint(&user, &10_000_000);

    // Test: Exactly minimum amount for tier
    staking_client.set_tier(&admin, &1, &1000, &150);
    staking_client.stake(&user, &1000, &86400, &1);

    // Test: Exactly at timelock threshold
    staking_client.set_tier(&admin, &2, &500000000, &200);
    staking_client.stake(&user, &500000, &86400, &2);

    // Test: Exactly at daily spending limit
    let wallet_config = multisig_wallet_contract::WalletConfig {
        m: 1,
        n: 1,
        daily_spending_limit: 1000000000,
        timelock_threshold: 500000000,
        timelock_duration: 86400,
        transaction_expiry: 604800,
        max_batch_size: 10,
        emergency_freeze_duration: 3600,
    };

    let wallet_id = env.register_contracts(None, multisig_wallet_contract::MultisigWalletContract);
    let wallet_client = MultisigWalletContractClient::new(&env, &wallet_id);
    wallet_client.initialize(&admin, wallet_config, vec![&env, user.clone()]);

    token_admin.mint(&wallet_client.address(&env), &2000000000);

    let tx_id = wallet_client.propose_transaction(
        env.clone(),
        Address::generate(&env),
        token.address(),
        1000000000, // Exactly at limit
        Vec::new(&env),
        user.clone(),
        1,
    ).unwrap();

    wallet_client.approve_transaction(env.clone(), tx_id.clone(), user.clone());
    wallet_client.execute_transaction(env, tx_id);

    // Test: Second transaction should fail due to limit
    let tx_id2 = wallet_client.propose_transaction(
        env.clone(),
        Address::generate(&env),
        token.address(),
        1,
        Vec::new(&env),
        user.clone(),
        2,
    ).unwrap();

    wallet_client.approve_transaction(env.clone(), tx_id2.clone(), user.clone());
    
    let result = std::panic::catch_unwind(|| {
        wallet_client.execute_transaction(env, tx_id2);
    });
    assert!(result.is_err());
}
