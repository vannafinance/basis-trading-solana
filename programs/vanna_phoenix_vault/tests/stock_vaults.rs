//! Tokenized-stock vaults: TSLA and META, held as Backed's xStocks (Token-2022) and hedged on
//! Phoenix's TSLA and META perps. The tokens are the mainnet mints with their real extensions;
//! deposits and withdrawals run real Jupiter routes over Raydium's TSLAx and METAx pools.

use crate::env::*;
use vanna_phoenix_vault::events::{Deposited, Swapped, Withdrawn};
use vanna_phoenix_vault::instructions::VaultParams;
use vanna_phoenix_vault::swap::Sell;

fn expect<T>(result: Result<T, Vec<String>>) -> T {
    result.unwrap_or_else(|logs| panic!("{}", logs.join("\n")))
}

fn stock_params() -> VaultParams {
    VaultParams {
        margin_bps: 9_000,
        ..default_params()
    }
}

/// The short a holding of `units` stock tokens needs: the shares they are worth, in 0.001-share
/// lots.
fn hedge_lots(env: &Env, asset: Asset, units: u64) -> i64 {
    (units as f64 * env.multiplier(asset) / 100_000.0).round() as i64
}

#[test]
fn tsla_vault_usdc_in_usdc_out_through_jupiter() {
    let mut env = Env::new();
    env.update_vault(Asset::Tsla, stock_params());
    env.use_jupiter();
    let ann = env.add_user("ann", 10 * SOL);
    let quoted = env
        .jupiter_route_for(Asset::Tsla, Sell::Usdc, 5_000 * USDC)
        .quoted_out;

    let meta = expect(env.deposit(Asset::Tsla, "ann", 10_000 * USDC));
    let swap = &events::<Swapped>(&meta)[0];
    let dep = &events::<Deposited>(&meta)[0];
    let v = env.print(Asset::Tsla, "ann deposits $10,000");
    eprintln!(
        "USDC -> TSLAx: $5,000 bought {:.6} TSLAx (Jupiter quoted {:.6}), {} CU",
        swap.received as f64 / STOCK as f64,
        quoted as f64 / STOCK as f64,
        meta.compute_units_consumed
    );
    assert_eq!(swap.spent, 5_000 * USDC, "half the USDC buys TSLAx");
    assert!(swap.received * 1_000 >= quoted * 995);
    assert_eq!(v.asset_units, swap.received, "the vault holds what arrived");
    assert_eq!(
        -v.position_lots,
        hedge_lots(&env, Asset::Tsla, swap.received),
        "and is short exactly that many TSLA"
    );
    assert!(
        (v.collateral_usd - 5_000.0).abs() < 5.0,
        "the rest is margin"
    );
    assert!(usd(dep.value_usd) > 9_950.0, "{}", usd(dep.value_usd));

    // Out again in USDC: the vault sells all of ann's TSLAx through Jupiter.
    let shares = env.shares(Asset::Tsla, ann);
    let meta = expect(env.withdraw(Asset::Tsla, "ann", shares));
    let swap = &events::<Swapped>(&meta)[0];
    let out = &events::<Withdrawn>(&meta)[0];
    let v = env.print(Asset::Tsla, "ann withdraws everything");
    let back = usd(env.usdc(ann));
    eprintln!(
        "ann: $10,000.00 in, ${back:.2} USDC out ({:.3}% for two swaps and two trades)",
        (1.0 - back / 10_000.0) * 100.0
    );
    assert_eq!(swap.spent, out.asset_sold, "the route sold all of it");
    assert_eq!(env.token(ann, TSLA_MINT), 0, "no TSLAx comes back");
    assert!(back > 9_900.0);
    assert_eq!(v.position_lots, 0, "the short is closed");
}

#[test]
fn a_meta_token_is_worth_more_than_a_share_and_the_short_follows() {
    let mut env = Env::new();
    env.update_vault(Asset::Meta, stock_params());
    let multiplier = env.multiplier(Asset::Meta);
    eprintln!("METAx multiplier: {multiplier} shares per token (reinvested dividends)");
    assert!(
        multiplier > 1.0028,
        "mainnet METAx after its September dividend"
    );

    let bob = env.add_user("bob", 10 * SOL);
    let meta = expect(env.deposit(Asset::Meta, "bob", 15_000 * USDC));
    let dep = &events::<Deposited>(&meta)[0];
    let v = env.print(Asset::Meta, "bob deposits $15,000");
    // $7,500 buys tokens at the share price times the multiplier; the short is the shares
    // those tokens are worth, more than the token count.
    let tokens = dep.asset_bought as f64 / STOCK as f64;
    eprintln!(
        "$7,500 bought {tokens:.6} METAx at ${:.2} x {multiplier:.6}; short {:.3} META",
        v.mark_usd,
        -v.position_lots as f64 / 1_000.0
    );
    assert_eq!(
        -v.position_lots,
        hedge_lots(&env, Asset::Meta, dep.asset_bought)
    );
    assert!(-v.position_lots as f64 > tokens * 1_000.0 + 25.0);
    assert!(usd(dep.value_usd) > 14_950.0, "{}", usd(dep.value_usd));

    // The price moves; the vault does not.
    env.move_price(Asset::Meta, 820.0);
    let up = env.print(Asset::Meta, "META +10%");
    assert!((up.nav_usd - v.nav_usd).abs() < v.nav_usd * 0.001);

    // Bob takes his part back in USDC: a Token-2022 transfer out of the vault to the swap.
    let shares = env.shares(Asset::Meta, bob);
    let meta = expect(env.withdraw(Asset::Meta, "bob", shares));
    let out = &events::<Withdrawn>(&meta)[0];
    env.print(Asset::Meta, "bob withdraws everything");
    eprintln!(
        "bob sold {:.4} METAx, got ${:.2} at $820, for ${:.2} put in",
        out.asset_sold as f64 / STOCK as f64,
        usd(out.usdc_out),
        usd(dep.value_usd)
    );
    assert_eq!(out.asset_sold, dep.asset_bought, "all his METAx is sold");
    assert_eq!(env.token(bob, META_MINT), 0);
    assert!((usd(out.usdc_out) - usd(dep.value_usd)).abs() < 20.0);
}

#[test]
fn meta_vault_usdc_in_usdc_out_through_raydium() {
    let mut env = Env::new();
    env.update_vault(Asset::Meta, stock_params());
    env.use_jupiter();
    let cy = env.add_user("cy", 10 * SOL);
    let meta = expect(env.deposit(Asset::Meta, "cy", 5_000 * USDC));
    let swap = &events::<Swapped>(&meta)[0];
    let v = env.print(Asset::Meta, "cy deposits $5,000");
    eprintln!(
        "USDC -> METAx: $2,500 bought {:.6} METAx, {} CU",
        swap.received as f64 / STOCK as f64,
        meta.compute_units_consumed
    );
    assert_eq!(swap.spent, 2_500 * USDC, "half the USDC buys METAx");
    assert_eq!(v.asset_units, swap.received);
    assert_eq!(-v.position_lots, hedge_lots(&env, Asset::Meta, swap.received));

    let shares = env.shares(Asset::Meta, cy);
    let meta = expect(env.withdraw(Asset::Meta, "cy", shares));
    let swap = &events::<Swapped>(&meta)[0];
    let out = &events::<Withdrawn>(&meta)[0];
    env.print(Asset::Meta, "cy withdraws everything");
    eprintln!(
        "METAx -> USDC: {:.6} METAx sold for ${:.2}; cy got ${:.2}",
        swap.spent as f64 / STOCK as f64,
        usd(swap.received),
        usd(out.usdc_out)
    );
    assert_eq!(swap.spent, out.asset_sold);
    assert_eq!(env.token(cy, META_MINT), 0);
    assert!(usd(out.usdc_out) > 5_000.0 * 0.98);
}

#[test]
fn stock_deposits_wait_for_the_session_but_withdrawals_do_not() {
    let mut env = Env::new();
    env.update_vault(
        Asset::Tsla,
        VaultParams {
            after_hours_deposits: false,
            ..default_params()
        },
    );
    let dee = env.add_user("dee", 10 * SOL);
    expect(env.deposit(Asset::Tsla, "dee", 8_000 * USDC));

    // Friday 8pm New York: Phoenix holds TSLA after hours until Sunday night.
    env.set_after_hours(Asset::Tsla, true);
    assert!(failed_with(
        env.deposit(Asset::Tsla, "dee", 8_000 * USDC),
        "MarketAfterHours"
    ));
    let half = env.shares(Asset::Tsla, dee) / 2;
    let meta = expect(env.withdraw(Asset::Tsla, "dee", half));
    let out = &events::<Withdrawn>(&meta)[0];
    eprintln!(
        "after hours: deposit refused, withdrawal paid ${:.2} ({:.4} TSLAx sold)",
        usd(out.usdc_out),
        out.asset_sold as f64 / STOCK as f64,
    );

    // Sunday 8pm: the session opens again.
    env.set_after_hours(Asset::Tsla, false);
    expect(env.deposit(Asset::Tsla, "dee", 8_000 * USDC));
}

#[test]
fn a_paused_xstock_stops_token_moves_until_backed_unpauses() {
    let mut env = Env::new();
    let eve = env.add_user("eve", 10 * SOL);
    expect(env.deposit(Asset::Tsla, "eve", 8_000 * USDC));

    // Backed can pause every TSLAx transfer (Token-2022's pausable extension).
    env.set_token_paused(Asset::Tsla, true);
    let shares = env.shares(Asset::Tsla, eve);
    assert!(
        env.withdraw(Asset::Tsla, "eve", shares).is_err(),
        "the vault cannot sell tokens that cannot move"
    );
    assert_eq!(env.shares(Asset::Tsla, eve), shares, "and burns nothing");

    env.set_token_paused(Asset::Tsla, false);
    let meta = expect(env.withdraw(Asset::Tsla, "eve", shares));
    let out = &events::<Withdrawn>(&meta)[0];
    assert_eq!(env.usdc(eve), out.usdc_out);
    assert!(usd(out.usdc_out) > 7_950.0);
}
