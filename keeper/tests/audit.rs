//! Regression tests for the keeper's 2026-10-16 audit (AUDIT.md, K-1 to K-7): each is the
//! finding's scenario, end to end against the vault and Phoenix.

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_spl::associated_token::get_associated_token_address;
use vanna_keeper::{Action, Keeper, Policy};
use vanna_phoenix_multi_vault::instructions::{LegParams, VaultParams};
use vanna_phoenix_multi_vault::state::request_address;

use crate::env::*;
use crate::scenarios::*;
use crate::sim::*;

fn wide() -> Policy {
    Policy {
        max_leg_nav_share_bps: 10_000,
        ..policy()
    }
}

fn only(k: &mut Keeper<Sim>, rates: &[(Asset, f64)]) {
    let all: Vec<(Asset, f64)> = Asset::ALL
        .into_iter()
        .map(|a| (a, rates.iter().find(|r| r.0 == a).map_or(0.0, |r| r.1)))
        .collect();
    set_funding(k, &all);
}

fn cap(k: &mut Keeper<Sim>, asset: Asset, usd: u64) {
    expect(k.backend.env.update_leg(
        asset,
        LegParams {
            max_notional_usd: usd * USDC,
            ..open_leg()
        },
    ));
}

fn deposit(k: &mut Keeper<Sim>, name: &str, usd: u64) -> Pubkey {
    let user = k.backend.env.add_user(name, 10 * SOL);
    expect(k.backend.env.deposit(name, usd * USDC));
    user
}

fn unwinds(moves: &[Action], asset: Asset) -> usize {
    moves
        .iter()
        .filter(|a| matches!(a, Action::Unwind { leg, .. } if *leg == leg_of(asset)))
        .count()
}

/// Ticks until nothing more happens, allowing moves to fail; returns what was done and how
/// many moves failed.
fn run(k: &mut Keeper<Sim>, ticks: usize) -> (Vec<Action>, usize) {
    let (mut done, mut failed) = (vec![], 0);
    for _ in 0..ticks {
        let report = k.tick().unwrap();
        failed += report.failed.len();
        if report.executed.is_empty() && report.failed.is_empty() {
            break;
        }
        done.extend(report.executed.into_iter().map(|(d, _)| d.action));
    }
    (done, failed)
}

/// K-1: the oldest request held the queue when the program could not pay it (its user closed
/// their USDC account, or it paid nothing): every later request, and the buffer refill,
/// waited forever. It is returned now, and the next one is paid.
#[test]
fn a_request_the_program_cannot_pay_is_returned_and_the_queue_moves_on() {
    let mut env = Env::new();
    open_interest(&mut env);
    expect(env.update_vault(VaultParams {
        exit_fee_bps: 10,
        ..default_params()
    }));
    let mut k = keeper(env, wide());
    deposit(&mut k, "alice", 20_000);
    let mallory = deposit(&mut k, "mallory", 5_000);
    let carol = deposit(&mut k, "carol", 1);
    let bob = deposit(&mut k, "bob", 5_000);
    only(&mut k, &[(Asset::Sol, 25.0)]);
    settle(&mut k);
    let env = &mut k.backend.env;
    let m = expect(env.request_withdraw("mallory", env.shares(mallory)));
    let usdc = get_associated_token_address(&mallory, &env.usdc_mint);
    let close = Instruction {
        program_id: TOKEN_PROGRAM,
        accounts: vec![
            AccountMeta::new(usdc, false),
            AccountMeta::new(mallory, false),
            AccountMeta::new_readonly(mallory, true),
        ],
        data: vec![9],
    };
    expect(env.send("mallory", vec![close], "close usdc"));
    let c = expect(env.request_withdraw("carol", 1)); // one share atom: pays nothing
    let b = expect(env.request_withdraw("bob", env.shares(bob)));

    let moves = settle(&mut k);
    for id in [m, c] {
        assert!(moves.iter().any(|a| matches!(a, Action::ReturnWithdraw { id: i, .. } if *i == id)), "{moves:?}");
    }
    assert!(moves.iter().any(|a| matches!(a, Action::FulfillWithdraw { id, .. } if *id == b)));
    let env = &mut k.backend.env;
    assert!(env.usdc(bob) > 4_980 * USDC);
    assert_eq!(env.vault_account().pending_shares, 0);
    // Mallory's shares wait for her, whole.
    assert!(env.ctx.svm.get_account(&request_address(&env.vault.address, m).0).is_some());
    expect(env.cancel_withdraw("mallory", mallory, m));
    assert!(env.shares(mallory) > 4_990 * USDC);
    assert!(env.shares(carol) > 0);
}

/// K-2: after a SOL rally the vault owes Phoenix USDC; with every idle dollar reserved for a
/// request, the keeper could neither post it nor sell the SOL, and the request waited forever.
#[test]
fn a_sol_vault_pays_a_full_withdrawal_after_a_rally() {
    let (mut k, alice) = funded(40_000, wide());
    only(&mut k, &[(Asset::Sol, 25.0)]);
    settle(&mut k);
    k.backend.env.move_price(Asset::Sol, 195.0);
    let rally = k.observe().unwrap();
    assert!(rally.snapshot.margin.equity < -8_000 * USDC as i128, "{}", rally.summary());
    let env = &mut k.backend.env;
    let id = expect(env.request_withdraw("alice", env.shares(alice)));
    let moves = settle(&mut k);
    assert!(moves.iter().any(|a| matches!(a, Action::FulfillWithdraw { id: i, .. } if *i == id)), "{moves:?}");
    let paid = k.backend.env.usdc(alice) as f64 / USDC as f64;
    assert!(paid > dollars(rally.nav()) * 0.99, "paid ${paid:.2} of ${:.2}", dollars(rally.nav()));
    assert!(dollars(k.observe().unwrap().nav()) < 1.0);
}

/// K-3: a market that cannot be bought (no route, a thin book) made every tick sell the leg
/// it rotated out of, fail to buy, and buy it back. The keeper now checks the route first, and
/// a failed market waits out a cooldown that doubles each time.
#[test]
fn a_market_that_cannot_be_bought_does_not_churn_the_vault() {
    let (mut k, _) = funded(60_000, wide());
    only(&mut k, &[(Asset::Sol, 25.0)]);
    settle(&mut k);
    let start = k.observe().unwrap();
    cap(&mut k, Asset::Tsla, 20_000);
    k.backend.no_route.insert(TSLA_MINT);
    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Tsla, 100.0)]);
    let (moves, failed) = run(&mut k, 6);
    let end = k.observe().unwrap();
    assert_eq!(unwinds(&moves, Asset::Sol), 0, "{moves:?}");
    assert!(failed >= 1);
    assert!(!moves.iter().any(|a| matches!(a, Action::MoveMargin { .. })), "{moves:?}");
    assert_eq!(end.state(Asset::Sol.index()).units, start.state(Asset::Sol.index()).units);
}

/// K-4: raising USDC tried one leg only: if the cheapest leg could not be sold, a request
/// waited. The next leg pays now.
#[test]
fn if_the_cheapest_leg_cannot_be_sold_the_next_one_pays() {
    let (mut k, _) = funded(70_000, wide());
    let bob = deposit(&mut k, "bob", 30_000);
    cap(&mut k, Asset::Sol, 30_000);
    cap(&mut k, Asset::Eth, 20_000);
    cap(&mut k, Asset::Tsla, 15_000);
    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Eth, 40.0), (Asset::Tsla, 30.0)]);
    settle(&mut k);
    k.backend.no_route.insert(TSLA_MINT);
    let env = &mut k.backend.env;
    let id = expect(env.request_withdraw("bob", env.shares(bob)));
    let before = k.observe().unwrap();
    let (moves, _) = run(&mut k, 6);
    assert!(moves.iter().any(|a| matches!(a, Action::FulfillWithdraw { id: i, .. } if *i == id)), "{moves:?}");
    assert!(unwinds(&moves, Asset::Eth) > 0, "{moves:?}");
    let after = k.observe().unwrap();
    let tsla = Asset::Tsla.index();
    assert_eq!(after.state(tsla).units, before.state(tsla).units);
    assert!(k.backend.env.usdc(bob) > 29_800 * USDC);
}

/// K-5: in a stock token's multiplier window, its sales must fetch the higher multiplier's
/// value (the program's check). The keeper leaves such a leg alone and raises a request from
/// the others, even ones that pay more.
#[test]
fn a_stock_in_a_multiplier_window_is_left_alone() {
    let (mut k, alice) = funded(60_000, wide());
    cap(&mut k, Asset::Sol, 30_000);
    cap(&mut k, Asset::Tsla, 15_000);
    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Tsla, 20.0)]);
    settle(&mut k);
    let env = &mut k.backend.env;
    let now = env.now();
    let m = env.multiplier(Asset::Tsla);
    env.set_multiplier(Asset::Tsla, m, m * 1.5, now + 3_600);
    let shares = env.shares(alice) / 6; // $10,000
    let id = expect(env.request_withdraw("alice", shares));
    only(&mut k, &[(Asset::Sol, 25.0), (Asset::Tsla, 20.0)]);
    let moves = settle(&mut k);
    assert!(moves.iter().any(|a| matches!(a, Action::FulfillWithdraw { id: i, .. } if *i == id)), "{moves:?}");
    assert_eq!(unwinds(&moves, Asset::Tsla), 0, "{moves:?}");
    assert!(unwinds(&moves, Asset::Sol) > 0);
    assert!(!moves.iter().any(|a| matches!(a, Action::Rehedge { .. })));
}

/// K-6: alerts were deduplicated by their text, which carries live numbers: a stale funding
/// alert went out every tick as the leg's value moved. Once an hour now.
#[test]
fn an_alert_goes_out_once_however_its_numbers_move() {
    let (mut k, _) = funded(20_000, wide());
    only(&mut k, &[(Asset::Sol, 25.0)]);
    settle(&mut k);
    k.backend.funding.clear();
    k.backend.env.warp_hours(4); // the last funding read goes stale
    for price in [151.0, 152.0, 153.0] {
        k.backend.env.move_price(Asset::Sol, price);
        k.tick().unwrap();
    }
    let stale = k
        .backend
        .alerts
        .iter()
        .filter(|a| a.contains("no fresh funding for SOL"))
        .count();
    assert_eq!(stale, 1, "{:?}", k.backend.alerts);
}
