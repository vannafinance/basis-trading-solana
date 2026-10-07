//! Share tokens and account checks.

use crate::env::*;
use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::AccountMeta;
use vanna_phoenix_vault::events::Withdrawn;
use vanna_phoenix_vault::instructions::VaultParams;

fn expect<T>(result: Result<T, Vec<String>>) -> T {
    result.unwrap_or_else(|logs| panic!("{}", logs.join("\n")))
}

#[test]
fn shares_are_plain_tokens_anyone_can_hold_move_and_redeem() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    let bob = env.add_user("bob", 10 * SOL);
    env.deposit(Asset::Sol, "alice", 1_500 * USDC).unwrap();
    let shares = env.shares(Asset::Sol, alice);
    assert_eq!(
        env.share_freeze_authority(Asset::Sol),
        None,
        "nobody can freeze shares"
    );
    assert!(!env.shares_frozen(Asset::Sol, alice));

    // Out in the same slot as in: there is no lock.
    let meta = env.withdraw(Asset::Sol, "alice", shares / 10).unwrap();
    let out = &events::<Withdrawn>(&meta)[0];
    eprintln!("alice withdraws 10% at once: ${:.2}", usd(out.usdc_out));

    // Alice hands shares to Bob, who never deposited; Bob redeems them himself.
    env.create_ata("bob", bob, env.sol.share_mint);
    let gift = env.shares(Asset::Sol, alice) / 2;
    env.transfer_shares(Asset::Sol, "alice", bob, gift).unwrap();
    assert_eq!(env.shares(Asset::Sol, bob), gift);
    let meta = expect(env.withdraw(Asset::Sol, "bob", gift));
    let out = &events::<Withdrawn>(&meta)[0];
    eprintln!(
        "bob redeems {:.2} shares from alice: ${:.2}",
        usd(gift),
        usd(out.usdc_out)
    );
    assert_eq!(env.usdc(bob), out.usdc_out);
    assert_eq!(env.shares(Asset::Sol, bob), 0);
    assert!(
        usd(out.usdc_out) > usd(gift) * 0.99,
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
    let alice = env.add_user("alice", 10 * SOL);
    let mut ix = env.deposit_ix(Asset::Sol, alice, 1_500 * USDC, 0);
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
    let alice = env.add_user("alice", 10 * SOL);
    env.mint_usdc(alice, 1_500 * USDC);
    let mut ix = env.deposit_ix(Asset::Sol, alice, 1_500 * USDC, 0);
    let gti = env.gti[0];
    let at = ix.accounts.iter().position(|m| m.pubkey == gti).unwrap();
    ix.accounts[at] = AccountMeta::new(Pubkey::new_unique(), false);
    assert!(failed_with(
        env.send("alice", vec![ix], "fake index"),
        "InvalidPhoenixAccount"
    ));
}

#[test]
fn deposits_close_and_the_protocol_pauses() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    env.update_vault(
        Asset::Sol,
        VaultParams {
            deposits_open: false,
            ..default_params()
        },
    );
    assert!(failed_with(
        env.deposit(Asset::Sol, "alice", 150 * USDC),
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
    let alice = env.add_user("alice", 10 * SOL);
    env.mint_usdc(alice, 18_000 * USDC);
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

    // Every deposit and withdrawal carries a real Jupiter route: 25 accounts for a direct Orca
    // route (SOL, ETH), 32 for Raydium CLMM (xStocks, Token-2022).
    for (asset, usdc, label) in [
        (Asset::Sol, 1_000, "SOL"),
        (Asset::Eth, 7_000, "ETH"),
        (Asset::Tsla, 10_000, "TSLA"),
    ] {
        let ix = env.deposit_ix(asset, alice, usdc * USDC, 0);
        let cu = expect(env.send("alice", vec![ix.clone()], "deposit")).compute_units_consumed;
        report(&format!("{label} deposit (Jupiter)"), ix, cu);
        let shares = env.shares(asset, alice) / 2;
        let ix = env.withdraw_ix(asset, alice, shares, 0);
        let cu = expect(env.send("alice", vec![ix.clone()], "withdraw")).compute_units_consumed;
        report(&format!("{label} withdraw (Jupiter)"), ix, cu);
    }

    env.mint_usdc(env.keeper, 3_500 * USDC);
    let usdc = (env.view(Asset::Eth).mark_usd / 2.0 * USDC as f64) as u64;
    let cu = expect(env.rebalance(Asset::Eth, false, ETH / 2, usdc)).compute_units_consumed;
    eprintln!("{:<26} {cu:>7} CU", "ETH keeper rebalance");
}
