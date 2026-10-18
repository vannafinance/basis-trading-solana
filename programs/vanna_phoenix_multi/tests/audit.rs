//! Regression tests for the 2026-10-16 audit (AUDIT.md): each one is the finding's scenario,
//! which the program now refuses or survives.

use crate::env::*;
use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_spl::associated_token::get_associated_token_address;
use vanna_phoenix_multi_vault::events::WithdrawCancelled;
use vanna_phoenix_multi_vault::instructions::{LegParams, VaultParams};
use vanna_phoenix_multi_vault::swap::Sell;

fn expect<T>(result: Result<T, Vec<String>>) -> T {
    result.unwrap_or_else(|logs| panic!("{}", logs.join("\n")))
}

/// SPL Token `SetAuthority(AccountOwner)`: `owner_seed` gives `account` to `new_owner`.
fn give_away(env: &mut Env, owner_seed: &str, account: Pubkey, new_owner: Pubkey) {
    let owner = env.ctx.signer_pubkey(owner_seed);
    let mut data = vec![6, 2, 1];
    data.extend_from_slice(new_owner.as_ref());
    let ix = Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![AccountMeta::new(account, false), AccountMeta::new_readonly(owner, true)],
        data,
    };
    expect(env.send(owner_seed, vec![ix], "set owner"));
}

/// The issuer breaks a stock token's dividend multiplier (the vault cannot read it).
fn break_multiplier(env: &mut Env, mint: Pubkey) {
    let data = env.ctx.account_data(&mint);
    let mut at = 166;
    while u16::from_le_bytes([data[at], data[at + 1]]) != 25 {
        at += 4 + u16::from_le_bytes([data[at + 2], data[at + 3]]) as usize;
    }
    let at = at + 4;
    let mut account = env.ctx.svm.get_account(&mint).unwrap();
    account.data[at + 32..at + 40].copy_from_slice(&(-1.0f64).to_le_bytes());
    account.data[at + 48..at + 56].copy_from_slice(&(-1.0f64).to_le_bytes());
    env.ctx.svm.set_account(mint, account).unwrap();
}

fn inactive() -> LegParams {
    LegParams {
        active: false,
        ..open_leg()
    }
}

/// M-1: a request could be fulfilled by anyone the moment the buffer held its payout, so a
/// later request (fulfilled by its own user) took the USDC the keeper raised for an older one.
#[test]
fn a_request_cannot_jump_the_queue_for_usdc_raised_for_an_older_one() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    let bob = env.add_user("bob", 10 * SOL);
    expect(env.deposit("alice", 10_000 * USDC));
    expect(env.deposit("bob", 10_000 * USDC));
    env.deploy_hedged(Asset::Eth, 18_000 * USDC);
    let a = expect(env.request_withdraw("alice", 5_000 * USDC));
    // The keeper raises Alice's $5,000: the buffer holds about $8,000.
    let units = env.view().leg(Asset::Eth).units;
    expect(env.unwind(Asset::Eth, units / 3));
    expect(env.move_margin(false, 3_000 * USDC));
    let b = expect(env.request_withdraw("bob", 5_000 * USDC));
    assert!(failed_with(env.fulfill_withdraw("bob", bob, b), "NotEnoughLiquidity"));
    assert!(failed_with(env.fulfill_withdraw("alice", bob, b), "NotEnoughLiquidity"));
    let keeper = env.keeper_seed();
    expect(env.fulfill_withdraw(keeper, alice, a));
    assert!(env.usdc(alice) > 4_990 * USDC);
    // Once the buffer holds Bob's too, anyone may pay it.
    let units = env.view().leg(Asset::Eth).units;
    expect(env.unwind(Asset::Eth, units));
    let free = env.view_margin().withdrawable_collateral_quote_lots;
    expect(env.move_margin(false, free));
    expect(env.fulfill_withdraw("bob", bob, b));
    assert!(env.usdc(bob) > 4_990 * USDC);
}

/// M-2: a user who gave their USDC and share accounts away made their request unpayable and
/// uncancellable, holding the buffer forever. The keeper now returns it without moving tokens.
#[test]
fn a_request_whose_user_gave_their_accounts_away_is_returned() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    let mallory = env.add_user("mallory", 10 * SOL);
    let accomplice = env.add_user("accomplice", 10 * SOL);
    expect(env.deposit("alice", 5_000 * USDC));
    expect(env.deposit("mallory", 5_000 * USDC));
    env.deploy_hedged(Asset::Eth, 4_000 * USDC); // $6,000 left in the buffer
    let id = expect(env.request_withdraw("mallory", 5_000 * USDC));
    let usdc = get_associated_token_address(&mallory, &env.usdc_mint);
    let shares = get_associated_token_address(&mallory, &env.vault.share_mint);
    give_away(&mut env, "mallory", usdc, accomplice);
    give_away(&mut env, "mallory", shares, accomplice);
    let keeper = env.keeper_seed();
    assert!(env.fulfill_withdraw(keeper, mallory, id).is_err());
    assert!(failed_with(env.withdraw("alice", 5_000 * USDC), "NotEnoughLiquidity"), "held for Mallory");
    expect(env.return_withdraw(keeper, id));
    assert_eq!(env.vault_account().pending_shares, 0);
    expect(env.withdraw("alice", 5_000 * USDC));
    assert!(env.usdc(alice) >= 4_990 * USDC);
    // Mallory's shares wait in the escrow until she has a share account of her own again.
    give_away(&mut env, "accomplice", shares, mallory);
    expect(env.cancel_withdraw("mallory", mallory, id));
    assert_eq!(env.shares(mallory), 5_000 * USDC, "a returned request comes back whole");
}

/// L-2: fulfilling a request created its user's USDC account at the cranker's cost, which a user
/// could close and re-request to drain the keeper's SOL. It is now never created for them.
#[test]
fn a_cranker_never_pays_for_its_users_accounts() {
    let mut env = Env::new();
    let bob = env.add_user("bob", 10 * SOL);
    expect(env.deposit("bob", 2_000 * USDC));
    let id = expect(env.request_withdraw("bob", 1_000 * USDC));
    let usdc = get_associated_token_address(&bob, &env.usdc_mint);
    let close = Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![
            AccountMeta::new(usdc, false),
            AccountMeta::new(bob, false),
            AccountMeta::new_readonly(bob, true),
        ],
        data: vec![9],
    };
    expect(env.send("bob", vec![close], "close usdc"));
    let keeper = env.keeper;
    let lamports = env.ctx.svm.get_balance(&keeper).unwrap();
    let seed = env.keeper_seed();
    assert!(env.fulfill_withdraw(seed, bob, id).is_err());
    let spent = lamports - env.ctx.svm.get_balance(&keeper).unwrap();
    assert!(spent < 100_000, "only the fee: {spent} lamports");
    expect(env.return_withdraw(seed, id));
}

/// M-3: retiring valued every leg, the leg itself included, so a flat leg whose token broke
/// could not be retired, and nothing else worked either. Two broken legs retire one by one.
#[test]
fn flat_legs_whose_tokens_broke_can_still_be_retired() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", 10_000 * USDC));
    break_multiplier(&mut env, META_MINT);
    break_multiplier(&mut env, TSLA_MINT);
    assert!(failed_with(env.deposit("alice", 1_000 * USDC), "InvalidMint"));
    assert!(failed_with(env.retire_leg(Asset::Meta), "RetireActiveLeg"));
    expect(env.update_leg(Asset::Meta, inactive()));
    expect(env.update_leg(Asset::Tsla, inactive()));
    expect(env.retire_leg_holding(Asset::Meta, 0));
    expect(env.retire_leg_holding(Asset::Tsla, 0));
    expect(env.deposit("alice", 1_000 * USDC));
    expect(env.withdraw("alice", 1_000 * USDC));
}

/// F-4: a $1 donation stopped a retirement. Dust is now up to $100 at the mark, and the admin
/// names how much of the asset it expects.
#[test]
fn a_small_donation_cannot_stop_a_retirement() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    env.add_user("griefer", 10 * SOL);
    expect(env.deposit("alice", 10_000 * USDC));
    // 0.2 TSLAx is about $76; 0.2 METAx about $150.
    env.donate_asset(Asset::Tsla, "griefer", STOCK / 5);
    env.donate_asset(Asset::Meta, "griefer", STOCK / 5);
    assert!(failed_with(env.retire_leg_holding(Asset::Tsla, STOCK / 10), "LegNotFlat"));
    expect(env.retire_leg_holding(Asset::Tsla, STOCK));
    assert!(failed_with(env.retire_leg(Asset::Meta), "LegNotFlat"));
}

/// F-1: a stock token whose multiplier the vault cannot read is refused as a leg, rather than
/// stopping every deposit and withdrawal once added.
#[test]
fn a_token_with_an_unreadable_multiplier_cannot_be_added() {
    let mut env = Env::with_first_legs(3);
    break_multiplier(&mut env, TSLA_MINT);
    assert!(failed_with(env.add_leg(Asset::Tsla, open_leg()), "InvalidMint"));
}

/// F-2: in a multiplier window the keeper's sale was checked at the current multiplier, so a
/// key could sell tokens about to be worth three times as much at a third of their worth.
#[test]
fn a_sale_in_a_multiplier_window_must_fetch_the_higher_value() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", 30_000 * USDC));
    env.deploy_hedged(Asset::Tsla, 20_000 * USDC);
    let now = env.now();
    let m = env.multiplier(Asset::Tsla);
    env.set_multiplier(Asset::Tsla, m, m * 3.0, now + 3_600);
    let sell = env.view().leg(Asset::Tsla).units / 2;
    let price = env.token_price(Asset::Tsla) as u128;
    let at_mark = (sell as u128 * price / STOCK as u128) as u64;
    let keeper = env.keeper;
    let seed = env.keeper_seed();
    let cheap = env.mock_route(Asset::Tsla, Sell::Asset, sell, at_mark);
    let ix = env.unwind_ix(keeper, Asset::Tsla, sell, cheap);
    assert!(failed_with(env.send(seed, vec![ix], "unwind"), "SwapPriceTooLow"));
    let fair = env.mock_route(Asset::Tsla, Sell::Asset, sell, at_mark * 3);
    let ix = env.unwind_ix(keeper, Asset::Tsla, sell, fair);
    expect(env.send(seed, vec![ix], "unwind"));
    // Nor does the hedge follow a multiplier Phoenix may not agree with yet.
    assert!(failed_with(env.rehedge(Asset::Tsla), "MultiplierChanging"));
}

/// L-1: anyone could fulfill someone else's request inside a multiplier window, at the low
/// price withdrawals get there. Only the user or the keeper now picks that moment.
#[test]
fn only_the_user_or_the_keeper_fulfills_in_a_multiplier_window() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    let bob = env.add_user("bob", 10 * SOL);
    env.add_user("carol", 10 * SOL);
    expect(env.deposit("alice", 20_000 * USDC));
    expect(env.deposit("bob", 5_000 * USDC));
    env.deploy_hedged(Asset::Tsla, 10_000 * USDC);
    let id = expect(env.request_withdraw("bob", 1_000 * USDC));
    let now = env.now();
    let m = env.multiplier(Asset::Tsla);
    env.set_multiplier(Asset::Tsla, m, m * 1.02, now + 3_600);
    assert!(failed_with(env.fulfill_withdraw("carol", bob, id), "MultiplierChanging"));
    expect(env.fulfill_withdraw("bob", bob, id));
}

/// L-3: cancelling was free, so requesting and cancelling made the keeper trade at everyone's
/// cost. A cancel now costs the exit fee; a request the keeper returned comes back whole.
#[test]
fn a_cancel_costs_the_exit_fee_and_a_returned_request_does_not() {
    let mut env = Env::new();
    expect(env.update_vault(VaultParams {
        exit_fee_bps: 50,
        ..default_params()
    }));
    let alice = env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", 10_000 * USDC));
    let id = expect(env.request_withdraw("alice", 1_000 * USDC));
    let meta = expect(env.cancel_withdraw("alice", alice, id));
    let e = &events::<WithdrawCancelled>(&meta)[0];
    assert_eq!((e.shares, e.fee_shares), (995 * USDC, 5 * USDC));
    assert_eq!(env.shares(alice), 9_995 * USDC);
    assert_eq!(env.view().supply, 9_995 * USDC, "the fee shares are burned");
    let id = expect(env.request_withdraw("alice", 1_000 * USDC));
    let keeper = env.keeper_seed();
    expect(env.return_withdraw(keeper, id));
    expect(env.cancel_withdraw("alice", alice, id));
    assert_eq!(env.shares(alice), 9_995 * USDC);
}

/// Bot audit F2: after a SOL rally the vault owes Phoenix USDC, and Phoenix will not let the SOL
/// go while it does; with every idle dollar reserved for requests nothing could be posted, so
/// nothing could be paid. Reserved USDC may now cover the deficit, and no more.
#[test]
fn reserved_usdc_may_cover_what_the_vault_owes_phoenix() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", 40_000 * USDC));
    expect(env.deploy(Asset::Sol, 37_000 * USDC));
    env.move_price(Asset::Sol, 160.0);
    let owed = -env.view().collateral_usd - env.view().upnl_usd;
    assert!(owed > 2_000.0, "Phoenix is owed ${owed:.2}");
    let shares = env.shares(alice);
    expect(env.request_withdraw("alice", shares));
    assert!(failed_with(env.move_margin(true, 3_000 * USDC), "BufferTooThin"), "more than it owes");
    expect(env.move_margin(true, 2_000 * USDC));
}
