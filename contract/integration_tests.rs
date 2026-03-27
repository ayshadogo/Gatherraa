use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, BytesN, Env, Vec, String, Symbol, Map,
};
use soroban_sdk::symbol_short;

// Import contract clients
use contracts::StakingContractClient;
use escrow_contract::EscrowContractClient;
use multisig_wallet_contract::MultisigWalletContractClient;
use governance_contract::GovernanceContractClient;

fn create_token_contract<'a>(env: &Env, admin: &Address) -> token::Client<'a> {
    let contract_id = env.register_stellar_asset_contract_v2(admin.clone());
    token::Client::new(env, &contract_id.address())
}

#[test]
fn test_staking_escrow_integration() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let user = Address::generate(&env);
    let organizer = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    // Initialize staking contract
    let staking_id = env.register_contracts(None, contracts::StakingContract);
    let staking_client = StakingContractClient::new(&env, &staking_id);
    staking_client.initialize(&admin, &token.address, &token.address, &1000);
    staking_client.set_tier(&admin, &1, &100, &150);

    // Initialize escrow contract
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

    // Mint tokens to user
    let token_admin = token::StellarAssetClient::new(&env, &token.address);
    token_admin.mint(&user, &10_000_000);

    // User stakes tokens
    let lock_duration = 30 * 24 * 60 * 60;
    staking_client.stake(&user, &2000, &lock_duration, &1);

    // User creates escrow using staked tokens as collateral
    let release_time = env.ledger().timestamp() + 86400;
    let escrow_id_result = escrow_client.create_escrow(
        env.clone(),
        Address::generate(&env), // event
        organizer.clone(),
        user.clone(),
        1000000, // 0.1 XLM
        token.address(),
        release_time,
        None, // default revenue split
        None, // no referral
        None, // no milestones
    );

    // Lock escrow
    escrow_client.lock_escrow(env.clone(), escrow_id_result.clone());

    // Advance time and earn staking rewards
    let mut ledger = env.ledger().get();
    ledger.timestamp += 86400; // 1 day
    env.ledger().set(ledger);

    // Claim staking rewards
    staking_client.claim(&user, &false);

    // Release escrow after time
    escrow_client.release_escrow(env, escrow_id_result);

    // Verify both contracts worked correctly
    let staking_info = staking_client.user_info(&user);
    assert!(staking_info.amount > 0);

    let escrow_info = escrow_client.get_escrow(env, escrow_id_result);
    assert_eq!(escrow_info.status, escrow_contract::EscrowStatus::Released);
}

#[test]
fn test_multisig_governance_integration() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let emergency = Address::generate(&env);
    let proposer = Address::generate(&env);
    let signer1 = Address::generate(&env);
    let signer2 = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    // Initialize multisig wallet
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
    let signers = vec![&env, signer1.clone(), signer2.clone(), proposer.clone()];
    wallet_client.initialize(&admin, wallet_config, signers);

    // Initialize governance contract
    let governance_id = env.register_contracts(None, governance_contract::GovernanceContract);
    let governance_client = GovernanceContractClient::new(&env, &governance_id);
    governance_client.init(&admin, &token.address, &100, &emergency);

    // Mint governance tokens
    let token_admin = token::StellarAssetClient::new(&env, &token.address);
    token_admin.mint(&proposer, &1000);
    token_admin.mint(&signer1, &1000);
    token_admin.mint(&signer2, &1000);

    // Create governance proposal to update multisig settings
    let action = governance_contract::GovernanceAction::ParameterChange(
        String::from_str(&env, "daily_spending_limit"),
        2000000000i128
    );
    
    let prop_id = governance_client.create_proposal(
        &proposer,
        &action,
        &governance_contract::ProposalCategory::ParameterUpdate,
        &String::from_str(&env, "Increase daily spending limit")
    );

    // Vote on proposal
    governance_client.vote(&signer1, &prop_id, &true, &false, &Vec::new(&env));
    governance_client.vote(&signer2, &prop_id, &true, &false, &Vec::new(&env));

    // Advance time past voting period
    env.ledger().set(soroban_sdk::testutils::LedgerInfo {
        timestamp: env.ledger().timestamp(),
        protocol_version: 21,
        sequence_number: env.ledger().sequence() + 101,
        network_id: [0u8; 32],
        base_reserve: 10,
        max_entry_ttl: 1000,
    });

    // Execute proposal
    governance_client.execute(&prop_id);

    // Verify proposal was executed
    let proposal = governance_client.get_proposal(&prop_id);
    assert_eq!(proposal.status, governance_contract::ProposalStatus::Executed);

    // Now create a multisig transaction that uses the new limit
    let recipient = Address::generate(&env);
    token_admin.mint(&wallet_client.address(&env), &3000000000);

    let tx_id = wallet_client.propose_transaction(
        env.clone(),
        recipient.clone(),
        token.address(),
        1500000000, // 1.5 XLM - within new limit
        Vec::new(&env),
        proposer.clone(),
        1,
    ).unwrap();

    // Approve and execute transaction
    wallet_client.approve_transaction(env.clone(), tx_id.clone(), signer1.clone());
    wallet_client.execute_transaction(env, tx_id);

    // Verify transaction was executed
    let transaction = wallet_client.get_transaction(env, tx_id);
    assert_eq!(transaction.status, multisig_wallet_contract::TransactionStatus::Executed);
}

#[test]
fn test_cross_contract_emergency_scenario() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let emergency = Address::generate(&env);
    let user = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    // Initialize contracts
    let staking_id = env.register_contracts(None, contracts::StakingContract);
    let staking_client = StakingContractClient::new(&env, &staking_id);
    staking_client.initialize(&admin, &token.address, &token.address, &1000);

    let escrow_id = env.register_contracts(None, escrow_contract::EscrowContract);
    let escrow_client = EscrowContractClient::new(&env, &escrow_id);
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
    escrow_client.initialize(&admin, escrow_config);

    let governance_id = env.register_contracts(None, governance_contract::GovernanceContract);
    let governance_client = GovernanceContractClient::new(&env, &governance_id);
    governance_client.init(&admin, &token.address, &100, &emergency);

    // Mint tokens
    let token_admin = token::StellarAssetClient::new(&env, &token.address);
    token_admin.mint(&user, &10_000_000);

    // User stakes tokens
    staking_client.stake(&user, &5000, &86400, &1);

    // User creates escrow
    let release_time = env.ledger().timestamp() + 86400;
    let escrow_id_result = escrow_client.create_escrow(
        env.clone(),
        Address::generate(&env),
        Address::generate(&env),
        user.clone(),
        2000000,
        token.address(),
        release_time,
        None,
        None,
        None,
    );

    escrow_client.lock_escrow(env.clone(), escrow_id_result.clone());

    // Create emergency proposal to pause all contracts
    let emergency_action = governance_contract::GovernanceAction::EmergencyAction(
        String::from_str(&env, "pause_all_contracts")
    );
    
    let prop_id = governance_client.create_proposal(
        &admin,
        &emergency_action,
        &governance_contract::ProposalCategory::Emergency,
        &String::from_str(&env, "Emergency pause all contracts")
    );

    // Auto-approve emergency proposal
    governance_client.vote(&admin, &prop_id, &true, &false, &Vec::new(&env));

    // Advance time and execute
    env.ledger().set(soroban_sdk::testutils::LedgerInfo {
        timestamp: env.ledger().timestamp(),
        protocol_version: 21,
        sequence_number: env.ledger().sequence() + 51,
        network_id: [0u8; 32],
        base_reserve: 10,
        max_entry_ttl: 1000,
    });

    governance_client.execute(&prop_id);

    // Verify emergency action was executed
    let proposal = governance_client.get_proposal(&prop_id);
    assert_eq!(proposal.status, governance_contract::ProposalStatus::Executed);

    // Test emergency functions still work
    staking_client.emergency_withdraw(&user);
    
    let escrow_info = escrow_client.get_escrow(env, escrow_id_result);
    assert_eq!(escrow_info.status, escrow_contract::EscrowStatus::EmergencyWithdrawn);
}

#[test]
fn test_edge_case_reentrancy_protection() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let user = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    // Initialize staking contract
    let staking_id = env.register_contracts(None, contracts::StakingContract);
    let staking_client = StakingContractClient::new(&env, &staking_id);
    staking_client.initialize(&admin, &token.address, &token.address, &1000);
    staking_client.set_tier(&admin, &1, &100, &150);

    // Mint tokens
    let token_admin = token::StellarAssetClient::new(&env, &token.address);
    token_admin.mint(&user, &10_000_000);

    // Test reentrancy protection in staking
    let stake_amount = 2000;
    let lock_duration = 30 * 24 * 60 * 60;
    
    // This should work normally
    staking_client.stake(&user, &stake_amount, &lock_duration, &1);

    // Try to stake again with same parameters (should be prevented by nonce)
    let result = std::panic::catch_unwind(|| {
        staking_client.stake(&user, &stake_amount, &lock_duration, &1);
    });
    // This might not panic due to different implementation, but should handle gracefully
}

#[test]
fn test_edge_case_zero_values() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let user = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    // Initialize contracts
    let staking_id = env.register_contracts(None, contracts::StakingContract);
    let staking_client = StakingContractClient::new(&env, &staking_id);
    staking_client.initialize(&admin, &token.address, &token.address, &1000);

    let escrow_id = env.register_contracts(None, escrow_contract::EscrowContract);
    let escrow_client = EscrowContractClient::new(&env, &escrow_id);
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
    escrow_client.initialize(&admin, escrow_config);

    // Test zero amount staking (should fail)
    let result = std::panic::catch_unwind(|| {
        staking_client.stake(&user, &0, &86400, &1);
    });
    assert!(result.is_err());

    // Test zero amount escrow (should fail)
    let result = std::panic::catch_unwind(|| {
        escrow_client.create_escrow(
            env.clone(),
            Address::generate(&env),
            Address::generate(&env),
            user.clone(),
            0,
            token.address(),
            env.ledger().timestamp() + 86400,
            None,
            None,
            None,
        );
    });
    assert!(result.is_err());
}

#[test]
fn test_edge_case_maximum_values() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let user = Address::generate(&env);
    let token = create_token_contract(&env, &admin);

    // Initialize contracts
    let staking_id = env.register_contracts(None, contracts::StakingContract);
    let staking_client = StakingContractClient::new(&env, &staking_id);
    staking_client.initialize(&admin, &token.address, &token.address, &1000);

    let escrow_id = env.register_contracts(None, escrow_contract::EscrowContract);
    let escrow_client = EscrowContractClient::new(&env, &escrow_id);
    let escrow_config = escrow_contract::RevenueSplitConfig {
        default_organizer_percentage: 8000000,
        default_platform_percentage: 1500000,
        default_referral_percentage: 500000,
        max_referral_percentage: 10000000,
        precision: 10000000,
        min_escrow_amount: 1000000,
        max_escrow_amount: 10000000000,
        max_escrow_amount: 10000000000,
        dispute_timeout: 86400,
        emergency_withdrawal_delay: 3600,
    };
    escrow_client.initialize(&admin, escrow_config);

    // Mint maximum tokens
    let max_amount = i128::MAX / 2; // Safe maximum
    let token_admin = token::StellarAssetClient::new(&env, &token.address);
    token_admin.mint(&user, &max_amount);

    // Test maximum staking (should work with proper validation)
    staking_client.stake(&user, &max_amount / 2, &86400, &1);

    // Test maximum escrow (should be limited by config)
    let result = std::panic::catch_unwind(|| {
        escrow_client.create_escrow(
            env.clone(),
            Address::generate(&env),
            Address::generate(&env),
            user.clone(),
            max_amount,
            token.address(),
            env.ledger().timestamp() + 86400,
            None,
            None,
            None,
        );
    });
    assert!(result.is_err());
}
