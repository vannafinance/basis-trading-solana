//! Money moving: between legs as funding moves, in and out with several users, and which leg a
//! withdrawal is paid from. Every scenario runs the keeper end to end against the vault and
//! Phoenix, and every move it sends must succeed.

use anchor_lang::prelude::Pubkey;
use vanna_keeper::{Action, Keeper, Observation, Policy};
use vanna_phoenix_multi_vault::instructions::{LegParams, VaultParams};

use crate::env::*;
use crate::scenarios::*;
use crate::sim::*;

/// The test policy with no per-leg share of the NAV: the leg caps and open interest decide.
fn wide() -> Policy {
    Policy {
        max_leg_nav_share_bps: 10_000,
        ..policy()
    }
}

/// Funding for the given markets; every other leg's market pays nothing.
fn only(k: &mut Keeper<Sim>, rates: &[(Asset, f64)]) {
    let all: Vec<(Asset, f64)> = Asset::ALL
        .into_iter()
        .map(|a| (a, rates.iter().find(|r| r.0 == a).map_or(0.0, |r| r.1)))
        .collect();
    set_funding(k, &all);
}

/// Caps a leg's short at `usd`: the room the keeper sees in it.
fn cap(k: &mut Keeper<Sim>, asset: Asset, usd: u64) {
    expect(k.backend.env.update_leg(
        asset,
        LegParams {
            max_notional_usd: usd * USDC,
            ..open_leg()
        },
    ));
}

fn value(obs: &Observation, asset: Asset) -> f64 {
    dollars(obs.value(asset.index()))
}

fn short(obs: &Observation, asset: Asset) -> f64 {
    dollars(obs.notional(asset.index()))
}

/// Where in `moves` the keeper unwound `asset`.
fn unwinds(moves: &[Action], asset: Asset) -> Vec<usize> {
    moves
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a, Action::Unwind { leg, .. } if *leg == leg_of(asset)))
        .map(|(i, _)| i)
        .collect()
}

fn fulfilled_at(moves: &[Action], request: u64) -> Option<usize> {
    moves
        .iter()
        .position(|a| matches!(a, Action::FulfillWithdraw { id, .. } if *id == request))
}

fn deposit(k: &mut Keeper<Sim>, name: &str, usd: u64) -> Pubkey {
    let user = k.backend.env.add_user(name, 10 * SOL);
    expect(k.backend.env.deposit(name, usd * USDC));
    user
}

fn observe(k: &mut Keeper<Sim>, label: &str) -> Observation {
    k.backend.env.print(label);
    let obs = k.observe().unwrap();
    assert_hedged(&obs);
    obs
}

/// Scenario 1: Funding leadership moves SOL → ETH → TSLA. Each new leader is paid for by selling the
/// leg that pays least at that moment: ETH out of SOL; then TSLA, again out of SOL, not ETH.
#[test]
fn funds_move_from_sol_to_eth_to_tsla_as_funding_moves() {
    let (mut k, _) = funded(60_000, wide());
    only(&mut k, &[(Asset::Sol, 25.0)]);
    settle(&mut k);
    let s1 = observe(&mut k, "1: all in SOL");
    assert!(value(&s1, Asset::Sol) > 58_000.0);

    cap(&mut k, Asset::Eth, 20_000);
    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Eth, 90.0)]);
    let moves = settle(&mut k);
    let s2 = observe(&mut k, "2: ETH pays 45% on capital");
    assert!(!unwinds(&moves, Asset::Sol).is_empty(), "{moves:?}");
    assert!((value(&s1, Asset::Sol) - value(&s2, Asset::Sol) - 39_600.0).abs() < 200.0);
    assert!(short(&s2, Asset::Eth) > 19_500.0);

    cap(&mut k, Asset::Tsla, 10_000);
    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Eth, 90.0), (Asset::Tsla, 150.0)]);
    let moves = settle(&mut k);
    let s3 = observe(&mut k, "3: TSLA pays 75% on capital");
    // SOL (25%) pays less than ETH (45%): TSLA is bought with SOL, ETH is kept.
    assert!(!unwinds(&moves, Asset::Sol).is_empty(), "{moves:?}");
    assert!(unwinds(&moves, Asset::Eth).is_empty(), "{moves:?}");
    assert!((short(&s3, Asset::Eth) - short(&s2, Asset::Eth)).abs() < 50.0);
    assert!(value(&s3, Asset::Sol) < 500.0, "{}", s3.summary());
    assert!(short(&s3, Asset::Tsla) > 9_000.0, "{}", s3.summary());
    assert!(dollars(s3.nav()) > 60_000.0 * 0.99);
}

/// Scenario 2: A new market opens up paying the most. Its room is filled from the legs that pay least,
/// in that order (TSLA, then ETH); the best leg (SOL) is not touched.
#[test]
fn a_new_best_market_is_filled_from_the_worst_legs_first() {
    let (mut k, _) = funded(100_000, wide());
    k.backend.env.outside_trade(Asset::Meta, true, 100_000); // 100 META of open interest
    cap(&mut k, Asset::Sol, 30_000);
    cap(&mut k, Asset::Eth, 20_000);
    cap(&mut k, Asset::Tsla, 15_000);
    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Eth, 40.0), (Asset::Tsla, 30.0)]);
    settle(&mut k);
    let before = observe(&mut k, "SOL 25%, ETH 20%, TSLA 15% on capital");
    for asset in [Asset::Sol, Asset::Eth, Asset::Tsla] {
        assert!(value(&before, asset) > 14_000.0, "{}", before.summary());
    }

    cap(&mut k, Asset::Meta, 20_000);
    only(
        &mut k,
        &[(Asset::Sol, 25.0), (Asset::Eth, 40.0), (Asset::Tsla, 30.0), (Asset::Meta, 120.0)],
    );
    let moves = settle(&mut k);
    let after = observe(&mut k, "META pays 60% on capital");
    let (tsla, eth) = (unwinds(&moves, Asset::Tsla), unwinds(&moves, Asset::Eth));
    assert!(!tsla.is_empty() && !eth.is_empty(), "{moves:?}");
    assert!(tsla[0] < eth[0], "the lowest-paying leg goes first: {moves:?}");
    assert!(unwinds(&moves, Asset::Sol).is_empty(), "{moves:?}");
    assert!(value(&after, Asset::Tsla) < 500.0);
    assert!((value(&after, Asset::Sol) - value(&before, Asset::Sol)).abs() < 1.0);
    assert!(short(&after, Asset::Meta) > 19_000.0, "{}", after.summary());
    assert!(dollars(after.nav()) > dollars(before.nav()) * 0.995);
}

/// Scenario 3: A deposit lands just as a better leg appears: the new USDC fills it first, so the keeper
/// sells half as much SOL as it would have.
#[test]
fn new_deposits_fill_a_better_leg_before_any_rotation_sells() {
    let (mut k, _) = funded(60_000, wide());
    only(&mut k, &[(Asset::Sol, 25.0)]);
    settle(&mut k);
    let start = k.observe().unwrap();
    cap(&mut k, Asset::Eth, 20_000);
    deposit(&mut k, "bob", 20_000);
    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Eth, 90.0)]);
    let moves = settle(&mut k);
    let end = observe(&mut k, "Bob's $20,000 and SOL into ETH");
    let first_deploy = moves
        .iter()
        .position(|a| matches!(a, Action::Deploy { leg, .. } if *leg == leg_of(Asset::Eth)))
        .unwrap();
    assert!(first_deploy < unwinds(&moves, Asset::Sol)[0], "Bob's USDC first: {moves:?}");
    let sold = value(&start, Asset::Sol) - value(&end, Asset::Sol);
    assert!((17_000.0..22_000.0).contains(&sold), "sold ${sold:.2} of SOL");
    assert!(short(&end, Asset::Eth) > 19_500.0);
}

/// Alice and Bob in a vault holding SOL ($30k), ETH ($20k) and TSLA ($15k), paying 25%, 20%
/// and 15% on capital.
fn three_legs(alice: u64, others: &[(&str, u64)]) -> (Keeper<Sim>, Pubkey) {
    let (mut k, a) = funded(alice, wide());
    for (name, usd) in others {
        deposit(&mut k, name, *usd);
    }
    cap(&mut k, Asset::Sol, 30_000);
    cap(&mut k, Asset::Eth, 20_000);
    cap(&mut k, Asset::Tsla, 15_000);
    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Eth, 40.0), (Asset::Tsla, 30.0)]);
    settle(&mut k);
    (k, a)
}

/// Scenario 4: Withdrawals are paid from the leg that pays least: Bob's from TSLA, then Alice's from
/// ETH. SOL, which pays most, is not sold for either.
#[test]
fn withdrawals_come_from_the_lowest_paying_leg_first() {
    let (mut k, alice) = three_legs(70_000, &[("bob", 30_000)]);
    let start = observe(&mut k, "three legs");
    let bob = k.backend.env.ctx.signer_pubkey("bob");
    let shares = k.backend.env.shares(bob);
    let id = expect(k.backend.env.request_withdraw("bob", shares));
    let moves = settle(&mut k);
    let mid = observe(&mut k, "Bob leaves");
    assert!(fulfilled_at(&moves, id).is_some(), "{moves:?}");
    assert!(!unwinds(&moves, Asset::Tsla).is_empty(), "{moves:?}");
    assert!(unwinds(&moves, Asset::Eth).is_empty() && unwinds(&moves, Asset::Sol).is_empty(), "{moves:?}");
    assert!(k.backend.env.usdc(bob) as f64 / USDC as f64 > 29_850.0);

    let shares = k.backend.env.shares(alice) * 2 / 7; // $20,000 of her $70,000
    let id = expect(k.backend.env.request_withdraw("alice", shares));
    let moves = settle(&mut k);
    let end = observe(&mut k, "Alice takes $20,000");
    assert!(fulfilled_at(&moves, id).is_some(), "{moves:?}");
    assert!(!unwinds(&moves, Asset::Eth).is_empty(), "{moves:?}");
    assert!(unwinds(&moves, Asset::Sol).is_empty(), "{moves:?}");
    assert!((value(&end, Asset::Sol) - value(&start, Asset::Sol)).abs() < 1.0);
    assert!(value(&end, Asset::Eth) < value(&mid, Asset::Eth) - 9_000.0);
    assert!(k.backend.env.usdc(alice) as f64 / USDC as f64 > 19_900.0);
}

/// Scenario 5: An instant withdrawal is paid from the buffer, with no trade at all. The keeper then
/// sells the lowest-paying leg to bring the buffer back to its 10% reserve.
#[test]
fn instant_withdrawals_use_the_buffer_and_the_keeper_refills_it() {
    let mut env = Env::new();
    open_interest(&mut env);
    expect(env.update_vault(VaultParams {
        buffer_bps: 1_000,
        ..default_params()
    }));
    let mut k = keeper(env, policy());
    deposit(&mut k, "alice", 60_000);
    let bob = deposit(&mut k, "bob", 40_000);
    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Eth, 40.0)]);
    settle(&mut k);
    let start = observe(&mut k, "10% buffer kept");
    assert!(start.snapshot.idle_usdc as u128 >= start.reserved());

    let shares = k.backend.env.shares(bob) / 5; // $8,000
    expect(k.backend.env.withdraw("bob", shares));
    let paid = k.backend.env.usdc(bob) as f64 / USDC as f64;
    let after_withdraw = k.observe().unwrap();
    assert!((7_990.0..8_001.0).contains(&paid), "Bob got ${paid:.2}");
    for asset in [Asset::Sol, Asset::Eth] {
        let i = asset.index();
        assert_eq!(after_withdraw.state(i).units, start.state(i).units, "no trade for Bob");
    }

    let moves = settle(&mut k);
    let end = observe(&mut k, "the keeper refills the buffer");
    assert!(!unwinds(&moves, Asset::Eth).is_empty(), "ETH pays least: {moves:?}");
    assert!(unwinds(&moves, Asset::Sol).is_empty(), "{moves:?}");
    assert!(end.snapshot.idle_usdc as u128 >= end.reserved(), "{}", end.summary());
}

/// Scenario 6: Three requests, oldest first: Bob's, then Carol's. Dave cancels his before the keeper
/// gets to it: his shares are his again and the keeper skips it.
#[test]
fn requests_are_paid_oldest_first_and_a_cancelled_one_is_skipped() {
    let (mut k, alice) = three_legs(50_000, &[("bob", 30_000), ("carol", 20_000), ("dave", 10_000)]);
    let env = &mut k.backend.env;
    let [bob, carol, dave] = ["bob", "carol", "dave"].map(|n| env.ctx.signer_pubkey(n));
    let dave_shares = env.shares(dave);
    let b = expect(env.request_withdraw("bob", env.shares(bob)));
    let c = expect(env.request_withdraw("carol", env.shares(carol)));
    let d = expect(env.request_withdraw("dave", dave_shares));
    expect(env.cancel_withdraw("dave", dave, d));
    let moves = settle(&mut k);
    let end = observe(&mut k, "Bob and Carol paid");
    let (pb, pc) = (fulfilled_at(&moves, b).unwrap(), fulfilled_at(&moves, c).unwrap());
    assert!(pb < pc, "oldest first: {moves:?}");
    assert!(fulfilled_at(&moves, d).is_none());
    let env = &mut k.backend.env;
    assert_eq!(env.shares(dave), dave_shares);
    assert_eq!(env.vault_account().pending_shares, 0);
    assert!(env.usdc(bob) as f64 / USDC as f64 > 29_850.0);
    assert!(env.usdc(carol) as f64 / USDC as f64 > 19_900.0);
    // The best leg is kept for the holders who stay.
    assert!(value(&end, Asset::Sol) > 29_000.0);
    assert!(env.shares(alice) > 0);
}

/// Scenario 7: Alice is in through a rotation; Bob joins after it. Both leave: the keeper unwinds every
/// leg and the vault ends empty. Alice paid for the rotation, Bob did not.
#[test]
fn everyone_leaves_and_the_vault_winds_down_to_nothing() {
    let (mut k, alice) = funded(60_000, wide());
    only(&mut k, &[(Asset::Sol, 25.0)]);
    settle(&mut k);
    cap(&mut k, Asset::Eth, 20_000);
    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Eth, 90.0)]);
    settle(&mut k);
    let bob = deposit(&mut k, "bob", 40_000);
    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Eth, 90.0)]);
    settle(&mut k);
    observe(&mut k, "Alice and Bob in SOL and ETH");

    let env = &mut k.backend.env;
    let a = expect(env.request_withdraw("alice", env.shares(alice)));
    let b = expect(env.request_withdraw("bob", env.shares(bob)));
    let moves = settle(&mut k);
    let end = observe(&mut k, "both leave");
    assert!(fulfilled_at(&moves, a).unwrap() < fulfilled_at(&moves, b).unwrap());
    let env = &mut k.backend.env;
    let (pa, pb) = (env.usdc(alice) as f64 / USDC as f64, env.usdc(bob) as f64 / USDC as f64);
    eprintln!("Alice ${pa:.2} of $60,000, Bob ${pb:.2} of $40,000");
    assert_eq!(end.snapshot.supply, 0);
    assert!(dollars(end.nav()) < 1.0, "{}", end.summary());
    for (i, l) in end.legs.iter().enumerate() {
        assert!(end.value(i) < USDC as u128, "{} still holds", l.symbol);
    }
    assert!(pa / 60_000.0 > 0.99 && pb / 40_000.0 > 0.99);
    assert!(pb / 40_000.0 > pa / 60_000.0, "the rotation was Alice's cost");
}

/// Scenario 8: Every leg's funding turns negative: everything is unwound to USDC in the vault, Phoenix's
/// margin comes back, deposits close. When funding returns, deposits reopen and it is deployed
/// again.
#[test]
fn when_every_leg_stops_paying_it_all_goes_to_usdc_and_comes_back() {
    let (mut k, alice) = three_legs(100_000, &[]);
    let start = observe(&mut k, "three legs");
    // Every paying leg is full: deposits are closed already, new USDC would only sit idle.
    assert!(!start.vault.deposits_open);
    let shares = k.backend.env.shares(alice);
    only(&mut k, &[(Asset::Sol, -10.0), (Asset::Eth, -10.0), (Asset::Tsla, -10.0)]);
    let moves = settle(&mut k);
    let cash = observe(&mut k, "every leg pays shorts nothing");
    assert!(!cash.vault.deposits_open, "{moves:?}");
    for asset in [Asset::Sol, Asset::Eth, Asset::Tsla] {
        assert!(!unwinds(&moves, asset).is_empty(), "{moves:?}");
        assert!(value(&cash, asset) < 1.0);
    }
    assert_eq!(cash.total_notional(), 0);
    assert!(cash.snapshot.idle_usdc as f64 / USDC as f64 > dollars(cash.nav()) * 0.999);
    assert!(dollars(cash.nav()) > dollars(start.nav()) * 0.995);
    assert_eq!(k.backend.env.shares(alice), shares);

    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Eth, 40.0), (Asset::Tsla, 30.0)]);
    let moves = settle(&mut k);
    let back = observe(&mut k, "funding returns");
    assert_eq!(moves.first(), Some(&Action::SetDepositsOpen(true)), "{moves:?}");
    for asset in [Asset::Sol, Asset::Eth, Asset::Tsla] {
        assert!(value(&back, asset) > 14_000.0, "{}", back.summary());
    }
}

/// The vault's short as a share of the market's open interest.
fn oi_share(obs: &Observation, asset: Asset) -> f64 {
    let i = asset.index();
    obs.state(i).short_lots() as f64 / obs.legs[i].open_interest_lots as f64
}

/// Scenario 9: With the default 25% share of open interest: the ETH leg grows as other traders open
/// shorts, and is trimmed when they close them and the vault's share passes 35%.
#[test]
fn open_interest_growing_and_shrinking_resizes_the_leg() {
    let mut env = Env::new();
    env.outside_trade(Asset::Eth, true, 8_000); // others short 8 ETH
    let mut k = keeper(
        env,
        Policy {
            max_trade_usd: 1_000_000,
            ..Policy::default()
        },
    );
    deposit(&mut k, "alice", 100_000);
    only(&mut k, &[(Asset::Eth, 40.0)]);
    settle(&mut k);
    let small = observe(&mut k, "8 ETH of others' open interest");
    assert!((0.18..=0.26).contains(&oi_share(&small, Asset::Eth)), "{}", oi_share(&small, Asset::Eth));

    k.backend.env.outside_trade(Asset::Eth, true, 20_000);
    only(&mut k, &[(Asset::Eth, 40.0)]);
    settle(&mut k);
    let grown = observe(&mut k, "20 ETH more");
    assert!(short(&grown, Asset::Eth) > short(&small, Asset::Eth) * 2.5);
    assert!((0.18..=0.26).contains(&oi_share(&grown, Asset::Eth)), "{}", oi_share(&grown, Asset::Eth));

    k.backend.env.outside_trade(Asset::Eth, false, 18_000);
    let shrunk = k.observe().unwrap();
    assert!(oi_share(&shrunk, Asset::Eth) > 0.35, "{}", oi_share(&shrunk, Asset::Eth));
    only(&mut k, &[(Asset::Eth, 40.0)]);
    let moves = settle(&mut k);
    let trimmed = observe(&mut k, "others close 18 ETH");
    assert!(!unwinds(&moves, Asset::Eth).is_empty(), "{moves:?}");
    assert!(oi_share(&trimmed, Asset::Eth) <= 0.35, "{}", oi_share(&trimmed, Asset::Eth));
}

/// Scenario 10: TSLA pays most but its market is after hours, which its leg does not allow: SOL takes
/// the deposit. When the session opens, the keeper rotates SOL into TSLA.
#[test]
fn a_stock_market_after_hours_waits_and_then_takes_its_share() {
    let (mut k, _) = funded(60_000, wide());
    expect(k.backend.env.update_leg(
        Asset::Tsla,
        LegParams {
            max_notional_usd: 20_000 * USDC,
            after_hours_trading: false,
            ..open_leg()
        },
    ));
    k.backend.env.set_after_hours(Asset::Tsla, true);
    only(&mut k, &[(Asset::Sol, 20.0), (Asset::Tsla, 60.0)]);
    let moves = settle(&mut k);
    let closed = observe(&mut k, "TSLA after hours");
    assert!(!moves.iter().any(|a| matches!(a, Action::Deploy { leg, .. } if *leg == leg_of(Asset::Tsla))));
    assert!(value(&closed, Asset::Sol) > 58_000.0);

    k.backend.env.set_after_hours(Asset::Tsla, false);
    only(&mut k, &[(Asset::Sol, 20.0), (Asset::Tsla, 60.0)]);
    let moves = settle(&mut k);
    let open = observe(&mut k, "TSLA's session opens");
    assert!(!unwinds(&moves, Asset::Sol).is_empty(), "{moves:?}");
    assert!(short(&open, Asset::Tsla) > 19_500.0, "{}", open.summary());
    assert!(value(&open, Asset::Sol) < value(&closed, Asset::Sol) - 39_000.0);
}
