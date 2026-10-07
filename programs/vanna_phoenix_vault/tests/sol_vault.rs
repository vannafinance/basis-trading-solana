//! The SOL vault: native SOL is Phoenix collateral, so all of a USDC deposit buys SOL, which
//! backs a short of the same size. $1,500 in at $150: 10 SOL bought, 10 SOL short, nothing
//! held outside Phoenix.

use crate::env::*;
use vanna_phoenix_vault::events::{Deposited, Withdrawn};

#[test]
fn usdc_in_buys_sol_and_shorts_the_same() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);

    let meta = env
        .deposit(Asset::Sol, "alice", 1_500 * USDC)
        .unwrap_or_else(|logs| panic!("{}", logs.join("\n")));
    eprintln!("deposit: {} compute units", meta.compute_units_consumed);
    let deposited = &events::<Deposited>(&meta)[0];
    let v = env.print(Asset::Sol, "after alice deposits $1,500");

    assert_eq!(deposited.asset_bought, 10 * SOL, "all of it buys SOL");
    assert_eq!(
        v.asset_units,
        10 * SOL,
        "all 10 SOL sit in Phoenix as collateral"
    );
    assert_eq!(
        v.position_lots, -1_000,
        "short exactly 10 SOL (1,000 lots of 0.01)"
    );
    let shares = env.shares(Asset::Sol, alice);
    // The depositor pays their own spread and taker fee.
    assert!(
        v.nav_usd > 1_498.0 && v.nav_usd < 1_500.0,
        "nav {}",
        v.nav_usd
    );
    assert!(
        (usd(shares) - v.nav_usd).abs() < 0.01,
        "first depositor gets one share per dollar"
    );
}

#[test]
fn sol_moving_does_not_move_the_vault_and_everyone_exits_fairly() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    let bob = env.add_user("bob", 10 * SOL);

    env.deposit(Asset::Sol, "alice", 1_500 * USDC).unwrap();
    let start = env.print(Asset::Sol, "alice deposits $1,500 at $150");

    env.move_price(Asset::Sol, 180.0);
    let up = env.print(Asset::Sol, "SOL +20% to $180");
    assert!(
        (up.nav_usd - start.nav_usd).abs() < 0.5,
        "a hedged vault stays flat in USD"
    );

    let meta = env.deposit(Asset::Sol, "bob", 1_800 * USDC).unwrap();
    let bob_in = &events::<Deposited>(&meta)[0];
    let after_bob = env.print(Asset::Sol, "bob deposits $1,800 at $180");
    eprintln!(
        "bob: ${:.2} added, {:.2} shares",
        usd(bob_in.value_usd),
        usd(bob_in.shares)
    );
    assert_eq!(after_bob.position_lots, -2_000, "20 SOL held, 20 SOL short");
    assert!(
        (after_bob.share_price() - up.share_price()).abs() < 1e-5,
        "bob's deposit leaves alice's share price alone"
    );

    env.move_price(Asset::Sol, 120.0);
    let down = env.print(Asset::Sol, "SOL -33% to $120");
    assert!((down.nav_usd - after_bob.nav_usd).abs() < 1.0);

    let meta = env
        .withdraw(Asset::Sol, "alice", env.shares(Asset::Sol, alice))
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let alice_out = &events::<Withdrawn>(&meta)[0];
    env.print(Asset::Sol, "alice withdraws everything");
    eprintln!(
        "alice: put in $1,500, took out ${:.2} USDC at $120 ({:.4} SOL sold, {} compute units)",
        usd(alice_out.usdc_out),
        alice_out.asset_sold as f64 / SOL as f64,
        meta.compute_units_consumed
    );
    assert_eq!(env.usdc(alice), alice_out.usdc_out, "paid in USDC");
    assert!(usd(alice_out.usdc_out) > 1_495.0 && usd(alice_out.usdc_out) < 1_500.0);

    let meta = env
        .withdraw(Asset::Sol, "bob", env.shares(Asset::Sol, bob))
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let bob_out = &events::<Withdrawn>(&meta)[0];
    let end = env.print(Asset::Sol, "bob withdraws everything");
    // SOL fell, so part of the vault's value is the short's profit, in USDC: the last one out
    // takes the SOL that is left, sold, and that USDC.
    eprintln!(
        "bob: put in $1,800, took out ${:.2} USDC at $120 ({:.4} SOL sold)",
        usd(bob_out.usdc_out),
        bob_out.asset_sold as f64 / SOL as f64
    );
    assert_eq!(env.usdc(bob), bob_out.usdc_out);
    assert!(usd(bob_out.usdc_out) > 1_795.0 && usd(bob_out.usdc_out) < 1_800.0);
    assert_eq!(env.shares(Asset::Sol, bob), 0);
    assert_eq!(end.supply, 0);
    assert_eq!(end.position_lots, 0, "the short is fully closed");
    assert!(end.nav_usd < 1.0, "only dust is left: ${}", end.nav_usd);
}

#[test]
fn a_partial_withdrawal_pays_its_share() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    env.deposit(Asset::Sol, "alice", 3_000 * USDC).unwrap();
    let before = env.print(Asset::Sol, "alice deposits $3,000");
    let half = env.shares(Asset::Sol, alice) / 2;

    let meta = env.withdraw(Asset::Sol, "alice", half).unwrap();
    let out = &events::<Withdrawn>(&meta)[0];
    let after = env.print(Asset::Sol, "alice withdraws half her shares");
    eprintln!(
        "half the shares sold {:.4} SOL for ${:.2}",
        out.asset_sold as f64 / SOL as f64,
        usd(out.usdc_out)
    );
    assert!(out.asset_sold > 9_980_000_000 && out.asset_sold < 10 * SOL);
    assert!(usd(out.usdc_out) > 1_497.0 && usd(out.usdc_out) < 1_500.0);
    assert_eq!(
        after.position_lots,
        -(after.asset_units as i64 / 10_000_000),
        "still hedged"
    );
    assert!(
        after.share_price() >= before.share_price() - 1e-6,
        "the leaver's costs stay with the leaver"
    );
}

#[test]
fn the_cap_and_the_slippage_band_hold() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    env.update_vault(
        Asset::Sol,
        vanna_phoenix_vault::instructions::VaultParams {
            max_position_usd: 1_000 * USDC,
            ..sol_params()
        },
    );
    assert!(failed_with(
        env.deposit(Asset::Sol, "alice", 1_500 * USDC),
        "MaxPositionReached"
    ));

    env.update_vault(Asset::Sol, sol_params());
    // The book is far from the mark: the vault refuses to sell 2% below it.
    env.quote(Asset::Sol, 147.0);
    let r = env.deposit(Asset::Sol, "alice", 1_500 * USDC);
    assert!(r.is_err(), "an order outside the band does not fill");
}

#[test]
fn after_a_rally_the_vault_owes_usdc_and_the_keeper_settles_it() {
    let mut env = Env::new();
    env.setup_keeper_accounts();
    let alice = env.add_user("alice", 10 * SOL);
    env.deposit(Asset::Sol, "alice", 1_500 * USDC).unwrap();
    env.print(Asset::Sol, "alice deposits $1,500 at $150");
    env.move_price(Asset::Sol, 200.0);
    env.print(Asset::Sol, "SOL +33% to $200");

    // Closing the short realizes its $500 loss in USDC. Phoenix counts SOL at 80%, so some
    // SOL must stay to cover what the vault owes: alice gets what can leave and keeps shares
    // for the rest.
    let meta = env
        .withdraw(Asset::Sol, "alice", env.shares(Asset::Sol, alice))
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let first = &events::<Withdrawn>(&meta)[0];
    let owed = env.print(Asset::Sol, "alice withdraws everything");
    let kept = env.shares(Asset::Sol, alice);
    eprintln!(
        "alice sold {:.4} SOL for ${:.2}; {:.2} shares stay until the vault's USDC debt is paid",
        first.asset_sold as f64 / SOL as f64,
        usd(first.usdc_out),
        usd(kept)
    );
    let debt = -(owed.collateral_usd + owed.upnl_usd);
    assert!(kept > 0 && debt > 400.0, "the vault owes ${debt:.2}");

    // The keeper buys SOL from the vault for USDC, which pays the debt.
    let sell = (debt / owed.mark_usd * SOL as f64) as u64 + SOL / 100;
    let usdc = (sell as f64 / SOL as f64 * owed.mark_usd * USDC as f64) as u64;
    env.mint_usdc(env.keeper, usdc);
    env.rebalance(Asset::Sol, false, sell, usdc)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    env.print(Asset::Sol, "keeper settles the USDC debt");

    let meta = env
        .withdraw(Asset::Sol, "alice", kept)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let second = &events::<Withdrawn>(&meta)[0];
    env.print(Asset::Sol, "alice takes the rest");
    let total = usd(first.usdc_out) + usd(second.usdc_out);
    eprintln!("alice: put in $1,500, took out ${total:.2} USDC in two steps");
    assert_eq!(env.shares(Asset::Sol, alice), 0);
    assert_eq!(usd(env.usdc(alice)), total);
    assert!(total > 1_495.0 && total < 1_500.0);
}
