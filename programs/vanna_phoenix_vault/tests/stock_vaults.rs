//! Tokenized-stock vaults: TSLA and META, held as Backed's xStocks (Token-2022) and hedged on
//! Phoenix's TSLA and META perps. The tokens are the mainnet mints with their real extensions;
//! single-asset deposits and withdrawals run real Jupiter routes over Raydium's TSLAx and METAx
//! pools.

use crate::env::*;
use vanna_phoenix_vault::events::{Deposited, Swapped, Withdrawn};
use vanna_phoenix_vault::instructions::VaultParams;
use vanna_phoenix_vault::swap::{Receive, Sell};

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
    env.mint_usdc(ann, 10_000 * USDC);
    let quoted = env
        .jupiter_route_for(Asset::Tsla, Sell::Usdc, 5_000 * USDC)
        .quoted_out;

    let meta = expect(env.deposit_single(Asset::Tsla, "ann", 0, 10_000 * USDC));
    let swap = &events::<Swapped>(&meta)[0];
    let dep = &events::<Deposited>(&meta)[0];
    let v = env.print(Asset::Tsla, "ann deposits $10,000 USDC");
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

    // Out again in USDC only: the vault sells ann's TSLAx through Jupiter.
    let shares = env.shares(Asset::Tsla, ann);
    let meta = expect(env.withdraw_single(Asset::Tsla, "ann", shares, Receive::Usdc));
    let out = &events::<Withdrawn>(&meta)[0];
    let v = env.print(Asset::Tsla, "ann withdraws everything in USDC");
    // The client asks Jupiter to sell a hair less than the vault will pay out (0.05%); the
    // vault pays that remainder as TSLAx.
    let left = env.token(ann, TSLA_MINT);
    let back = usd(env.usdc(ann)) + left as f64 / STOCK as f64 * v.mark_usd;
    eprintln!(
        "ann: $10,000.00 in, ${:.2} USDC + {:.6} TSLAx = ${back:.2} out ({:.3}% for two swaps and two trades)",
        usd(env.usdc(ann)),
        left as f64 / STOCK as f64,
        (1.0 - back / 10_000.0) * 100.0
    );
    assert_eq!(out.asset_out, left);
    assert!(left < swap.received / 1_000);
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
    env.mint_asset(Asset::Meta, bob, 10 * STOCK);
    env.mint_usdc(bob, 7_500 * USDC);
    let meta = expect(env.deposit(Asset::Meta, "bob", 10 * STOCK, 7_500 * USDC));
    let dep = &events::<Deposited>(&meta)[0];
    let v = env.print(Asset::Meta, "bob deposits 10 METAx + $7,500");
    // 10 tokens are 10.0285 shares: the short is 10.029 META, not 10.
    assert_eq!(-v.position_lots, hedge_lots(&env, Asset::Meta, 10 * STOCK));
    assert!(-v.position_lots > 10_025);
    let tokens_usd = 10.0 * v.mark_usd * multiplier;
    eprintln!(
        "10 METAx at ${:.2} x {multiplier:.6} = ${tokens_usd:.2}; deposit valued ${:.2}",
        v.mark_usd,
        usd(dep.value_usd)
    );
    assert!((usd(dep.value_usd) - (tokens_usd + 7_500.0)).abs() < 10.0);

    // The price moves; the vault does not.
    env.move_price(Asset::Meta, 820.0);
    let up = env.print(Asset::Meta, "META +10%");
    assert!((up.nav_usd - v.nav_usd).abs() < v.nav_usd * 0.001);

    // Bob takes his part back as METAx and USDC: a Token-2022 transfer out of the vault.
    let shares = env.shares(Asset::Meta, bob);
    let meta = expect(env.withdraw(Asset::Meta, "bob", shares));
    let out = &events::<Withdrawn>(&meta)[0];
    env.print(Asset::Meta, "bob withdraws everything");
    // His 10 METAx back, and less USDC: the short paid for META's rise, which the tokens kept.
    let worth = 10.0 * 820.0 * multiplier + usd(env.usdc(bob));
    eprintln!(
        "bob gets {:.4} METAx + ${:.2} = ${worth:.2} at $820, for ${:.2} put in",
        out.asset_out as f64 / STOCK as f64,
        usd(out.usdc_out),
        usd(dep.value_usd)
    );
    assert_eq!(env.token(bob, META_MINT), 10 * STOCK);
    assert!((worth - usd(dep.value_usd)).abs() < 20.0);
}

#[test]
fn meta_tokens_alone_sell_their_margin_through_jupiter() {
    let mut env = Env::new();
    env.update_vault(Asset::Meta, stock_params());
    env.use_jupiter();
    let cy = env.add_user("cy", 10 * SOL);
    env.mint_asset(Asset::Meta, cy, 10 * STOCK);
    let meta = expect(env.deposit_single(Asset::Meta, "cy", 10 * STOCK, 0));
    let swap = &events::<Swapped>(&meta)[0];
    let v = env.print(Asset::Meta, "cy deposits 10 METAx alone");
    eprintln!(
        "METAx -> USDC: {:.4} METAx sold for ${:.2}, {} CU",
        swap.spent as f64 / STOCK as f64,
        usd(swap.received),
        meta.compute_units_consumed
    );
    assert_eq!(swap.spent, 5 * STOCK, "half the tokens become margin");
    assert_eq!(v.asset_units, 5 * STOCK);
    assert_eq!(-v.position_lots, hedge_lots(&env, Asset::Meta, 5 * STOCK));
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
    env.mint_asset(Asset::Tsla, dee, 20 * STOCK);
    env.mint_usdc(dee, 8_000 * USDC);
    expect(env.deposit(Asset::Tsla, "dee", 10 * STOCK, 4_000 * USDC));

    // Friday 8pm New York: Phoenix holds TSLA after hours until Sunday night.
    env.set_after_hours(Asset::Tsla, true);
    assert!(failed_with(
        env.deposit(Asset::Tsla, "dee", 10 * STOCK, 4_000 * USDC),
        "MarketAfterHours"
    ));
    let half = env.shares(Asset::Tsla, dee) / 2;
    let meta = expect(env.withdraw(Asset::Tsla, "dee", half));
    let out = &events::<Withdrawn>(&meta)[0];
    eprintln!(
        "after hours: deposit refused, withdrawal paid {:.4} TSLAx + ${:.2}",
        out.asset_out as f64 / STOCK as f64,
        usd(out.usdc_out)
    );

    // Sunday 8pm: the session opens again.
    env.set_after_hours(Asset::Tsla, false);
    expect(env.deposit(Asset::Tsla, "dee", 10 * STOCK, 4_000 * USDC));
}

#[test]
fn a_paused_xstock_stops_token_moves_until_backed_unpauses() {
    let mut env = Env::new();
    let eve = env.add_user("eve", 10 * SOL);
    env.mint_asset(Asset::Tsla, eve, 10 * STOCK);
    env.mint_usdc(eve, 4_000 * USDC);
    expect(env.deposit(Asset::Tsla, "eve", 10 * STOCK, 4_000 * USDC));

    // Backed can pause every TSLAx transfer (Token-2022's pausable extension).
    env.set_token_paused(Asset::Tsla, true);
    let shares = env.shares(Asset::Tsla, eve);
    assert!(
        env.withdraw(Asset::Tsla, "eve", shares).is_err(),
        "the vault cannot pay out tokens that cannot move"
    );
    assert_eq!(env.shares(Asset::Tsla, eve), shares, "and burns nothing");

    env.set_token_paused(Asset::Tsla, false);
    expect(env.withdraw(Asset::Tsla, "eve", shares));
    assert_eq!(env.token(eve, TSLA_MINT), 10 * STOCK);
}
