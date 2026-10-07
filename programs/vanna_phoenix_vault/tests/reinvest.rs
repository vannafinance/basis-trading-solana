//! Funding: Phoenix pays it to the shorts in USDC whenever the perp trades above spot. The NAV
//! counts it as soon as it accrues; the keeper's rebalance turns it into more of the asset
//! and a bigger short, so later funding is earned on it too.

use crate::env::*;
use vanna_phoenix_vault::events::Rebalanced;

#[test]
fn funding_raises_the_share_price_and_the_keeper_reinvests_it() {
    let mut env = Env::new();
    env.setup_keeper_accounts();
    env.add_user("alice", 10 * SOL);
    env.deposit(Asset::Sol, "alice", 15_000 * USDC).unwrap();
    env.print(Asset::Sol, "alice deposits $15,000 (100 SOL)");

    // Phoenix never lets SOL collateral back a USDC withdrawal, so the SOL vault keeps a USDC
    // buffer above the short's initial margin. The keeper sets it up by buying 8 SOL.
    let keeper = env.keeper;
    env.mint_usdc(keeper, 1_200 * USDC);
    env.rebalance(Asset::Sol, false, 8 * SOL, 1_200 * USDC)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let start = env.print(Asset::Sol, "keeper sets up a $1,200 USDC buffer");

    // Longs pay: the perp trades 1% over spot.
    env.set_perp_premium(Asset::Sol, 151.5, 150.0);
    env.warp_hours(24);
    let paid = env.print(Asset::Sol, "a day of funding later");
    assert!(
        paid.funding_usd > 40.0,
        "shorts earned funding: ${}",
        paid.funding_usd
    );
    assert!(paid.share_price() > start.share_price());
    eprintln!(
        "share price {:.6} -> {:.6} (+{:.3}% in a day)",
        start.share_price(),
        paid.share_price(),
        (paid.share_price() / start.share_price() - 1.0) * 100.0
    );

    // The keeper sells the vault SOL worth the funding, at the mark price.
    let lamports = (paid.funding_usd / paid.mark_usd * SOL as f64) as u64;
    let usdc = (lamports as f64 / SOL as f64 * paid.mark_usd * USDC as f64) as u64;
    let keeper_usdc = env.usdc(keeper);
    let meta = env
        .rebalance(Asset::Sol, true, lamports, usdc)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let r = &events::<Rebalanced>(&meta)[0];
    let reinvested = env.print(Asset::Sol, "keeper reinvests the funding");
    eprintln!(
        "reinvest: {:.4} SOL in, ${:.2} out to the keeper, NAV ${:.2} -> ${:.2}",
        lamports as f64 / SOL as f64,
        usd(usdc),
        usd(r.nav_before_usd),
        usd(r.nav_after_usd)
    );
    assert_eq!(
        env.usdc(keeper) - keeper_usdc,
        usdc,
        "the keeper got USDC from Phoenix"
    );
    assert!(
        reinvested.asset_units > paid.asset_units,
        "the vault holds more SOL"
    );
    assert!(
        reinvested.position_lots < paid.position_lots,
        "and is short more of it"
    );
    assert_eq!(
        -reinvested.position_lots as u64,
        (reinvested.asset_units + 5_000_000) / 10_000_000
    );
    assert!(
        (reinvested.nav_usd - paid.nav_usd).abs() < 1.0,
        "a fair swap leaves NAV in place"
    );

    env.warp_hours(24);
    let compounding = env.print(Asset::Sol, "another day, on the bigger short");
    assert!(compounding.share_price() > reinvested.share_price());
}

#[test]
fn spot_vault_funding_is_reinvested_into_eth() {
    let mut env = Env::new();
    env.update_vault(
        Asset::Eth,
        vanna_phoenix_vault::instructions::VaultParams {
            margin_bps: 9_000,
            ..default_params()
        },
    );
    env.setup_keeper_accounts();
    env.add_user("carol", 10 * SOL);
    env.deposit(Asset::Eth, "carol", 70_000 * USDC).unwrap();
    let start = env.print(Asset::Eth, "carol deposits $70,000 (10 ETH + margin)");

    env.set_perp_premium(Asset::Eth, 3_535.0, 3_500.0);
    env.warp_hours(72);
    let paid = env.print(Asset::Eth, "three days of funding");
    assert!(paid.share_price() > start.share_price());

    let keeper = env.keeper;
    let eth = (paid.funding_usd / paid.mark_usd * ETH as f64) as u64;
    let usdc = (eth as f64 / ETH as f64 * paid.mark_usd * USDC as f64) as u64;
    env.mint_asset(Asset::Eth, keeper, eth);
    env.rebalance(Asset::Eth, true, eth, usdc)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let reinvested = env.print(Asset::Eth, "keeper reinvests the funding into ETH");
    eprintln!(
        "reinvest: {:.5} ETH in for ${:.2}",
        eth as f64 / ETH as f64,
        usd(usdc)
    );
    assert!(reinvested.asset_units > 10 * ETH);
    assert!(reinvested.position_lots < -10_000);
    assert!((reinvested.nav_usd - paid.nav_usd).abs() < 2.0);
}

#[test]
fn shorts_pay_when_the_perp_trades_below_spot() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    env.deposit(Asset::Sol, "alice", 15_000 * USDC).unwrap();
    let start = env.print(Asset::Sol, "alice deposits $15,000 (100 SOL)");
    env.set_perp_premium(Asset::Sol, 148.5, 150.0);
    env.warp_hours(24);
    let paid = env.print(Asset::Sol, "a day of negative funding");
    assert!(paid.funding_usd < 0.0);
    assert!(
        paid.share_price() < start.share_price(),
        "negative funding is the strategy's risk"
    );
}

#[test]
fn only_the_keeper_rebalances_and_only_near_the_mark() {
    let mut env = Env::new();
    env.setup_keeper_accounts();
    env.add_user("alice", 10 * SOL);
    env.deposit(Asset::Sol, "alice", 15_000 * USDC).unwrap();
    env.mint_usdc(env.keeper, 10_000 * USDC);

    assert!(failed_with(
        env.rebalance_as("alice", Asset::Sol, true, SOL, 150 * USDC),
        "Unauthorized"
    ));
    // Paying $160 for 1 SOL marked at $150 would cost the vault: refused.
    assert!(failed_with(
        env.rebalance(Asset::Sol, true, SOL, 160 * USDC),
        "PriceOutOfBand"
    ));
    // Selling 1 SOL for $140 would too.
    assert!(failed_with(
        env.rebalance(Asset::Sol, false, SOL, 140 * USDC),
        "PriceOutOfBand"
    ));
    // At the mark it goes through either way.
    env.rebalance(Asset::Sol, false, SOL, 150 * USDC)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let v = env.print(Asset::Sol, "keeper bought 1 SOL for $150");
    assert_eq!(v.position_lots, -9_900);
}
