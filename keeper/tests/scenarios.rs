//! The keeper end to end: it reads the vault, decides and sends real transactions to the vault
//! program, Phoenix and the mock Jupiter, tick after tick, until it has nothing left to do.

use vanna_keeper::{Action, Keeper, Observation, Policy};
use vanna_phoenix_multi_vault::instructions::VaultParams;
use vanna_phoenix_multi_vault::state::request_address;

use crate::env::*;
use crate::sim::*;

pub fn expect<T>(result: Result<T, Vec<String>>) -> T {
    result.unwrap_or_else(|logs| panic!("{}", logs.join("\n")))
}

pub fn dollars(v: u128) -> f64 {
    v as f64 / USDC as f64
}

/// The keeper's defaults, with the market's own open interest as the only limit and room for
/// large trades, so a test settles in a few ticks.
pub fn policy() -> Policy {
    Policy {
        max_oi_share_bps: 10_000,
        trim_oi_share_bps: 20_000,
        max_trade_usd: 1_000_000,
        ..Policy::default()
    }
}

/// Other traders short each market the tests use: open interest for the vault to share.
pub fn open_interest(env: &mut Env) {
    env.outside_trade(Asset::Sol, true, 50_000); // 500 SOL
    env.outside_trade(Asset::Eth, true, 20_000); // 20 ETH
    env.outside_trade(Asset::Tsla, true, 100_000); // 100 TSLA
}

/// A vault with open interest and Alice's deposit, and its keeper.
pub fn funded(usd: u64, policy: Policy) -> (Keeper<Sim>, anchor_lang::prelude::Pubkey) {
    let mut env = Env::new();
    open_interest(&mut env);
    let alice = env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", usd * USDC));
    (keeper(env, policy), alice)
}

/// Ticks until the keeper has nothing left to do; returns every move it made. No move may
/// fail: the keeper must only send what the program accepts.
pub fn settle(k: &mut Keeper<Sim>) -> Vec<Action> {
    let mut done = vec![];
    for _ in 0..12 {
        let report = k.tick().unwrap();
        assert!(report.failed.is_empty(), "{:#?}", report.failed);
        if report.executed.is_empty() {
            return done;
        }
        done.extend(report.executed.into_iter().map(|(d, _)| d.action));
    }
    panic!("the keeper never settled: {done:?}");
}

pub fn assert_hedged(obs: &Observation) {
    for (i, l) in obs.legs.iter().enumerate().filter(|(_, l)| l.live()) {
        let s = obs.state(i);
        let target = s.market.hedge_lots(s.units).unwrap() as i64;
        assert!(
            (target + s.position_lots).abs() <= 1,
            "{} short {} lots for {} held",
            l.symbol,
            -s.position_lots,
            target
        );
    }
}

pub fn leg(asset: Asset) -> u8 {
    asset.index() as u8
}

#[test]
fn the_keeper_reads_the_vault_the_way_the_program_does() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", 50_000 * USDC));
    env.deploy_hedged(Asset::Eth, 20_000 * USDC);
    env.deploy_hedged(Asset::Sol, 10_000 * USDC);
    env.deploy_hedged(Asset::Tsla, 8_000 * USDC);
    let view = env.view();
    let mut k = keeper(env, policy());
    let obs = k.observe().unwrap();
    eprintln!("{}", obs.summary());
    assert!((dollars(obs.nav()) - view.nav_usd).abs() < 0.01, "{} vs {}", dollars(obs.nav()), view.nav_usd);
    assert_eq!(obs.snapshot.idle_usdc as f64 / USDC as f64, view.idle_usd);
    assert_eq!(obs.snapshot.margin.effective as f64 / USDC as f64, view.effective_usd);
    assert_eq!(obs.snapshot.supply, view.supply);
    for asset in Asset::ALL {
        let i = asset.index();
        assert_eq!(obs.state(i).units, view.leg(asset).units, "{}", asset.symbol());
        assert_eq!(obs.state(i).position_lots, view.leg(asset).position_lots);
        assert_eq!(obs.legs[i].symbol, asset.symbol(), "Phoenix's name for the market");
    }
    assert_hedged(&obs);
}

#[test]
fn deposits_go_to_the_best_paying_legs_hedged() {
    let (mut k, _) = funded(100_000, policy());
    set_funding(
        &mut k,
        &[
            (Asset::Sol, 25.0),
            (Asset::Eth, 30.0),
            (Asset::Btc, 8.0),
            (Asset::Tsla, 40.0),
            (Asset::Meta, 2.0),
        ],
    );
    let moves = settle(&mut k);
    k.backend.env.print("the keeper deploys $100,000");
    let obs = k.observe().unwrap();
    // SOL pays 25% on its capital, TSLA 20% and ETH 15% (spot legs at 1x earn on half);
    // BTC (4%) and META (1%) are under the 6% entry.
    assert!(obs.value(Asset::Sol.index()) > 40_000 * USDC as u128, "{moves:?}");
    assert!(obs.value(Asset::Tsla.index()) > 10_000 * USDC as u128);
    assert_eq!(obs.value(Asset::Btc.index()), 0);
    assert_eq!(obs.value(Asset::Meta.index()), 0);
    assert!(moves.iter().any(|a| matches!(a, Action::MoveMargin { to_phoenix: true, .. })));
    assert_hedged(&obs);
    // Everything but the slack is at work, and costs stay small.
    assert!(obs.free_usdc() < 2_000 * USDC, "{}", obs.summary());
    assert!(dollars(obs.nav()) > 99_500.0, "{}", obs.summary());
    assert!(obs.margin_bps().unwrap() >= 7_000, "{}", obs.summary());
}

#[test]
fn when_nothing_pays_the_keeper_keeps_the_usdc_and_closes_deposits() {
    let (mut k, alice) = funded(50_000, policy());
    let bob = k.backend.env.add_user("bob", 10 * SOL);
    set_funding(
        &mut k,
        &[
            (Asset::Sol, 1.0),
            (Asset::Eth, -5.0),
            (Asset::Btc, 2.0),
            (Asset::Tsla, 0.5),
            (Asset::Meta, -1.0),
        ],
    );
    // Nothing reaches the entry rate: no trade, only the funding guard.
    let moves = settle(&mut k);
    assert_eq!(moves, vec![Action::SetDepositsOpen(false)]);
    let env = &mut k.backend.env;
    assert!(!env.vault_account().deposits_open);
    assert!(failed_with(env.deposit("bob", 1_000 * USDC), "DepositsClosed"));
    // The USDC never left the vault: Alice takes all of it back at once.
    let shares = env.shares(alice);
    expect(env.withdraw("alice", shares));
    assert!(env.usdc(alice) >= 49_999 * USDC, "{}", env.usdc(alice));
    assert_eq!(env.usdc(bob), 1_000 * USDC, "Bob's deposit was refused, his USDC is his");

    // Funding comes back: the guard reopens deposits.
    set_funding(&mut k, &[(Asset::Sol, 12.0)]);
    let moves = settle(&mut k);
    assert_eq!(moves, vec![Action::SetDepositsOpen(true)]);
}

#[test]
fn a_leg_that_stops_paying_is_unwound_and_its_usdc_stays_in_the_vault() {
    let (mut k, alice) = funded(100_000, policy());
    set_funding(
        &mut k,
        &[
            (Asset::Sol, 25.0),
            (Asset::Eth, 40.0),
            (Asset::Btc, 0.0),
            (Asset::Tsla, 0.0),
            (Asset::Meta, 0.0),
        ],
    );
    settle(&mut k);
    let before = k.observe().unwrap();
    let (sol, eth) = (Asset::Sol.index(), Asset::Eth.index());
    assert!(before.value(eth) > 10_000 * USDC as u128, "{}", before.summary());
    let shares = k.backend.env.shares(alice);

    set_funding(&mut k, &[(Asset::Eth, -10.0)]);
    let moves = settle(&mut k);
    k.backend.env.print("ETH funding turns negative");
    let after = k.observe().unwrap();
    assert!(moves.iter().any(|a| matches!(a, Action::Unwind { leg, .. } if *leg == leg_of(Asset::Eth))));
    assert!(moves.iter().any(|a| matches!(a, Action::MoveMargin { to_phoenix: false, .. })), "its margin comes back too: {moves:?}");
    assert_eq!(after.state(eth).units, 0);
    assert_eq!(after.state(eth).position_lots, 0);
    assert_eq!(after.state(sol).units, before.state(sol).units, "SOL still pays: kept");
    // The ETH and its margin are USDC in the vault now, not paid out to anyone.
    assert!(
        after.snapshot.idle_usdc as u128 > before.snapshot.idle_usdc as u128 + before.value(eth) * 19 / 10,
        "{}",
        after.summary()
    );
    assert_eq!(k.backend.env.shares(alice), shares);
    assert!(dollars(after.nav()) > dollars(before.nav()) * 0.997);
    assert_hedged(&after);
}

pub fn leg_of(asset: Asset) -> u8 {
    leg(asset)
}

#[test]
fn the_keeper_raises_usdc_for_a_withdrawal_request_and_pays_it() {
    let (mut k, alice) = funded(60_000, policy());
    let bob = k.backend.env.add_user("bob", 10 * SOL);
    expect(k.backend.env.deposit("bob", 40_000 * USDC));
    set_funding(
        &mut k,
        &[
            (Asset::Sol, 25.0),
            (Asset::Eth, 40.0),
            (Asset::Tsla, 30.0),
            (Asset::Btc, 0.0),
            (Asset::Meta, 0.0),
        ],
    );
    settle(&mut k);
    let env = &mut k.backend.env;
    let start = env.print("the keeper deployed $100,000");
    let shares = env.shares(bob);
    assert!(failed_with(env.withdraw("bob", shares), "NotEnoughLiquidity"));
    let id = expect(env.request_withdraw("bob", shares));

    let moves = settle(&mut k);
    let env = &mut k.backend.env;
    let end = env.print("the keeper raised and paid Bob's request");
    assert!(moves.iter().any(|a| matches!(a, Action::FulfillWithdraw { id: i, .. } if *i == id)), "{moves:?}");
    assert!(moves.iter().any(|a| matches!(a, Action::Unwind { .. })));
    assert!(env.ctx.svm.get_account(&request_address(&env.vault.address, id).0).is_none());
    assert_eq!(env.vault_account().pending_shares, 0);
    let paid = env.usdc(bob) as f64 / USDC as f64;
    eprintln!("Bob paid ${paid:.2}");
    assert!(paid > 39_800.0, "{paid}");
    // Alice's shares are worth what they were, less a share of the trades.
    assert!(end.share_price() > start.share_price() * 0.997);
    assert!(env.shares(alice) > 0);
    assert_hedged(&k.observe().unwrap());
}

#[test]
fn a_rally_thins_the_margin_and_the_keeper_restores_it() {
    let (mut k, _) = funded(
        100_000,
        Policy {
            max_leg_nav_share_bps: 10_000,
            ..policy()
        },
    );
    set_funding(
        &mut k,
        &[
            (Asset::Eth, 40.0),
            (Asset::Sol, 0.0),
            (Asset::Btc, 0.0),
            (Asset::Tsla, 0.0),
            (Asset::Meta, 0.0),
        ],
    );
    settle(&mut k);
    k.backend.env.move_price(Asset::Eth, 4_900.0);
    let rally = k.observe().unwrap();
    eprintln!("ETH +40%: {}", rally.summary());
    assert!(rally.margin_bps().unwrap() < 6_000, "the short's loss came out of the margin");

    let moves = settle(&mut k);
    k.backend.env.print("the keeper restores the margin");
    let after = k.observe().unwrap();
    assert!(moves.iter().any(|a| matches!(a, Action::Unwind { leg, .. } if *leg == leg_of(Asset::Eth))), "{moves:?}");
    assert!(after.margin_bps().unwrap() >= 7_000, "{}", after.summary());
    assert!(dollars(after.nav()) > dollars(rally.nav()) * 0.995);
    assert_hedged(&after);
}

#[test]
fn the_sol_leg_leaves_after_a_rally_by_covering_its_short_in_usdc_first() {
    let (mut k, _) = funded(
        40_000,
        Policy {
            max_leg_nav_share_bps: 10_000,
            ..policy()
        },
    );
    set_funding(
        &mut k,
        &[
            (Asset::Sol, 25.0),
            (Asset::Eth, 0.0),
            (Asset::Btc, 0.0),
            (Asset::Tsla, 0.0),
            (Asset::Meta, 0.0),
        ],
    );
    settle(&mut k);
    // SOL +30%: the SOL is worth more, the short owes the difference in USDC on Phoenix.
    k.backend.env.move_price(Asset::Sol, 195.0);
    let rally = k.observe().unwrap();
    assert!(rally.snapshot.margin.equity < -8_000 * USDC as i128, "{}", rally.summary());

    set_funding(&mut k, &[(Asset::Sol, -10.0)]);
    let moves = settle(&mut k);
    k.backend.env.print("SOL funding turns negative after a rally");
    let after = k.observe().unwrap();
    let sol = Asset::Sol.index();
    assert_eq!(after.state(sol).units, 0, "{moves:?}");
    assert_eq!(after.state(sol).position_lots, 0);
    assert!(dollars(after.nav()) > dollars(rally.nav()) * 0.995, "{}", after.summary());
    assert!(after.snapshot.idle_usdc as f64 > dollars(after.nav()) * 0.99, "all of it is USDC in the vault");
}

#[test]
fn the_keeper_rehedges_a_donation() {
    let (mut k, _) = funded(40_000, policy());
    set_funding(
        &mut k,
        &[
            (Asset::Eth, 40.0),
            (Asset::Sol, 0.0),
            (Asset::Btc, 0.0),
            (Asset::Tsla, 0.0),
            (Asset::Meta, 0.0),
        ],
    );
    settle(&mut k);
    k.backend.env.add_user("donor", 10 * SOL);
    k.backend.env.donate_asset(Asset::Eth, "donor", ETH);
    let moves = settle(&mut k);
    assert_eq!(moves.first(), Some(&Action::Rehedge { leg: leg(Asset::Eth) }), "{moves:?}");
    assert_hedged(&k.observe().unwrap());
}

#[test]
fn rotation_waits_for_a_gap_that_pays_for_it() {
    let (mut k, _) = funded(
        60_000,
        Policy {
            max_leg_nav_share_bps: 10_000,
            ..policy()
        },
    );
    set_funding(
        &mut k,
        &[
            (Asset::Sol, 25.0),
            (Asset::Eth, 0.0),
            (Asset::Btc, 0.0),
            (Asset::Tsla, 0.0),
            (Asset::Meta, 0.0),
        ],
    );
    settle(&mut k);
    let start = k.observe().unwrap();
    let (sol, eth) = (Asset::Sol.index(), Asset::Eth.index());
    assert!(start.value(sol) > 55_000 * USDC as u128, "{}", start.summary());

    // ETH at 60% is 30% on capital: 5% more than SOL, under the 8% a rotation must clear.
    set_funding(&mut k, &[(Asset::Eth, 60.0)]);
    assert!(settle(&mut k).is_empty());

    // At 90% it is 45%: rotate.
    set_funding(&mut k, &[(Asset::Eth, 90.0)]);
    let moves = settle(&mut k);
    k.backend.env.print("the keeper rotates SOL into ETH");
    let end = k.observe().unwrap();
    assert!(moves.iter().any(|a| matches!(a, Action::Unwind { leg, .. } if *leg == leg_of(Asset::Sol))), "{moves:?}");
    assert!(moves.iter().any(|a| matches!(a, Action::Deploy { leg, .. } if *leg == leg_of(Asset::Eth))));
    assert!(end.value(eth) > 20_000 * USDC as u128, "{}", end.summary());
    assert!(dollars(end.nav()) > dollars(start.nav()) * 0.995);
    assert_hedged(&end);
}

#[test]
fn a_rotation_into_a_leg_with_20k_of_room_sells_40k_of_sol_at_once() {
    let (mut k, _) = funded(
        60_000,
        Policy {
            max_leg_nav_share_bps: 10_000,
            ..policy()
        },
    );
    set_funding(
        &mut k,
        &[
            (Asset::Sol, 25.0),
            (Asset::Eth, 0.0),
            (Asset::Btc, 0.0),
            (Asset::Tsla, 0.0),
            (Asset::Meta, 0.0),
        ],
    );
    settle(&mut k);
    // ETH has $20,000 of room and pays 45% on its capital: worth $40,000 of SOL (the ETH and
    // its margin), in one sale.
    expect(k.backend.env.update_leg(
        Asset::Eth,
        vanna_phoenix_multi_vault::instructions::LegParams {
            max_notional_usd: 20_000 * USDC,
            ..open_leg()
        },
    ));
    let start = k.observe().unwrap();
    set_funding(&mut k, &[(Asset::Eth, 90.0)]);
    let moves = settle(&mut k);
    k.backend.env.print("SOL into ETH's $20,000 of room");
    let end = k.observe().unwrap();
    let sol_sales = moves
        .iter()
        .filter(|a| matches!(a, Action::Unwind { leg, .. } if *leg == leg_of(Asset::Sol)))
        .count();
    assert_eq!(sol_sales, 1, "{moves:?}");
    let (sol, eth) = (Asset::Sol.index(), Asset::Eth.index());
    let sold = dollars(start.value(sol)) - dollars(end.value(sol));
    assert!((sold - 39_600.0).abs() < 100.0, "sold ${sold:.2}: {moves:?}");
    assert!(dollars(end.notional(eth)) > 19_500.0, "{}", end.summary());
    assert!(dollars(end.notional(eth)) <= 20_000.0);
    assert_hedged(&end);
}

#[test]
fn a_dry_run_plans_and_sends_nothing() {
    let (mut k, _) = funded(50_000, policy());
    k.settings.dry_run = true;
    set_funding(&mut k, &[(Asset::Sol, 25.0)]);
    let before = k.observe().unwrap();
    let report = k.tick().unwrap();
    assert!(report.executed.is_empty());
    assert!(matches!(report.planned.first().map(|d| &d.action), Some(Action::Deploy { .. })), "{:?}", report.planned);
    let after = k.observe().unwrap();
    assert_eq!(after.snapshot.idle_usdc, before.snapshot.idle_usdc);
}

#[test]
fn the_keeper_stays_within_the_daily_turnover() {
    let mut env = Env::new();
    open_interest(&mut env);
    expect(env.update_vault(VaultParams {
        max_turnover_usd: 30_000 * USDC,
        ..default_params()
    }));
    env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", 60_000 * USDC));
    let mut k = keeper(
        env,
        Policy {
            max_leg_nav_share_bps: 10_000,
            ..policy()
        },
    );
    set_funding(&mut k, &[(Asset::Sol, 25.0)]);
    settle(&mut k);
    let day_one = k.observe().unwrap();
    let sol = Asset::Sol.index();
    assert!(dollars(day_one.value(sol)) <= 30_100.0, "{}", day_one.summary());
    assert!(dollars(day_one.value(sol)) > 29_000.0);

    k.backend.env.warp_hours(25);
    set_funding(&mut k, &[(Asset::Sol, 25.0)]);
    settle(&mut k);
    let day_two = k.observe().unwrap();
    assert!(dollars(day_two.value(sol)) > 55_000.0, "{}", day_two.summary());
}
