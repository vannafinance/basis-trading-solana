//! The ETH and BTC vaults: Phoenix does not take ETH or BTC as collateral, so the vault holds
//! the asset as tokens and posts USDC as the short's margin. At 1x margin half of a USDC
//! deposit buys the asset and half stays as margin: $35,000 at $3,500 is 5 ETH held, 5 ETH
//! short and $17,500 of margin.

use crate::env::*;
use vanna_phoenix_vault::events::{Deposited, Withdrawn};
use vanna_phoenix_vault::instructions::VaultParams;
use vanna_phoenix_vault::swap::Sell;

fn spot_params() -> VaultParams {
    VaultParams {
        margin_bps: 9_000,
        ..default_params()
    }
}

#[test]
fn usdc_in_is_half_eth_held_and_short_half_margin() {
    let mut env = Env::new();
    env.update_vault(Asset::Eth, spot_params());
    let carol = env.add_user("carol", 10 * SOL);

    let meta = env
        .deposit(Asset::Eth, "carol", 35_000 * USDC)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let e = &events::<Deposited>(&meta)[0];
    let v = env.print(Asset::Eth, "carol deposits $35,000");
    eprintln!(
        "deposit: {} compute units, {:.2} shares",
        meta.compute_units_consumed,
        usd(e.shares)
    );

    assert_eq!(e.asset_bought, 5 * ETH, "$17,500 buys 5 ETH");
    assert_eq!(v.asset_units, 5 * ETH, "the vault holds 5 ETH as tokens");
    assert_eq!(
        v.position_lots, -5_000,
        "and is short 5 ETH (5,000 lots of 0.001)"
    );
    assert!(
        v.collateral_usd > 17_490.0,
        "the other $17,500 is the short's margin on Phoenix"
    );
    assert!(
        v.nav_usd > 34_980.0 && v.nav_usd < 35_000.0,
        "nav {}",
        v.nav_usd
    );
    assert_eq!(env.usdc(carol), 0);
}

#[test]
fn the_short_follows_what_the_swap_delivered_and_the_depositor_pays_for_it() {
    let mut env = Env::new();
    env.update_vault(Asset::Eth, spot_params());
    let carol = env.add_user("carol", 10 * SOL);
    env.mint_usdc(carol, 70_000 * USDC);

    // Jupiter fills 0.5% under the mark: $17,500 buys 4.975 ETH, and the short is 4.975 ETH.
    let route = env.mock_route(Asset::Eth, Sell::Usdc, 17_500 * USDC, 4_975 * ETH / 1_000);
    let ix = env.deposit_ix_with(Asset::Eth, carol, 35_000 * USDC, 0, route);
    let meta = env
        .send("carol", vec![ix], "deposit")
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let e = &events::<Deposited>(&meta)[0];
    let v = env.print(Asset::Eth, "carol deposits $35,000, filled 0.5% under");
    assert_eq!(v.asset_units, 4_975 * ETH / 1_000);
    assert_eq!(v.position_lots, -4_975, "hedges what arrived");
    assert!(
        usd(e.value_usd) < 34_915.0,
        "the $87.50 the fill cost is carol's: {}",
        usd(e.value_usd)
    );

    // 2% under the mark is past the vault's 1% band.
    let route = env.mock_route(Asset::Eth, Sell::Usdc, 17_500 * USDC, 49 * ETH / 10);
    let ix = env.deposit_ix_with(Asset::Eth, carol, 35_000 * USDC, 0, route);
    assert!(failed_with(
        env.send("carol", vec![ix], "deposit"),
        "SwapPriceTooLow"
    ));
}

#[test]
fn eth_rallies_the_keeper_moves_value_to_margin_and_carol_exits_whole() {
    let mut env = Env::new();
    env.update_vault(Asset::Eth, spot_params());
    env.setup_keeper_accounts();
    let carol = env.add_user("carol", 10 * SOL);
    env.deposit(Asset::Eth, "carol", 35_000 * USDC).unwrap();
    let start = env.print(Asset::Eth, "carol deposits $35,000");

    env.move_price(Asset::Eth, 4_550.0);
    let up = env.print(Asset::Eth, "ETH +30% to $4,550");
    assert!((up.nav_usd - start.nav_usd).abs() < 1.0, "flat in USD");
    let ratio = up.effective_usd / (-up.position_lots as f64 / 1_000.0 * up.mark_usd);
    eprintln!("margin is down to {:.0}% of the short", ratio * 100.0);
    assert!(ratio < 0.6);

    // Back to 1x: hold = short = NAV / (2 x price). The keeper buys the extra ETH for USDC.
    let target = up.nav_usd / (2.0 * up.mark_usd);
    let sell = ((5.0 - target) * ETH as f64) as u64;
    let usdc = (sell as f64 / ETH as f64 * up.mark_usd * USDC as f64) as u64;
    let keeper = env.keeper;
    env.mint_usdc(keeper, usdc);
    env.rebalance(Asset::Eth, false, sell, usdc)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let topped = env.print(Asset::Eth, "keeper buys ETH, vault margin back to 1x");
    let ratio = topped.effective_usd / (-topped.position_lots as f64 / 1_000.0 * topped.mark_usd);
    eprintln!(
        "margin back to {:.0}% of the short; keeper holds {:.4} ETH",
        ratio * 100.0,
        env.asset_balance(Asset::Eth, keeper) as f64 / ETH as f64
    );
    assert!(ratio > 0.99);
    assert!(
        (topped.nav_usd - up.nav_usd).abs() < 5.0,
        "a fair swap leaves NAV in place"
    );

    let meta = env
        .withdraw(Asset::Eth, "carol", env.shares(Asset::Eth, carol))
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let out = &events::<Withdrawn>(&meta)[0];
    let end = env.print(Asset::Eth, "carol withdraws everything");
    eprintln!(
        "carol: put in $35,000, took out ${:.2} USDC ({:.4} ETH sold, {} compute units)",
        usd(out.usdc_out),
        out.asset_sold as f64 / ETH as f64,
        meta.compute_units_consumed
    );
    assert_eq!(env.usdc(carol), out.usdc_out, "paid in USDC");
    assert_eq!(env.asset_balance(Asset::Eth, carol), 0, "and no ETH");
    assert!(
        usd(out.usdc_out) > 34_900.0,
        "payout {}",
        usd(out.usdc_out)
    );
    assert_eq!(end.supply, 0);
    assert_eq!(end.position_lots, 0);
}

#[test]
fn btc_vault_two_users_and_a_drop() {
    let mut env = Env::new();
    env.update_vault(Asset::Btc, spot_params());
    let dan = env.add_user("dan", 10 * SOL);
    env.add_user("erin", 10 * SOL);
    env.deposit(Asset::Btc, "dan", 100_000 * USDC)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    env.print(Asset::Btc, "dan deposits $100,000");

    env.move_price(Asset::Btc, 90_000.0);
    let down = env.print(Asset::Btc, "BTC -10% to $90,000");
    env.deposit(Asset::Btc, "erin", 18_000 * USDC).unwrap();
    let both = env.print(Asset::Btc, "erin deposits $18,000");
    assert!((both.share_price() - down.share_price()).abs() < 1e-5);
    assert_eq!(both.position_lots, -6_000, "0.6 BTC held, 0.6 BTC short");

    let half = env.shares(Asset::Btc, dan) / 2;
    let supply = both.supply;
    let meta = env
        .withdraw(Asset::Btc, "dan", half)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let out = &events::<Withdrawn>(&meta)[0];
    let after = env.print(Asset::Btc, "dan withdraws half");
    eprintln!(
        "dan's half: {:.4} BTC sold, ${:.2} USDC",
        out.asset_sold as f64 / BTC as f64,
        usd(out.usdc_out),
    );
    // Every share holds the same mix of BTC and USDC, so dan sells his shares' part of the BTC.
    assert_eq!(out.asset_sold, (6 * BTC / 10) * half / supply);
    assert!(usd(out.usdc_out) > 49_850.0 && usd(out.usdc_out) < 50_000.0);
    assert!(after.share_price() >= both.share_price() - 1e-6);
    assert_eq!(
        after.position_lots,
        -((after.asset_units as i64 + 5_000) / 10_000),
        "still hedged"
    );
}

#[test]
fn a_phoenix_deposit_cooldown_would_delay_usdc_withdrawals() {
    // Phoenix can block a trader's USDC withdrawals for a while after a USDC deposit. It is
    // off on mainnet today; this is what happens if it is turned on.
    let mut env = Env::new();
    env.update_vault(Asset::Eth, spot_params());
    set_deposit_cooldown(&mut env.ctx, 150);
    let carol = env.add_user("carol", 10 * SOL);
    env.add_user("alice", 10 * SOL);
    env.deposit(Asset::Eth, "carol", 35_000 * USDC).unwrap();
    let half = env.shares(Asset::Eth, carol) / 2;
    assert!(failed_with(
        env.withdraw(Asset::Eth, "carol", half),
        "deposit cooldown"
    ));
    env.warp_slots(150);
    env.withdraw(Asset::Eth, "carol", half)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));

    // The SOL vault posts no USDC (all of a deposit buys SOL) and pays out SOL sold through
    // Jupiter, so nothing holds it back.
    env.deposit(Asset::Sol, "alice", 1_500 * USDC).unwrap();
    let shares = env.shares(Asset::Sol, env.ctx.signer_pubkey("alice"));
    env.withdraw(Asset::Sol, "alice", shares / 2)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
}

#[test]
fn the_keeper_reinvests_within_its_band_and_frank_exits_in_usdc() {
    let mut env = Env::new();
    env.update_vault(Asset::Eth, spot_params());
    env.setup_keeper_accounts();
    let frank = env.add_user("frank", 10 * SOL);
    let meta = env
        .deposit(Asset::Eth, "frank", 35_000 * USDC)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let e = &events::<Deposited>(&meta)[0];
    env.print(Asset::Eth, "frank deposits $35,000");
    eprintln!("frank: {:.2} shares", usd(e.shares));

    // The keeper takes 0.1 ETH off the vault for $350 of margin, within its band. Selling the
    // vault 3 ETH for $10,500 of its margin would leave it under its 90% floor: refused.
    let keeper = env.keeper;
    env.mint_asset(Asset::Eth, keeper, 3 * ETH);
    env.mint_usdc(keeper, 1_000 * USDC);
    env.rebalance(Asset::Eth, false, ETH / 10, 350 * USDC)
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    assert!(failed_with(
        env.rebalance(Asset::Eth, true, 3 * ETH, 10_500 * USDC),
        "MarginTooThin"
    ));
    let hedged = env.print(Asset::Eth, "keeper rebalances");
    assert_eq!(hedged.asset_units, 5 * ETH - ETH / 10);
    assert_eq!(hedged.position_lots, -4_900);

    env.move_price(Asset::Eth, 4_000.0);
    env.print(Asset::Eth, "ETH +14% to $4,000");
    let meta = env
        .withdraw(Asset::Eth, "frank", env.shares(Asset::Eth, frank))
        .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    let out = &events::<Withdrawn>(&meta)[0];
    env.print(Asset::Eth, "frank withdraws everything");
    eprintln!(
        "frank: put in $35,000, took out ${:.2} USDC at $4,000 ({:.4} ETH sold)",
        usd(out.usdc_out),
        out.asset_sold as f64 / ETH as f64,
    );
    assert!(usd(out.usdc_out) > 34_900.0);
}
