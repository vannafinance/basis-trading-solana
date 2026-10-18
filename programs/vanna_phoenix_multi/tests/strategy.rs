//! The keeper's strategy: spreading the vault over Phoenix's markets, rotating between them as
//! funding moves, reinvesting funding and keeping the shared margin.

use crate::env::*;
use vanna_phoenix_multi_vault::events::{Deployed, Unwound};
use vanna_phoenix_multi_vault::swap::Sell;

fn expect<T>(result: Result<T, Vec<String>>) -> T {
    result.unwrap_or_else(|logs| panic!("{}", logs.join("\n")))
}

/// Every leg's short matches its asset to the lot.
fn assert_hedged(env: &Env, v: &VaultView) {
    for asset in Asset::ALL {
        let l = v.leg(asset);
        let lot = 1.0 / 10f64.powi(env.leg(asset).base_lot_decimals as i32);
        assert!(
            (l.held(asset) - l.short(env, asset)).abs() <= lot,
            "{}: {} held, {} short",
            asset.symbol(),
            l.held(asset),
            l.short(env, asset)
        );
    }
}

#[test]
fn the_keeper_spreads_the_vault_over_five_hedged_legs() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", 200_000 * USDC));
    let route = env.route_for(Asset::Sol, Sell::Usdc, 40_000 * USDC);
    let meta = expect(env.deploy_with(env.keeper_seed(), Asset::Sol, 40_000 * USDC, route));
    let d = &events::<Deployed>(&meta)[0];
    eprintln!(
        "SOL deploy: $40,000 bought {:.4} SOL, short {} lots, {} compute units",
        d.asset_bought as f64 / SOL as f64,
        -d.position_lots,
        meta.compute_units_consumed
    );
    assert_eq!(d.usdc_spent, 40_000 * USDC);
    for (asset, usd) in [
        (Asset::Eth, 60_000),
        (Asset::Btc, 40_000),
        (Asset::Tsla, 20_000),
        (Asset::Meta, 20_000),
    ] {
        env.deploy_hedged(asset, usd * USDC);
    }
    let v = env.print("$200,000: SOL 20%, ETH 30%, BTC 20%, TSLA 10%, META 10%");
    assert_hedged(&env, &v);
    assert!((v.leg(Asset::Sol).held(Asset::Sol) - 266.6666).abs() < 0.001);
    assert!((v.leg(Asset::Eth).value_usd - 30_000.0).abs() < 1.0);
    assert_eq!(v.idle_usd, 20_000.0, "10% stays in the buffer");
    let cost = 200_000.0 - v.nav_usd;
    eprintln!("deploying $180,000 cost ${cost:.2} ({:.3}%)", cost / 1_800.0);
    assert!(cost > 0.0 && cost < 180.0, "less than 0.1%: {cost}");
    // The SOL is margin too: every short is covered more than 1x by Phoenix's measure.
    assert!(v.margin_ratio() > 0.9, "{}", v.margin_ratio());
}

#[test]
fn the_keeper_rotates_out_of_eth_into_tsla_when_funding_moves() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", 100_000 * USDC));
    env.deploy_hedged(Asset::Sol, 30_000 * USDC);
    env.deploy_hedged(Asset::Eth, 40_000 * USDC);
    env.deploy_hedged(Asset::Tsla, 20_000 * USDC);
    let start = env.print("SOL $30,000, ETH $40,000, TSLA $20,000");

    // ETH's perp trades under spot (shorts pay); TSLA's over it (shorts earn).
    env.set_perp_premium(Asset::Eth, 3_486.0, 3_500.0);
    env.set_perp_premium(Asset::Tsla, 385.0, 382.0);
    env.warp_hours(8);
    let skewed = env.print("8 hours: ETH funding negative, TSLA positive");

    // The keeper sells all of the ETH and puts the USDC into TSLA; ETH's margin stays on
    // Phoenix and now backs TSLA's bigger short (one cross-margin account).
    let eth = skewed.leg(Asset::Eth).units;
    let meta = expect(env.unwind(Asset::Eth, eth));
    let u = &events::<Unwound>(&meta)[0];
    eprintln!(
        "unwound {:.4} ETH for ${:.2}",
        u.asset_sold as f64 / ETH as f64,
        usd(u.usdc_received)
    );
    assert_eq!(u.position_lots, 0, "the ETH short is closed");
    let freed = u.usdc_received;
    expect(env.deploy(Asset::Tsla, freed));
    let rotated = env.print("ETH -> TSLA");
    assert_hedged(&env, &rotated);
    assert_eq!(rotated.leg(Asset::Eth).units, 0);
    assert!(rotated.leg(Asset::Tsla).value_usd > 29_000.0);
    assert!(
        (rotated.nav_usd - skewed.nav_usd).abs() < skewed.nav_usd * 0.002,
        "a rotation costs a little: {} -> {}",
        skewed.nav_usd,
        rotated.nav_usd
    );
    assert!(rotated.margin_ratio() > 0.85, "{}", rotated.margin_ratio());
    // Today's deploys ($30,000 + $20,000 + $10,000), the ETH sold and the TSLA bought.
    let turnover = usd(env.vault_account().turnover_window_usd);
    eprintln!("keeper turnover today: ${turnover:.2}");
    assert!((turnover - (60_000.0 + 2.0 * usd(freed))).abs() < 50.0, "{turnover}");
    assert!(start.nav_usd > rotated.nav_usd - 100.0);
}

#[test]
fn funding_lifts_the_share_price_and_the_keeper_reinvests_it() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", 100_000 * USDC));
    env.deploy_hedged(Asset::Sol, 40_000 * USDC);
    env.deploy_hedged(Asset::Tsla, 40_000 * USDC);
    let start = env.print("SOL $40,000, TSLA $40,000");

    // Longs pay: both perps trade 1% over spot for a day.
    env.set_perp_premium(Asset::Sol, 151.5, 150.0);
    env.set_perp_premium(Asset::Tsla, 385.82, 382.0);
    env.warp_hours(24);
    let paid = env.print("a day of funding on SOL and TSLA");
    assert!(paid.funding_usd > 50.0, "shorts earned funding: {}", paid.funding_usd);
    assert!(paid.share_price() > start.share_price());
    eprintln!(
        "share price {:.6} -> {:.6} (+{:.3}% in a day)",
        start.share_price(),
        paid.share_price(),
        (paid.share_price() / start.share_price() - 1.0) * 100.0
    );

    // The keeper takes the funding out of Phoenix and buys more SOL with it.
    let funding = (paid.funding_usd * USDC as f64) as u64;
    expect(env.move_margin(false, funding));
    expect(env.deploy(Asset::Sol, funding));
    let reinvested = env.print("keeper reinvests the funding into SOL");
    assert_hedged(&env, &reinvested);
    assert!(reinvested.leg(Asset::Sol).units > paid.leg(Asset::Sol).units);
    assert!(
        (reinvested.nav_usd - paid.nav_usd).abs() < 1.0,
        "a fair swap leaves NAV in place: {} -> {}",
        paid.nav_usd,
        reinvested.nav_usd
    );
    env.warp_hours(24);
    let compounding = env.print("another day, on the bigger SOL short");
    assert!(compounding.share_price() > reinvested.share_price());
}

#[test]
fn shorts_pay_when_perps_trade_below_spot() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", 50_000 * USDC));
    env.deploy_hedged(Asset::Eth, 40_000 * USDC);
    let start = env.view();
    env.set_perp_premium(Asset::Eth, 3_465.0, 3_500.0);
    env.warp_hours(24);
    let paid = env.print("a day of negative ETH funding");
    assert!(paid.funding_usd < 0.0);
    assert!(paid.share_price() < start.share_price(), "negative funding is the strategy's risk");
}

#[test]
fn a_rally_thins_the_shared_margin_and_the_keeper_restores_it() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", 120_000 * USDC));
    env.deploy_hedged(Asset::Eth, 50_000 * USDC);
    env.deploy_hedged(Asset::Btc, 50_000 * USDC);
    let start = env.print("ETH and BTC at 1x, $20,000 buffer");
    assert!(start.margin_ratio() > 0.99);

    env.move_price(Asset::Eth, 4_900.0);
    let rally = env.print("ETH +40%");
    eprintln!("margin {:.1}% after the rally", rally.margin_ratio() * 100.0);
    assert!(rally.margin_ratio() < 0.8, "the ETH short's loss is everyone's margin");
    assert!((rally.nav_usd - start.nav_usd).abs() < 20.0, "but the vault is flat");

    // The keeper sells the ETH that rallied past 1x and posts the USDC as margin.
    let eth = rally.leg(Asset::Eth);
    let target = (rally.nav_usd - rally.idle_usd) / 2.0 / 2.0 / eth.mark_usd;
    let sell = ((eth.held(Asset::Eth) - target) * ETH as f64) as u64;
    let meta = expect(env.unwind(Asset::Eth, sell));
    let freed = events::<Unwound>(&meta)[0].usdc_received;
    expect(env.move_margin(true, freed));
    let restored = env.print("keeper sells ETH into margin");
    assert_hedged(&env, &restored);
    eprintln!("margin back to {:.1}%", restored.margin_ratio() * 100.0);
    assert!(restored.margin_ratio() > 0.95);
    assert!((restored.nav_usd - rally.nav_usd).abs() < 20.0);
}

#[test]
fn the_sol_leg_leaves_phoenix_when_it_is_unwound() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", 30_000 * USDC));
    env.deploy_hedged(Asset::Sol, 15_000 * USDC);
    let start = env.print("100 SOL as Phoenix collateral, 100 SOL short");
    let keeper_sol = env.ctx.svm.get_balance(&env.keeper).unwrap();
    let meta = expect(env.unwind(Asset::Sol, 40 * SOL));
    let u = &events::<Unwound>(&meta)[0];
    let after = env.print("keeper unwinds 40 SOL");
    eprintln!(
        "40 SOL out of Phoenix for ${:.2}, {} compute units",
        usd(u.usdc_received),
        meta.compute_units_consumed
    );
    assert_eq!(after.leg(Asset::Sol).units, start.leg(Asset::Sol).units - 40 * SOL);
    assert_eq!(after.leg(Asset::Sol).position_lots, -6_000, "60 SOL short");
    assert_hedged(&env, &after);
    assert!((usd(u.usdc_received) - 6_000.0).abs() < 1.0);
    assert!(
        keeper_sol - env.ctx.svm.get_balance(&env.keeper).unwrap() <= meta.fee,
        "the wrapped-SOL account's rent comes back to the keeper"
    );
}

#[test]
fn the_keeper_rehedges_a_donation() {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    env.add_user("donor", 10 * SOL);
    expect(env.deposit("alice", 20_000 * USDC));
    env.deploy_hedged(Asset::Eth, 14_000 * USDC);
    let before = env.view();
    env.donate_asset(Asset::Eth, "donor", ETH);
    let donated = env.view();
    assert_eq!(donated.leg(Asset::Eth).position_lots, before.leg(Asset::Eth).position_lots);
    assert!(donated.nav_usd > before.nav_usd + 3_400.0);
    expect(env.rehedge(Asset::Eth));
    let after = env.print("keeper re-hedges a 1 ETH donation");
    assert_hedged(&env, &after);
    // 2 ETH bought and 1 donated: 3 ETH short.
    assert_eq!(after.leg(Asset::Eth).position_lots, -3_000);
}

#[test]
fn transaction_sizes_and_compute_with_five_legs() {
    let mut env = Env::new();
    let alice = env.add_user("alice", 10 * SOL);
    env.mint_usdc(alice, 50_000 * USDC);
    let budget = solana_compute_budget_interface::ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
    let report = |label: &str, payer, ix: anchor_lang::solana_program::instruction::Instruction, cu: u64| {
        let (accounts, legacy, v0) = transaction_sizes(payer, &[budget.clone(), ix]);
        eprintln!("{label:<24} {accounts:>2} accounts | legacy {legacy:>4} bytes | with a lookup table {v0:>4} bytes | {cu:>7} CU");
        assert!(accounts <= 64, "Solana's account limit");
        assert!(v0 <= 1_232, "fits with a lookup table");
        assert!(cu < 1_400_000);
    };
    let ix = env.deposit_ix(alice, 50_000 * USDC, 0);
    let cu = expect(env.send("alice", vec![ix.clone()], "deposit")).compute_units_consumed;
    report("deposit", alice, ix, cu);
    env.deploy_everywhere(30_000 * USDC);
    let keeper = env.keeper;
    for asset in [Asset::Sol, Asset::Eth, Asset::Meta] {
        let route = env.route_for(asset, Sell::Usdc, 1_000 * USDC);
        let ix = env.deploy_ix(keeper, asset, 1_000 * USDC, route);
        let cu = expect(env.send(env.keeper_seed(), vec![ix.clone()], "deploy")).compute_units_consumed;
        report(&format!("deploy {}", asset.symbol()), keeper, ix, cu);
    }
    let units = env.view().leg(Asset::Sol).units / 10;
    let route = env.route_for(Asset::Sol, Sell::Asset, units);
    let ix = env.unwind_ix(keeper, Asset::Sol, units, route);
    let cu = expect(env.send(env.keeper_seed(), vec![ix.clone()], "unwind")).compute_units_consumed;
    report("unwind SOL", keeper, ix, cu);
    let ix = env.withdraw_ix(alice, 1_000 * USDC, 0);
    let cu = expect(env.send("alice", vec![ix.clone()], "withdraw")).compute_units_consumed;
    report("withdraw", alice, ix, cu);
    // A real Jupiter route adds about 15 more accounts than the mock's 10: a deploy with five
    // legs stays under 64.
}

#[test]
fn a_vault_with_all_eight_legs_fits_solanas_limits() {
    let mut env = Env::with_first_legs(8);
    let alice = env.add_user("alice", 10 * SOL);
    env.mint_usdc(alice, 200_000 * USDC);
    let budget = solana_compute_budget_interface::ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
    let mut worst = (0usize, 0u64);
    let mut report = |label: &str, payer, ix: anchor_lang::solana_program::instruction::Instruction, cu: u64| {
        let (accounts, _, v0) = transaction_sizes(payer, &[budget.clone(), ix]);
        eprintln!("{label:<16} {accounts:>2} accounts | with a lookup table {v0:>4} bytes | {cu:>7} CU");
        assert!(v0 <= 1_232);
        worst = (worst.0.max(accounts), worst.1.max(cu));
    };
    let ix = env.deposit_ix(alice, 200_000 * USDC, 0);
    let cu = expect(env.send("alice", vec![ix.clone()], "deposit")).compute_units_consumed;
    report("deposit", alice, ix, cu);
    let keeper = env.keeper;
    for asset in Asset::EVERY {
        if asset != Asset::Sol {
            expect(env.move_margin(true, 10_000 * USDC));
        }
        let route = env.route_for(asset, Sell::Usdc, 10_000 * USDC);
        let ix = env.deploy_ix(keeper, asset, 10_000 * USDC, route);
        let cu = expect(env.send(env.keeper_seed(), vec![ix.clone()], "deploy")).compute_units_consumed;
        report(&format!("deploy {}", asset.symbol()), keeper, ix, cu);
    }
    let v = env.print("eight legs");
    for asset in Asset::EVERY {
        let l = v.leg(asset);
        let lot = 1.0 / 10f64.powi(env.leg(asset).base_lot_decimals as i32);
        assert!((l.held(asset) - l.short(&env, asset)).abs() <= lot, "{} hedged", asset.symbol());
    }
    let units = v.leg(Asset::Spy).units / 2;
    let route = env.route_for(Asset::Spy, Sell::Asset, units);
    let ix = env.unwind_ix(keeper, Asset::Spy, units, route);
    let cu = expect(env.send(env.keeper_seed(), vec![ix.clone()], "unwind")).compute_units_consumed;
    report("unwind SPY", keeper, ix, cu);
    let ix = env.withdraw_ix(alice, 1_000 * USDC, 0);
    let cu = expect(env.send("alice", vec![ix.clone()], "withdraw")).compute_units_consumed;
    report("withdraw", alice, ix, cu);
    eprintln!("worst: {} accounts with a 10-account mock route, {} CU", worst.0, worst.1);
    // A direct Jupiter route is about 15 accounts more than the mock's.
    assert!(worst.0 + 15 <= 64, "a deploy with a real route fits Solana's 64 accounts");
    assert!(worst.1 < 1_000_000, "well under 1.4M compute units");
}
