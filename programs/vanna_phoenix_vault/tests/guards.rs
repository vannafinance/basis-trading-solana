//! Share tokens and account checks.

use crate::env::*;
use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::AccountMeta;
use vanna_phoenix_vault::instructions::VaultParams;
use vanna_phoenix_vault::swap::Sell;

#[test]
fn shares_are_plain_tokens_anyone_can_hold_move_and_redeem() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 1_000 * SOL);
    let bob = env.add_user("bob", 10 * SOL);
    env.deposit(Asset::Sol, "alice", 10 * SOL, 0).unwrap();
    let shares = env.shares(Asset::Sol, alice);
    assert_eq!(
        env.share_freeze_authority(Asset::Sol),
        None,
        "nobody can freeze shares"
    );
    assert!(!env.shares_frozen(Asset::Sol, alice));

    // Out in the same slot as in: there is no lock.
    let meta = env.withdraw(Asset::Sol, "alice", shares / 10).unwrap();
    let out = &events::<vanna_phoenix_vault::events::Withdrawn>(&meta)[0];
    eprintln!(
        "alice withdraws 10% at once: {:.4} SOL",
        out.asset_out as f64 / SOL as f64
    );

    // Alice hands shares to Bob, who never deposited; Bob redeems them himself.
    env.create_ata("bob", bob, env.sol.share_mint);
    let gift = env.shares(Asset::Sol, alice) / 2;
    env.transfer_shares(Asset::Sol, "alice", bob, gift).unwrap();
    assert_eq!(env.shares(Asset::Sol, bob), gift);
    let sol_before = env.asset_balance(Asset::Sol, bob);
    let meta = env
        .withdraw(Asset::Sol, "bob", gift)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let out = &events::<vanna_phoenix_vault::events::Withdrawn>(&meta)[0];
    let got = env.asset_balance(Asset::Sol, bob) + meta.fee - sol_before;
    eprintln!(
        "bob redeems {:.2} shares from alice: {:.4} SOL (${:.2})",
        usd(gift),
        got as f64 / SOL as f64,
        usd(out.payout_usd)
    );
    assert_eq!(got, out.asset_out);
    assert_eq!(env.shares(Asset::Sol, bob), 0);
    assert!(
        usd(out.payout_usd) > usd(gift) * 0.99,
        "a share is worth about a dollar"
    );

    // And Alice takes the rest. The last exit from the SOL vault leaves a dollar or two of
    // shares while the vault owes Phoenix its trading fees in USDC (Phoenix wants that covered
    // by SOL at 80%); the keeper's rebalance pays it off.
    let rest = env.shares(Asset::Sol, alice);
    env.withdraw(Asset::Sol, "alice", rest).unwrap();
    assert!(env.shares(Asset::Sol, alice) < rest / 100);
}

#[test]
fn a_vault_only_trades_its_own_market() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 1_000 * SOL);
    let mut ix = env.deposit_ix(Asset::Sol, alice, 10 * SOL, 0, 0);
    let (sol_book, eth_book) = (env.sol.orderbook, env.eth.orderbook);
    for meta in ix.accounts.iter_mut().filter(|m| m.pubkey == sol_book) {
        meta.pubkey = eth_book;
    }
    assert!(failed_with(
        env.send("alice", vec![ix], "wrong book"),
        "ConstraintAddress"
    ));
}

#[test]
fn phoenix_index_accounts_must_be_phoenix_accounts() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 1_000 * SOL);
    let mut ix = env.deposit_ix(Asset::Sol, alice, 10 * SOL, 0, 0);
    let n = ix.accounts.len();
    ix.accounts[n - 2] = AccountMeta::new(Pubkey::new_unique(), false);
    assert!(failed_with(
        env.send("alice", vec![ix], "fake index"),
        "InvalidPhoenixAccount"
    ));
}

#[test]
fn deposits_close_and_the_protocol_pauses() {
    let mut env = Env::new();
    env.add_user("alice", 1_000 * SOL);
    env.update_vault(
        Asset::Sol,
        VaultParams {
            deposits_open: false,
            ..default_params()
        },
    );
    assert!(failed_with(
        env.deposit(Asset::Sol, "alice", SOL, 0),
        "DepositsClosed"
    ));
}

#[test]
fn transaction_sizes_and_compute() {
    let mut env = Env::new();
    env.update_vault(
        Asset::Eth,
        VaultParams {
            margin_bps: 9_000,
            ..default_params()
        },
    );
    env.setup_keeper_accounts();
    env.use_jupiter();
    let alice = env.add_user("alice", 1_000 * SOL);
    env.mint_asset(Asset::Eth, alice, 5 * ETH);
    env.mint_usdc(alice, 17_500 * USDC);
    let budget = solana_compute_budget_interface::ComputeBudgetInstruction::set_compute_unit_limit(
        1_400_000,
    );
    let report = |label: &str,
                  ix: anchor_lang::solana_program::instruction::Instruction,
                  cu: u64| {
        let (accounts, legacy, v0) = transaction_sizes(alice, &[budget.clone(), ix]);
        let over = if legacy > 1_232 { " (over 1,232)" } else { "" };
        eprintln!("{label:<26} {accounts:>2} accounts | legacy {legacy:>4} bytes{over} | with a lookup table {v0:>4} bytes | {cu:>7} CU");
        assert!(v0 <= 1_232, "fits with a lookup table");
    };

    let ix = env.deposit_ix(Asset::Sol, alice, 10 * SOL, 0, 0);
    report(
        "SOL deposit",
        ix,
        env.deposit(Asset::Sol, "alice", 10 * SOL, 0)
            .unwrap()
            .compute_units_consumed,
    );
    let shares = env.shares(Asset::Sol, alice) / 2;
    let ix = env.withdraw_ix(Asset::Sol, alice, shares, 0, 0);
    report(
        "SOL withdraw",
        ix,
        env.withdraw(Asset::Sol, "alice", shares)
            .unwrap()
            .compute_units_consumed,
    );

    // 5 ETH and as much USDC as they are worth at Phoenix's mark: the vault's 1x mix.
    let usdc = (5.0 * env.view(Asset::Eth).mark_usd * USDC as f64) as u64;
    let ix = env.deposit_ix(Asset::Eth, alice, 5 * ETH, usdc, 0);
    report(
        "ETH deposit (ETH + USDC)",
        ix,
        env.deposit(Asset::Eth, "alice", 5 * ETH, usdc)
            .unwrap_or_else(|l| panic!("{}", l.join("\n")))
            .compute_units_consumed,
    );
    let shares = env.shares(Asset::Eth, alice) / 2;
    let ix = env.withdraw_ix(Asset::Eth, alice, shares, 0, 0);
    report(
        "ETH withdraw (ETH + USDC)",
        ix,
        env.withdraw(Asset::Eth, "alice", shares)
            .unwrap()
            .compute_units_consumed,
    );

    // Single-asset deposits carry a real Jupiter route (25 accounts for a direct Orca route).
    env.mint_usdc(alice, 8_000 * USDC);
    let route = env
        .jupiter_route_for(Asset::Eth, Sell::Usdc, 3_500 * USDC)
        .route;
    let ix = env.deposit_ix_with(Asset::Eth, alice, 0, 7_000 * USDC, 0, Some(route));
    let cu = env
        .send("alice", vec![ix.clone()], "routed deposit")
        .unwrap()
        .compute_units_consumed;
    report("ETH deposit, USDC (Jupiter)", ix, cu);
    let route = env
        .jupiter_route_for(Asset::Sol, Sell::Usdc, 1_000 * USDC)
        .route;
    let ix = env.deposit_ix_with(Asset::Sol, alice, 0, 1_000 * USDC, 0, Some(route));
    let cu = env
        .send("alice", vec![ix.clone()], "routed deposit")
        .unwrap()
        .compute_units_consumed;
    report("SOL deposit, USDC (Jupiter)", ix, cu);
    // A stock vault: Token-2022 xStocks over a Raydium CLMM route (32 accounts).
    env.mint_usdc(alice, 10_000 * USDC);
    let route = env
        .jupiter_route_for(Asset::Tsla, Sell::Usdc, 5_000 * USDC)
        .route;
    let ix = env.deposit_ix_with(Asset::Tsla, alice, 0, 10_000 * USDC, 0, Some(route));
    let cu = env
        .send("alice", vec![ix.clone()], "routed deposit")
        .unwrap_or_else(|l| panic!("{}", l.join("\n")))
        .compute_units_consumed;
    report("TSLA deposit, USDC (Jupiter)", ix, cu);
    env.mint_asset(Asset::Tsla, alice, 10 * STOCK);
    env.mint_usdc(alice, 3_900 * USDC);
    let ix = env.deposit_ix(Asset::Tsla, alice, 10 * STOCK, 3_900 * USDC, 0);
    report(
        "TSLA deposit (TSLAx + USDC)",
        ix,
        env.deposit(Asset::Tsla, "alice", 10 * STOCK, 3_900 * USDC)
            .unwrap_or_else(|l| panic!("{}", l.join("\n")))
            .compute_units_consumed,
    );

    env.mint_usdc(env.keeper, 3_500 * USDC);
    let usdc = (env.view(Asset::Eth).mark_usd / 2.0 * USDC as f64) as u64;
    let cu = env
        .rebalance(Asset::Eth, false, ETH / 2, usdc)
        .unwrap()
        .compute_units_consumed;
    eprintln!("{:<26} {cu:>7} CU", "ETH keeper rebalance");
}
