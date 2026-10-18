//! Deposits, withdrawals and the share price: the single-asset vault's accounting, over many
//! legs. Users only ever move USDC.

use crate::env::*;
use anchor_lang::solana_program::instruction::AccountMeta;
use vanna_phoenix_multi_vault::events::{Deposited, Withdrawn};
use vanna_phoenix_multi_vault::instructions::VaultParams;

/// An edit to an instruction's leg list.
type LegEdit = Box<dyn Fn(&mut Vec<AccountMeta>)>;

fn expect<T>(result: Result<T, Vec<String>>) -> T {
    result.unwrap_or_else(|logs| panic!("{}", logs.join("\n")))
}

#[test]
fn the_first_dollar_buys_one_share_and_waits_in_the_buffer() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    let meta = expect(env.deposit("alice", 10_000 * USDC));
    let d = &events::<Deposited>(&meta)[0];
    let v = env.print("alice deposits $10,000");
    eprintln!("deposit with 5 legs: {} compute units", meta.compute_units_consumed);
    assert_eq!(d.shares, 10_000 * USDC, "one share per dollar");
    assert_eq!(env.shares(alice), 10_000 * USDC);
    assert_eq!((d.nav_before_usd, d.nav_usd, d.supply), (0, 10_000 * USDC, 10_000 * USDC));
    assert_eq!(v.idle_usd, 10_000.0, "all of it waits for the keeper");
    assert_eq!(env.usdc(alice), 0);
}

#[test]
fn shares_follow_the_nav_of_every_leg_and_everyone_leaves_with_their_part() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    let bob = env.add_user("bob", 10 * SOL);
    expect(env.deposit("alice", 100_000 * USDC));
    env.deploy_everywhere(90_000 * USDC);
    let start = env.print("alice $100,000, $90,000 spread over 5 legs");
    for asset in Asset::ALL {
        let l = start.leg(asset);
        assert!(
            (l.held(asset) - l.short(&env, asset)).abs() <= 0.006,
            "{} is hedged: {} held, {} short",
            asset.symbol(),
            l.held(asset),
            l.short(&env, asset)
        );
    }
    assert!(start.nav_usd > 99_900.0, "the deploys cost little: {}", start.nav_usd);

    // Every market moves its own way; the vault does not.
    for (asset, price) in [
        (Asset::Sol, 180.0),
        (Asset::Eth, 3_000.0),
        (Asset::Btc, 110_000.0),
        (Asset::Tsla, 350.0),
        (Asset::Meta, 800.0),
    ] {
        env.move_price(asset, price);
    }
    let moved = env.print("SOL +20%, ETH -14%, BTC +10%, TSLA -8%, META +7%");
    assert!(
        (moved.nav_usd - start.nav_usd).abs() < start.nav_usd * 0.0005,
        "flat in USD: {} -> {}",
        start.nav_usd,
        moved.nav_usd
    );

    // Bob comes in at the share price now.
    let meta = expect(env.deposit("bob", 50_000 * USDC));
    let d = &events::<Deposited>(&meta)[0];
    let both = env.print("bob deposits $50,000");
    assert!((both.share_price() - moved.share_price()).abs() < 1e-6);
    assert!((usd(d.nav_before_usd) - moved.nav_usd).abs() < 0.01);
    env.deploy_everywhere(45_000 * USDC);
    let deployed = env.print("bob's $45,000 deployed too");

    // A day of funding on SOL and TSLA: longs pay shorts.
    env.set_perp_premium(Asset::Sol, 181.8, 180.0);
    env.set_perp_premium(Asset::Tsla, 353.5, 350.0);
    env.warp_hours(24);
    let paid = env.print("a day of funding on SOL and TSLA");
    assert!(paid.funding_usd > 0.0);
    assert!(paid.share_price() > deployed.share_price());

    // Both leave: the keeper unwinds everything and anyone fulfills their requests.
    let alice_shares = env.shares(alice);
    let bob_shares = env.shares(bob);
    let a = expect(env.request_withdraw("alice", alice_shares));
    let b = expect(env.request_withdraw("bob", bob_shares));
    let queued = env.view();
    assert_eq!(queued.escrow, alice_shares + bob_shares);
    env.unwind_share(1, 1);
    let raised = env.print("keeper unwinds every leg");
    let meta = expect(env.fulfill_withdraw("bob", alice, a));
    let alice_out = &events::<Withdrawn>(&meta)[0];
    let meta = expect(env.fulfill_withdraw("bob", bob, b));
    let bob_out = &events::<Withdrawn>(&meta)[0];
    let end = env.print("both fulfilled");
    let price = raised.share_price();
    eprintln!(
        "alice: $100,000 in, ${:.2} out; bob: $50,000 in, ${:.2} out; share price at exit {price:.6}",
        usd(alice_out.usdc_out),
        usd(bob_out.usdc_out)
    );
    assert_eq!(env.usdc(alice), alice_out.usdc_out);
    assert_eq!(env.usdc(bob), bob_out.usdc_out);
    for (out, shares) in [(alice_out, alice_shares), (bob_out, bob_shares)] {
        let fair = usd(shares) * price;
        assert!((usd(out.usdc_out) - fair).abs() < 0.02, "{} vs {fair}", usd(out.usdc_out));
    }
    assert_eq!(end.supply, 0);
    assert!(end.nav_usd < 0.05, "only dust is left: {}", end.nav_usd);
    for asset in Asset::ALL {
        assert_eq!(end.leg(asset).position_lots, 0, "{} is flat", asset.symbol());
    }
}

#[test]
fn the_buffer_pays_at_once_and_requests_wait_for_the_keeper() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    let bob = env.add_user("bob", 10 * SOL);
    env.add_user("carol", 10 * SOL);
    expect(env.deposit("alice", 60_000 * USDC));
    expect(env.deposit("bob", 40_000 * USDC));
    env.deploy_everywhere(90_000 * USDC);
    let start = env.print("$100,000 in, $90,000 deployed, $10,000 buffer");

    // $5,000 out of the buffer at once.
    let meta = expect(env.withdraw("alice", 5_000 * USDC));
    let out = &events::<Withdrawn>(&meta)[0];
    eprintln!("instant withdrawal: {} compute units", meta.compute_units_consumed);
    assert_eq!(out.request_id, None);
    assert!((usd(out.usdc_out) - 5_000.0 * start.share_price()).abs() < 0.01);
    // Bob's $40,000 is more than the buffer holds.
    let bob_shares = env.shares(bob);
    assert!(failed_with(env.withdraw("bob", bob_shares), "NotEnoughLiquidity"));

    // He requests it; his shares wait in escrow and the buffer is his first.
    let id = expect(env.request_withdraw("bob", bob_shares));
    assert_eq!(env.shares(bob), 0);
    assert!(
        failed_with(env.withdraw("alice", 1_000 * USDC), "NotEnoughLiquidity"),
        "the buffer is reserved for the request"
    );
    // Nobody can fulfill it before the keeper raises the USDC.
    assert!(failed_with(env.fulfill_withdraw("carol", bob, id), "NotEnoughLiquidity"));

    env.unwind_share(1, 2);
    env.print("keeper unwinds half of every leg");
    let meta = expect(env.fulfill_withdraw("carol", bob, id));
    let out = &events::<Withdrawn>(&meta)[0];
    let after = env.print("carol fulfills bob's request");
    eprintln!("fulfilled: ${:.2} for {:.2} shares", usd(out.usdc_out), usd(out.shares));
    assert_eq!(out.request_id, Some(id));
    assert_eq!(env.usdc(bob), out.usdc_out, "the USDC goes to bob");
    assert!(usd(out.usdc_out) > 39_900.0);
    assert_eq!(after.escrow, 0);
    assert!(env.ctx.svm.get_account(&vanna_phoenix_multi_vault::state::request_address(&env.vault.address, id).0).is_none_or(|a| a.lamports == 0), "the request is closed");
    // Alice can use the buffer again.
    expect(env.withdraw("alice", 1_000 * USDC));
    assert!(env.usdc(alice) > 5_990 * USDC);
}

#[test]
fn a_request_can_be_cancelled_by_its_user_or_returned_by_the_keeper() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    env.add_user("mallory", 10 * SOL);
    expect(env.deposit("alice", 10_000 * USDC));
    let id = expect(env.request_withdraw("alice", 4_000 * USDC));
    assert_eq!(env.shares(alice), 6_000 * USDC);
    assert_eq!(env.vault_account().pending_shares, 4_000 * USDC);
    // Nobody else may cancel it.
    assert!(failed_with(env.cancel_withdraw("mallory", alice, id), "Unauthorized"));
    expect(env.cancel_withdraw("alice", alice, id));
    assert_eq!(env.shares(alice), 10_000 * USDC);
    assert_eq!((env.view().escrow, env.vault_account().pending_shares), (0, 0));
    assert!(env.fulfill_withdraw("alice", alice, id).is_err(), "it is gone");
    // The keeper may hand back a request it cannot pay: it no longer reserves the buffer, and
    // alice takes her shares back, losing only her place. Nobody else can return one.
    let id = expect(env.request_withdraw("alice", 2_000 * USDC));
    assert!(failed_with(env.return_withdraw("mallory", id), "Unauthorized"));
    let keeper = env.keeper_seed();
    expect(env.return_withdraw(keeper, id));
    assert_eq!(env.vault_account().pending_shares, 0);
    assert!(failed_with(env.fulfill_withdraw(keeper, alice, id), "RequestReturned"));
    assert!(failed_with(env.return_withdraw(keeper, id), "RequestReturned"));
    assert!(failed_with(env.cancel_withdraw(keeper, alice, id), "Unauthorized"));
    expect(env.cancel_withdraw("alice", alice, id));
    assert_eq!(env.shares(alice), 10_000 * USDC);
}

#[test]
fn shares_sent_straight_to_the_escrow_reserve_nothing() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    env.add_user("mallory", 10 * SOL);
    let mallory = env.ctx.signer_pubkey("mallory");
    expect(env.deposit("alice", 10_000 * USDC));
    expect(env.deposit("mallory", 5_000 * USDC));
    // Mallory parks a third of the supply in the escrow without a request: the buffer is not
    // reserved for it, and alice can still leave at once.
    let escrow_owner = env.vault.authority;
    expect(env.transfer_shares("mallory", escrow_owner, 5_000 * USDC));
    assert_eq!(env.view().escrow, 5_000 * USDC);
    assert_eq!(env.vault_account().pending_shares, 0);
    let meta = expect(env.withdraw("alice", 10_000 * USDC));
    assert_eq!(events::<Withdrawn>(&meta)[0].usdc_out, 10_000 * USDC);
    assert_eq!(env.shares(mallory), 0);
    assert_eq!(env.usdc(alice), 10_000 * USDC);
}

#[test]
fn fees_stay_in_the_vault_for_those_who_stay() {
    let mut env = Env::new();
    expect(env.update_vault(VaultParams {
        entry_fee_bps: 20,
        exit_fee_bps: 30,
        ..default_params()
    }));
    let alice = env.add_user("alice", 10 * SOL);
    env.add_user("bob", 10 * SOL);
    expect(env.deposit("alice", 10_000 * USDC));
    assert_eq!(env.shares(alice), 9_980 * USDC, "0.2% on the way in");
    let before = env.view();
    let meta = expect(env.deposit("bob", 10_000 * USDC));
    let bob_shares = events::<Deposited>(&meta)[0].shares;
    let mid = env.view();
    assert!(mid.share_price() > before.share_price(), "bob's fee lifts alice's shares");
    let meta = expect(env.withdraw("bob", bob_shares));
    let out = &events::<Withdrawn>(&meta)[0];
    assert_eq!(out.fee_usd, out.payout_usd - out.usdc_out);
    // Bob paid 0.2% in and 0.3% out, and got back the part of his own fees his shares held.
    let lost = 10_000.0 - usd(out.usdc_out);
    eprintln!("bob: $10,000 in, ${:.2} out (${lost:.2} of fees stayed)", usd(out.usdc_out));
    assert!(lost > 25.0 && lost < 50.0, "{lost}");
    let after = env.view();
    eprintln!(
        "alice's share price: {:.6} -> {:.6} after bob came and went",
        before.share_price(),
        after.share_price()
    );
    assert!(after.share_price() > mid.share_price());
}

#[test]
fn deposits_respect_their_caps_and_withdrawals_survive_a_pause() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    expect(env.update_vault(VaultParams {
        max_deposit_usd: 5_000 * USDC,
        max_tvl_usd: 8_000 * USDC,
        ..default_params()
    }));
    assert!(failed_with(env.deposit("alice", 6_000 * USDC), "DepositTooLarge"));
    expect(env.deposit("alice", 5_000 * USDC));
    assert!(failed_with(env.deposit("alice", 4_000 * USDC), "MaxTvlReached"));
    expect(env.deposit("alice", 3_000 * USDC));

    let keeper = env.keeper_seed();
    expect(env.set_deposits_open(keeper, false));
    assert!(failed_with(env.deposit("alice", USDC), "DepositsClosed"));
    assert!(failed_with(env.set_deposits_open("alice", true), "Unauthorized"));
    expect(env.set_deposits_open(keeper, true));

    let keeper_key = env.keeper;
    expect(env.set_config(keeper_key, JUPITER, true));
    assert!(failed_with(env.deposit("alice", USDC), "Paused"));
    let shares = env.shares(alice);
    expect(env.withdraw("alice", shares / 2));
    let id = expect(env.request_withdraw("alice", shares / 4));
    expect(env.fulfill_withdraw("alice", alice, id));
}

#[test]
fn every_leg_must_be_passed_in_order() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    env.mint_usdc(alice, 3_000 * USDC);
    expect(env.deposit("alice", 1_000 * USDC));
    env.deploy_hedged(Asset::Eth, 600 * USDC);

    let full = env.deposit_ix(alice, 1_000 * USDC, 0);
    let head = full.accounts.len() - env.leg_metas().len();
    let eth = env.leg(Asset::Eth).clone();
    let tries: Vec<(&str, LegEdit)> = vec![
        // Leaving out the ETH leg would value the vault without its ETH: shares too cheap.
        ("a leg left out", Box::new(move |metas: &mut Vec<AccountMeta>| {
            let at = metas.iter().position(|m| m.pubkey == eth.address).unwrap();
            metas.drain(at..at + 2);
        })),
        ("the legs out of order", Box::new(|metas: &mut Vec<AccountMeta>| metas.swap(0, 1))),
        ("another account as the ETH holding", {
            let eth = env.leg(Asset::Eth).clone();
            let btc = env.leg(Asset::Btc).holding.unwrap();
            Box::new(move |metas: &mut Vec<AccountMeta>| {
                let at = metas.iter().position(|m| Some(m.pubkey) == eth.holding).unwrap();
                metas[at].pubkey = btc;
            })
        }),
        ("no legs at all", Box::new(|metas: &mut Vec<AccountMeta>| metas.clear())),
    ];
    for (label, edit) in tries {
        let mut ix = full.clone();
        let mut legs = ix.accounts.split_off(head);
        edit(&mut legs);
        ix.accounts.extend(legs);
        assert!(
            failed_with(env.send("alice", vec![ix], label), "InvalidLegAccounts"),
            "{label} is refused"
        );
    }
    expect(env.send("alice", vec![full], "every leg"));
}

#[test]
fn a_round_trip_in_a_tiny_vault_takes_nothing_and_inflation_fails() {
    let mut env = Env::new();
    let dot = env.add_user("dot", 10 * SOL);
    env.add_user("eve", 10 * SOL);
    let eve = env.ctx.signer_pubkey("eve");
    // Dot holds one share of a vault worth $51: $1 and a $50 donation of ETH to its leg.
    expect(env.deposit("dot", USDC));
    env.donate_asset(Asset::Eth, "dot", ETH / 70);
    let dot_before = env.view().nav_usd;
    // Eve deposits $70,000 and leaves at once: she takes nothing from dot.
    expect(env.deposit("eve", 70_000 * USDC));
    let shares = env.shares(eve);
    expect(env.withdraw("eve", shares));
    let dot_after = env.view().nav_usd;
    eprintln!("dot's holding: ${dot_before:.4} before eve, ${dot_after:.4} after; eve got ${:.2}", usd(env.usdc(eve)));
    assert!(dot_after >= dot_before - 0.000_01);
    assert!(env.usdc(eve) <= 70_000 * USDC);
    assert_eq!(env.shares(dot), USDC);
}

#[test]
fn a_vault_that_owes_more_than_it_holds_takes_no_deposits() {
    let mut env = Env::new();
    env.add_user("bo", 10 * SOL);
    expect(env.deposit("bo", 1_500 * USDC));
    env.deploy_hedged(Asset::Sol, 1_500 * USDC);
    // The trader account loses its SOL (as in a liquidation): the vault's equity is negative.
    let trader = env.vault.trader;
    let mut account = env.ctx.svm.get_account(&trader).unwrap();
    account.lamports = env.ctx.svm.minimum_balance_for_rent_exemption(account.data.len());
    env.ctx.svm.set_account(trader, account).unwrap();
    assert!(failed_with(env.deposit("bo", 1_500 * USDC), "VaultInsolvent"));
}

#[test]
fn shares_are_plain_tokens_anyone_can_hold_and_redeem() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    let bob = env.add_user("bob", 10 * SOL);
    expect(env.deposit("alice", 2_000 * USDC));
    assert_eq!(env.share_freeze_authority(), None, "nobody can freeze shares");
    expect(env.transfer_shares("alice", bob, 500 * USDC));
    let meta = expect(env.withdraw("bob", 500 * USDC));
    assert_eq!(events::<Withdrawn>(&meta)[0].usdc_out, 500 * USDC);
    assert_eq!(env.usdc(bob), 500 * USDC);
    assert_eq!(env.shares(alice), 1_500 * USDC);
}
