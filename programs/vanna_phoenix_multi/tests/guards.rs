//! The limits on the keeper and the admin, and what a swap may touch. Each test tries what a
//! wrong or stolen key could, and checks the program refuses it.

use crate::env::*;
use anchor_lang::prelude::Pubkey;
use anchor_spl::associated_token::get_associated_token_address;
use vanna_phoenix_multi_vault::constants::NATIVE_MINT;
use vanna_phoenix_multi_vault::instructions::{LegParams, MarketParams, VaultParams};
use vanna_phoenix_multi_vault::state::LegKind;
use vanna_phoenix_multi_vault::swap::Sell;

fn expect<T>(result: Result<T, Vec<String>>) -> T {
    result.unwrap_or_else(|logs| panic!("{}", logs.join("\n")))
}

/// A vault with `usd` deposited by alice.
fn funded(usd: u64) -> Env {
    let mut env = Env::new();
    env.add_user("alice", 10 * SOL);
    expect(env.deposit("alice", usd * USDC));
    env
}

#[test]
fn only_the_keeper_or_the_admin_moves_the_vault() {
    let mut env = funded(20_000);
    env.deploy_hedged(Asset::Eth, 10_000 * USDC);
    let route = env.route_for(Asset::Sol, Sell::Usdc, 1_000 * USDC);
    assert!(failed_with(env.deploy_with("alice", Asset::Sol, 1_000 * USDC, route), "Unauthorized"));
    assert!(failed_with(env.unwind_as("alice", Asset::Eth, ETH / 10), "Unauthorized"));
    assert!(failed_with(env.rehedge_as("alice", Asset::Eth), "Unauthorized"));
    assert!(failed_with(env.move_margin_as("alice", false, 100 * USDC), "Unauthorized"));
    // The admin may step in for the keeper.
    let admin = env.admin_seed();
    expect(env.unwind_as(admin, Asset::Eth, ETH / 10));
}

#[test]
fn an_inactive_leg_takes_nothing_new_but_can_be_unwound() {
    let mut env = funded(20_000);
    env.deploy_hedged(Asset::Btc, 10_000 * USDC);
    expect(env.update_leg(Asset::Btc, LegParams { active: false, ..open_leg() }));
    assert!(failed_with(env.deploy(Asset::Btc, 1_000 * USDC), "LegInactive"));
    let units = env.view().leg(Asset::Btc).units;
    expect(env.unwind(Asset::Btc, units));
    assert_eq!(env.view().leg(Asset::Btc).position_lots, 0);
}

#[test]
fn the_keeper_keeps_the_buffer_and_what_requests_are_owed() {
    let mut env = funded(100_000);
    expect(env.update_vault(VaultParams { buffer_bps: 1_000, ..default_params() }));
    // 10% of NAV stays: $90,000 may go (the deploy's own cost takes the NAV just under
    // $100,000), $100 more may not.
    expect(env.deploy(Asset::Sol, 90_000 * USDC));
    let route = env.route_for(Asset::Sol, Sell::Usdc, 100 * USDC);
    assert!(failed_with(
        env.deploy_with(env.keeper_seed(), Asset::Sol, 100 * USDC, route),
        "BufferTooThin"
    ));
    // Unwinding refills it; a pending request reserves its value on top of the buffer.
    expect(env.unwind(Asset::Sol, 200 * SOL));
    let v = env.view();
    let free = v.idle_usd - v.nav_usd * 0.1;
    assert!(free > 29_000.0, "{free}");
    let shares = 20_000 * USDC;
    expect(env.request_withdraw("alice", shares));
    let route = env.route_for(Asset::Eth, Sell::Usdc, 15_000 * USDC);
    assert!(failed_with(
        env.deploy_with(env.keeper_seed(), Asset::Eth, 15_000 * USDC, route),
        "BufferTooThin"
    ));
    assert!(failed_with(env.move_margin(true, 15_000 * USDC), "BufferTooThin"));
    expect(env.move_margin(true, 5_000 * USDC));
}

#[test]
fn the_margin_floor_holds_for_deploys_and_margin_taken_out() {
    let mut env = funded(20_000);
    expect(env.update_vault(VaultParams { margin_bps: 9_000, ..default_params() }));
    // $1,000 of margin for a $5,000 ETH short is 20%: Phoenix would take it, the vault does not.
    expect(env.move_margin(true, 1_000 * USDC));
    assert!(failed_with(env.deploy(Asset::Eth, 5_000 * USDC), "MarginTooThin"));
    expect(env.move_margin(true, 4_000 * USDC));
    expect(env.deploy(Asset::Eth, 5_000 * USDC));
    // Taking margin back out below 90% is refused; some of it can go.
    assert!(failed_with(env.move_margin(false, 1_000 * USDC), "MarginTooThin"));
    expect(env.move_margin(false, 300 * USDC));
    assert!(env.view().margin_ratio() >= 0.9);
}

#[test]
fn every_leg_cap_holds() {
    let mut env = funded(100_000);
    // The short's notional.
    expect(env.update_leg(Asset::Eth, LegParams { max_notional_usd: 5_000 * USDC, ..open_leg() }));
    expect(env.move_margin(true, 10_000 * USDC));
    assert!(failed_with(env.deploy(Asset::Eth, 6_000 * USDC), "MaxPositionReached"));
    expect(env.deploy(Asset::Eth, 4_000 * USDC));
    // The asset held: the SOL leg under Phoenix's native SOL cap.
    expect(env.update_leg(Asset::Sol, LegParams { max_asset_units: 50 * SOL, ..open_leg() }));
    assert!(failed_with(env.deploy(Asset::Sol, 9_000 * USDC), "MaxPositionReached"));
    expect(env.deploy(Asset::Sol, 7_000 * USDC));
    // Its share of the vault.
    expect(env.update_leg(Asset::Btc, LegParams { max_nav_share_bps: 1_000, ..open_leg() }));
    expect(env.move_margin(true, 15_000 * USDC));
    assert!(failed_with(env.deploy(Asset::Btc, 12_000 * USDC), "LegShareReached"));
    expect(env.deploy(Asset::Btc, 9_000 * USDC));
    // Its share of the market's open interest: alone, the vault would be every TSLA short.
    expect(env.update_leg(Asset::Tsla, LegParams { max_oi_share_bps: 2_500, ..open_leg() }));
    assert!(failed_with(env.deploy(Asset::Tsla, 1_000 * USDC), "OpenInterestShareReached"));
    // Others short 100 TSLA: the vault may add about a third of that.
    env.outside_trade(Asset::Tsla, true, 100_000);
    expect(env.deploy(Asset::Tsla, 10_000 * USDC));
    assert!(failed_with(env.deploy(Asset::Tsla, 5_000 * USDC), "OpenInterestShareReached"));
    let v = env.print("every leg at or under its cap");
    let tsla = v.leg(Asset::Tsla).short(&env, Asset::Tsla);
    assert!(tsla * 10_000.0 <= env.open_interest_lots(Asset::Tsla) as f64 / 1_000.0 * 2_500.0);
}

#[test]
fn the_keepers_turnover_is_limited_per_day() {
    let mut env = funded(100_000);
    expect(env.update_vault(VaultParams { max_turnover_usd: 50_000 * USDC, ..default_params() }));
    expect(env.deploy(Asset::Sol, 30_000 * USDC));
    assert!(failed_with(env.deploy(Asset::Sol, 30_000 * USDC), "TurnoverLimitReached"));
    // Unwinds count too: churning in and out is what the limit stops.
    expect(env.unwind(Asset::Sol, 100 * SOL));
    assert!(failed_with(env.unwind(Asset::Sol, 50 * SOL), "TurnoverLimitReached"));
    env.warp_hours(25);
    expect(env.deploy(Asset::Sol, 30_000 * USDC));
}

#[test]
fn swaps_fill_near_the_mark_and_move_nothing_else() {
    let mut env = funded(50_000);
    env.deploy_hedged(Asset::Btc, 10_000 * USDC);
    expect(env.move_margin(true, 10_000 * USDC));
    let keeper = env.keeper_seed();

    // 2% under the mark is past the vault's 1% band.
    let route = env.mock_route(Asset::Eth, Sell::Usdc, 3_500 * USDC, 98 * ETH / 100);
    assert!(failed_with(env.deploy_with(keeper, Asset::Eth, 3_500 * USDC, route), "SwapPriceTooLow"));
    // 0.5% under is the vault's to carry, and the short follows what arrived.
    let route = env.mock_route(Asset::Eth, Sell::Usdc, 3_500 * USDC, 995 * ETH / 1_000);
    expect(env.deploy_with(keeper, Asset::Eth, 3_500 * USDC, route));
    let v = env.view();
    assert_eq!(v.leg(Asset::Eth).units, 995 * ETH / 1_000);
    assert_eq!(v.leg(Asset::Eth).position_lots, -995);
    // A route built for a third of the amount is resized to it.
    let route = env.mock_route(Asset::Eth, Sell::Usdc, 1_000 * USDC, ETH * 1_000 / 3_500);
    let meta = expect(env.deploy_with(keeper, Asset::Eth, 3_000 * USDC, route));
    let swap = &events::<vanna_phoenix_multi_vault::events::Swapped>(&meta)[0];
    assert_eq!(swap.spent, 3_000 * USDC);

    // A route that takes another leg's BTC while it buys ETH: a $1 deploy, resized to 1,000,000
    // units, takes 0.01 BTC.
    let mut route = env.mock_route(Asset::Eth, Sell::Usdc, USDC, ETH / 3_500);
    let btc = env.leg(Asset::Btc).holding.unwrap();
    let reserve_btc = Env::ata(&mock_reserve(), &BTC_MINT);
    route.metas[1].pubkey = btc;
    route.metas[3].pubkey = reserve_btc;
    route.metas[6].pubkey = BTC_MINT;
    assert!(failed_with(env.deploy_with(keeper, Asset::Eth, USDC, route), "SwapMovedVaultFunds"));
    // A route that takes the escrowed shares of a pending request.
    env.request_withdraw("alice", 1_000 * USDC).unwrap();
    let mut route = env.mock_route(Asset::Eth, Sell::Usdc, USDC, ETH / 3_500);
    env.create_ata(keeper, mock_reserve(), env.vault.share_mint);
    route.metas[1].pubkey = env.vault.escrow;
    route.metas[3].pubkey = get_associated_token_address(&mock_reserve(), &env.vault.share_mint);
    route.metas[6].pubkey = env.vault.share_mint;
    assert!(failed_with(env.deploy_with(keeper, Asset::Eth, USDC, route), "SwapMovedVaultFunds"));
    // Only Jupiter's own route instructions, which the vault knows how to resize.
    let mut route = env.route_for(Asset::Eth, Sell::Usdc, 1_000 * USDC);
    route.args.data[0] ^= 1;
    assert!(failed_with(env.deploy_with(keeper, Asset::Eth, 1_000 * USDC, route), "UnsupportedRoute"));
    // With swaps off, the keeper cannot deploy at all.
    let k = env.keeper;
    expect(env.set_config(k, Pubkey::default(), false));
    assert!(failed_with(env.deploy(Asset::Eth, 1_000 * USDC), "InvalidSwapAccounts"));
}

#[test]
fn stock_legs_wait_for_the_session() {
    let mut env = funded(50_000);
    expect(env.update_leg(Asset::Meta, LegParams { after_hours_trading: false, ..open_leg() }));
    env.deploy_hedged(Asset::Meta, 10_000 * USDC);
    // Friday 8pm New York: Phoenix holds META after hours until Sunday night.
    env.set_after_hours(Asset::Meta, true);
    assert!(failed_with(env.deploy(Asset::Meta, 1_000 * USDC), "MarketAfterHours"));
    expect(env.unwind(Asset::Meta, STOCK));
    expect(env.deposit("alice", 1_000 * USDC));
    expect(env.withdraw("alice", 1_000 * USDC));
    env.set_after_hours(Asset::Meta, false);
    expect(env.deploy(Asset::Meta, 1_000 * USDC));
}

#[test]
fn a_multiplier_change_is_priced_for_those_who_stay_and_never_closes_the_vault() {
    let mut env = funded(50_000);
    env.add_user("bob", 10 * SOL);
    let bob = env.ctx.signer_pubkey("bob");
    env.deploy_hedged(Asset::Meta, 20_000 * USDC);
    let before = env.view();
    let meta_value = before.leg(Asset::Meta).value_usd;

    // A 2-for-1 split through METAx's multiplier, an hour from now. Until Phoenix and the
    // token have caught up the NAV could be wrong either way, so a deposit values METAx as if
    // it were already worth two shares, and a withdrawal as if it were still worth one.
    let now = env.now();
    let m = env.multiplier(Asset::Meta);
    env.set_multiplier(Asset::Meta, m, m * 2.0, now + 3_600);
    assert!(failed_with(env.deploy(Asset::Meta, 1_000 * USDC), "MultiplierChanging"));
    let meta = expect(env.deposit("bob", 10_000 * USDC));
    let d = &events::<vanna_phoenix_multi_vault::events::Deposited>(&meta)[0];
    eprintln!(
        "bob's deposit priced at a NAV of ${:.2} (METAx ${meta_value:.2} counted twice); alice's ${:.2}",
        usd(d.nav_before_usd),
        before.nav_usd
    );
    assert!((usd(d.nav_before_usd) - (before.nav_usd + meta_value)).abs() < 1.0);
    // Leaving at once gives back less than he brought, and alice keeps the difference: the
    // window costs only who acts in it.
    let shares = env.shares(bob);
    let meta = expect(env.withdraw("bob", shares));
    let out = events::<vanna_phoenix_multi_vault::events::Withdrawn>(&meta)[0].usdc_out;
    let after = env.view();
    eprintln!("bob: $10,000 in, ${:.2} out in the window", usd(out));
    assert!(usd(out) < 9_000.0);
    assert!(after.nav_usd > before.nav_usd + 1_000.0);
    // Alice can leave at any time in the window.
    expect(env.withdraw("alice", 1_000 * USDC));

    // A regular dividend's step changes nothing.
    env.set_multiplier(Asset::Meta, m, m * 1.0007, now + 3_600);
    expect(env.deploy(Asset::Meta, 1_000 * USDC));
}

#[test]
fn a_dust_donation_cannot_hold_the_vault() {
    let mut env = funded(20_000);
    env.add_user("griefer", 10 * SOL);
    // The META leg is flat; a griefer sends it one unit of METAx just before a split.
    env.donate_asset(Asset::Meta, "griefer", 1);
    let now = env.now();
    let m = env.multiplier(Asset::Meta);
    env.set_multiplier(Asset::Meta, m, m * 2.0, now + 3_600);
    expect(env.deposit("alice", 1_000 * USDC));
    expect(env.withdraw("alice", 1_000 * USDC));
    let id = expect(env.request_withdraw("alice", 1_000 * USDC));
    let alice = env.ctx.signer_pubkey("alice");
    expect(env.fulfill_withdraw("alice", alice, id));
}

#[test]
fn a_flat_leg_retires_and_its_market_can_no_longer_hold_the_vault() {
    let mut env = funded(30_000);
    env.add_user("griefer", 10 * SOL);
    env.deploy_hedged(Asset::Meta, 10_000 * USDC);
    // A leg that holds or shorts anything cannot retire.
    assert!(failed_with(env.retire_leg(Asset::Meta), "LegNotFlat"));
    let units = env.view().leg(Asset::Meta).units;
    expect(env.unwind(Asset::Meta, units));
    // Dust someone sends does not keep it alive.
    env.donate_asset(Asset::Meta, "griefer", 100);
    let before = env.view();
    let accounts = env.leg_metas().len();
    expect(env.retire_leg(Asset::Meta));
    assert_eq!(env.leg_metas().len(), accounts - 2, "the leg alone now, no holding or mint");
    let after = env.view();
    assert!((after.nav_usd - before.nav_usd).abs() < 0.01, "only the dust leaves the NAV");

    // The issuer breaks METAx's multiplier: a live leg would stop every deposit and withdrawal
    // (the vault could not value it); the retired one is never read.
    let at = {
        let data = env.ctx.account_data(&META_MINT);
        let mut at = 166;
        while u16::from_le_bytes([data[at], data[at + 1]]) != 25 {
            at += 4 + u16::from_le_bytes([data[at + 2], data[at + 3]]) as usize;
        }
        at + 4
    };
    let mut account = env.ctx.svm.get_account(&META_MINT).unwrap();
    account.data[at + 32..at + 40].copy_from_slice(&(-1.0f64).to_le_bytes());
    account.data[at + 48..at + 56].copy_from_slice(&(-1.0f64).to_le_bytes());
    env.ctx.svm.set_account(META_MINT, account).unwrap();
    expect(env.deposit("alice", 1_000 * USDC));
    expect(env.withdraw("alice", 1_000 * USDC));
    // It stays retired.
    let route = env.mock_route(Asset::Meta, Sell::Usdc, 1_000 * USDC, STOCK);
    let keeper = env.keeper_seed();
    assert!(failed_with(env.deploy_with(keeper, Asset::Meta, 1_000 * USDC, route), "LegRetired"));
    assert!(failed_with(env.update_leg(Asset::Meta, open_leg()), "LegRetired"));
    assert!(failed_with(env.retire_leg(Asset::Meta), "LegRetired"));
}

#[test]
fn a_paused_vault_takes_no_money_in_but_lets_it_out() {
    let mut env = funded(30_000);
    env.deploy_hedged(Asset::Eth, 10_000 * USDC);
    let k = env.keeper;
    expect(env.set_config(k, JUPITER, true));
    assert!(failed_with(env.deploy(Asset::Sol, 1_000 * USDC), "Paused"));
    assert!(failed_with(env.move_margin(true, 1_000 * USDC), "Paused"));
    // Unwinding, margin out and withdrawals stay open.
    expect(env.unwind(Asset::Eth, ETH / 2));
    expect(env.move_margin(false, 1_000 * USDC));
    expect(env.withdraw("alice", 1_000 * USDC));
}

#[test]
fn legs_are_checked_against_phoenix_and_each_other() {
    let mut env = Env::with_first_legs(2);
    // ETH's mint on BTC's market: a second leg on one asset would count its tokens twice.
    let mut btc = env.leg(Asset::Btc).clone();
    let market = MarketParams {
        asset_id: btc.asset_id,
        base_lot_decimals: btc.base_lot_decimals,
        tick_size: btc.tick_size,
    };
    let mut twice = btc.clone();
    twice.asset_mint = ETH_MINT;
    twice.holding = env.leg(Asset::Eth).holding;
    assert!(failed_with(env.try_add_leg(&twice, LegKind::Spot, market, open_leg()), "DuplicateLeg"));
    // A new token on ETH's market: one market, one leg.
    let mint = env.new_spot_mint("other-eth", 8);
    let mut eth = env.leg(Asset::Eth).clone();
    eth.asset_mint = mint;
    eth.holding = Some(get_associated_token_address(&env.vault.authority, &mint));
    let eth_market = MarketParams {
        asset_id: eth.asset_id,
        base_lot_decimals: eth.base_lot_decimals,
        tick_size: eth.tick_size,
    };
    assert!(failed_with(env.try_add_leg(&eth, LegKind::Spot, eth_market, open_leg()), "DuplicateLeg"));
    // BTC's market with a tick size 10x off.
    let wrong = MarketParams { tick_size: btc.tick_size * 10, ..market };
    assert!(failed_with(env.try_add_leg(&btc, LegKind::Spot, wrong, open_leg()), "InvalidPhoenixAccount"));
    // Wrapped SOL as a spot token: the SOL leg already holds SOL.
    let mut wsol = btc.clone();
    wsol.asset_mint = NATIVE_MINT;
    wsol.holding = Some(get_associated_token_address(&env.vault.authority, &NATIVE_MINT));
    assert!(failed_with(env.try_add_leg(&wsol, LegKind::Spot, market, open_leg()), "DuplicateLeg"));
    // A spot token as native SOL.
    btc.holding = None;
    assert!(failed_with(env.try_add_leg(&btc, LegKind::NativeSol, market, open_leg()), "InvalidMint"));
    // And the real BTC leg.
    let btc = env.leg(Asset::Btc).clone();
    expect(env.try_add_leg(&btc, LegKind::Spot, market, open_leg()));
    assert_eq!(env.vault_account().leg_count, 3);
}

#[test]
fn only_the_upgrade_authority_initializes_and_the_venue_is_jupiter() {
    let mut env = Env::bare();
    env.ctx.add_signer("mallory", 10 * SOL);
    assert!(failed_with(env.initialize("mallory"), "Unauthorized"));
    let admin = env.admin_seed();
    expect(env.initialize(admin));
    let k = env.keeper;
    assert!(failed_with(env.set_config(k, Pubkey::new_unique(), false), "InvalidParameter"));
    assert!(failed_with(env.set_config(k, anchor_spl::token::ID, false), "InvalidParameter"));
    expect(env.set_config(k, Pubkey::default(), false));
    expect(env.set_config(k, JUPITER, false));
    // Fees and bands are bounded.
    expect(env.create_vault(default_params()));
    for params in [
        VaultParams { entry_fee_bps: 101, ..default_params() },
        VaultParams { exit_fee_bps: 101, ..default_params() },
        VaultParams { slippage_bps: 9, ..default_params() },
        VaultParams { slippage_bps: 201, ..default_params() },
        VaultParams { buffer_bps: 10_001, ..default_params() },
        VaultParams { margin_bps: 0, ..default_params() },
    ] {
        assert!(failed_with(env.update_vault(params), "InvalidParameter"));
    }
}
