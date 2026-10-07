//! Regression tests for the security audit (AUDIT.md). Each test attacks the program the way
//! a finding describes and checks that the attack now fails, or that the scenario is safe.

use crate::env::*;
use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::AccountMeta;
use vanna_phoenix_vault::events::{Deposited, Withdrawn};
use vanna_phoenix_vault::instructions::VaultParams;

fn expect<T>(result: Result<T, Vec<String>>) -> T {
    result.unwrap_or_else(|logs| panic!("{}", logs.join("\n")))
}

/// H-01: the swap venue runs with the vault authority's signature. Only Jupiter v6 (or no
/// venue) may be set, so neither the admin key nor a typo can point it at a program that mints
/// shares or moves the vault's Phoenix collateral.
#[test]
fn h01_the_swap_venue_can_only_be_jupiter() {
    let mut env = Env::new();
    assert!(failed_with(
        env.set_config_swap_program(Pubkey::new_unique()),
        "InvalidParameter"
    ));
    // The token program would let a route mint shares with the authority's signature.
    assert!(failed_with(
        env.set_config_swap_program(anchor_spl::token::ID),
        "InvalidParameter"
    ));
    // Turning swaps off is allowed.
    expect(env.set_config_swap_program(Pubkey::default()));
    expect(env.set_config_swap_program(JUPITER));
}

/// H-02: a stolen keeper key could drain about 2% of a trade per buy/sell round trip (1% each
/// way), as often as it liked. The keeper now trades within its own tighter band, up to a daily
/// volume.
#[test]
fn h02_a_keeper_cannot_drain_the_vault_through_round_trips() {
    let mut env = Env::new();
    env.update_vault(
        Asset::Eth,
        VaultParams {
            keeper_band_bps: 20,
            max_rebalance_usd: 20_000 * USDC,
            ..default_params()
        },
    );
    env.setup_keeper_accounts();
    let carol = env.add_user("carol", 10 * SOL);
    env.mint_asset(Asset::Eth, carol, 10 * ETH);
    env.mint_usdc(carol, 35_000 * USDC);
    expect(env.deposit(Asset::Eth, "carol", 10 * ETH, 35_000 * USDC));
    let start = env.print(Asset::Eth, "carol deposits 10 ETH + $35,000");
    let keeper = env.keeper;
    env.mint_asset(Asset::Eth, keeper, 10 * ETH);
    env.mint_usdc(keeper, 100_000 * USDC);

    // The old 1% edge is refused.
    assert!(failed_with(
        env.rebalance(Asset::Eth, true, ETH, 3_535 * USDC),
        "PriceOutOfBand"
    ));
    // Round trips at the 0.2% edge stop at the daily limit: $20,000 of trades.
    let mut trips = 0;
    loop {
        let buy = env.rebalance(Asset::Eth, true, ETH, 3_507 * USDC);
        let sell = buy.and_then(|_| env.rebalance(Asset::Eth, false, ETH, 3_493 * USDC));
        match sell {
            Ok(_) => trips += 1,
            Err(logs) => {
                assert!(logs.iter().any(|l| l.contains("RebalanceLimitReached")));
                break;
            }
        }
    }
    let end = env.print(Asset::Eth, "after the keeper's round trips");
    let lost = start.nav_usd - end.nav_usd;
    eprintln!("{trips} round trips before the daily limit; the vault lost ${lost:.2}");
    assert!(trips <= 3);
    assert!(
        lost < 100.0,
        "bounded by the band on the daily limit, plus hedge costs"
    );

    // A day later the limit resets.
    env.warp_hours(25);
    expect(env.rebalance(Asset::Eth, true, ETH, 3_507 * USDC));
}

/// M-01: anyone could call `initialize` on a fresh deployment first and become its admin.
#[test]
fn m01_only_the_upgrade_authority_initializes() {
    let mut env = Env::bare();
    env.ctx.add_signer("mallory", 10 * SOL);
    assert!(failed_with(env.initialize("mallory"), "Unauthorized"));
    let admin = env.admin_seed();
    expect(env.initialize(admin));
}

/// M-02: Phoenix lets anyone register the trader account of any authority. With the vault's
/// authority fixed by its asset, someone could register it first and block that asset's vault
/// forever. The authority now takes a nonce the admin picks: a blocked address is skipped.
#[test]
fn m02_a_preregistered_trader_cannot_block_a_vault() {
    let mut env = Env::new();
    let mint = env.new_spot_mint("new-asset", 8);
    env.ctx.add_signer("griefer", 10 * SOL);
    let blocked = spot_vault(&env, mint, 0);
    env.preregister_trader("griefer", &blocked);
    assert!(env.try_create_vault(&blocked, default_params()).is_err());
    expect(env.try_create_vault(&spot_vault(&env, mint, 7), default_params()));
}

fn spot_vault(env: &Env, mint: Pubkey, nonce: u64) -> Vault {
    let mut v = env.spot_vault_on(mint, Asset::Eth, nonce);
    v.vault_asset = Some(Env::ata(&v.authority, &mint));
    v
}

/// M-03: `create_vault` took the market's lot and tick sizes and order book on trust. A wrong
/// tick size prices the vault 10x off; a wrong order book hedges on another market.
#[test]
fn m03_create_vault_checks_the_market_against_phoenix() {
    let mut env = Env::new();
    let fake = env.new_spot_mint("fake-eth", 8);
    // ETH's asset id and order book, with a tick size of 100 instead of 10.
    assert!(failed_with(
        env.try_create_vault_on(fake, Asset::Eth, |m| m.tick_size = 100),
        "InvalidPhoenixAccount"
    ));
    // ETH's asset id with BTC's order book.
    let btc_book = env.btc.orderbook;
    assert!(failed_with(
        env.try_create_vault_on_book(fake, Asset::Eth, btc_book),
        "InvalidPhoenixAccount"
    ));
    expect(env.try_create_vault_on(fake, Asset::Eth, |_| {}));
}

/// M-04: Hawkeye's views only read the trader index accounts the caller passes; Phoenix checks
/// them in its own instructions. Every valuation now starts with one, so other Phoenix-owned
/// accounts in their place are refused before anything is priced.
#[test]
fn m04_fake_trader_index_accounts_are_refused_before_valuing() {
    let mut env = Env::new();
    let ann = env.add_user("ann", 10 * SOL);
    env.mint_asset(Asset::Eth, ann, ETH);
    env.mint_usdc(ann, 3_500 * USDC);
    let mut ix = env.deposit_ix(Asset::Eth, ann, ETH, 3_500 * USDC, 0);
    let atb = env.atb[0];
    let other = env.perp_asset_map;
    for meta in ix.accounts.iter_mut().filter(|m| m.pubkey == atb) {
        *meta = AccountMeta::new(other, false);
    }
    let logs = env.send("ann", vec![ix], "fake index").unwrap_err();
    assert!(
        logs.iter()
            .any(|l| l.contains("EtrnLz") && l.contains("failed")),
        "Phoenix refused the index accounts"
    );
}

/// M-05: assets donated to a vault, or a short Phoenix liquidated or deleveraged, made the
/// next withdrawal open new short for them; a donation larger than the order book blocked
/// every withdrawal. A withdrawal now only closes its share of the short.
#[test]
fn m05_a_donation_cannot_block_withdrawals() {
    let mut env = Env::new();
    let ann = env.add_user("ann", 10 * SOL);
    let bob = env.add_user("bob", 10 * SOL);
    for (seed, who) in [("ann", ann), ("bob", bob)] {
        env.mint_asset(Asset::Tsla, who, 10 * STOCK);
        env.mint_usdc(who, 3_900 * USDC);
        expect(env.deposit(Asset::Tsla, seed, 10 * STOCK, 3_900 * USDC));
    }
    // More TSLAx than the whole book quotes (2,000 shares), sent straight to the vault.
    env.add_user("griefer", 10 * SOL);
    let griefer = env.ctx.signer_pubkey("griefer");
    env.mint_asset(Asset::Tsla, griefer, 3_000 * STOCK);
    env.donate_asset(Asset::Tsla, "griefer", 3_000 * STOCK);
    let shares = env.shares(Asset::Tsla, ann);
    let meta = expect(env.withdraw(Asset::Tsla, "ann", shares));
    let out = &events::<Withdrawn>(&meta)[0];
    let v = env.print(Asset::Tsla, "ann withdraws after the donation");
    eprintln!(
        "ann gets {:.2} TSLAx + ${:.2}; bob's 10 TSLA of short stay",
        out.asset_out as f64 / STOCK as f64,
        usd(out.usdc_out)
    );
    assert_eq!(
        v.position_lots, -10_000,
        "only ann's half of the short closed"
    );
}

/// M-05: what withdrawals leave unhedged, the keeper hedges with a hedge-only rebalance.
#[test]
fn m05_the_keeper_rehedges_a_donation() {
    let mut env = Env::new();
    env.setup_keeper_accounts();
    let ann = env.add_user("ann", 10 * SOL);
    env.mint_asset(Asset::Tsla, ann, 15 * STOCK);
    env.mint_usdc(ann, 3_900 * USDC);
    expect(env.deposit(Asset::Tsla, "ann", 10 * STOCK, 3_900 * USDC));
    env.donate_asset(Asset::Tsla, "ann", 5 * STOCK);
    expect(env.rebalance(Asset::Tsla, true, 0, 0));
    let v = env.print(Asset::Tsla, "keeper re-hedges a 5 TSLAx donation");
    assert_eq!(v.position_lots, -15_000);
}

/// M-06: a stock token's multiplier can jump (a special dividend, a split) at a time set in
/// advance; a deposit around it could be valued at the wrong number of shares. Deposits wait
/// for it to settle; withdrawals, which pay in kind, stay open.
#[test]
fn m06_deposits_wait_out_a_large_multiplier_change() {
    let mut env = Env::new();
    let cy = env.add_user("cy", 10 * SOL);
    env.mint_asset(Asset::Meta, cy, 20 * STOCK);
    env.mint_usdc(cy, 15_000 * USDC);
    expect(env.deposit(Asset::Meta, "cy", 10 * STOCK, 7_500 * USDC));
    // A 2-for-1 split through the multiplier, an hour from now.
    let now = env
        .ctx
        .svm
        .get_sysvar::<anchor_lang::prelude::Clock>()
        .unix_timestamp;
    let multiplier = env.multiplier(Asset::Meta);
    env.set_multiplier(Asset::Meta, multiplier, multiplier * 2.0, now + 3_600);
    assert!(failed_with(
        env.deposit(Asset::Meta, "cy", 10 * STOCK, 7_500 * USDC),
        "MultiplierChanging"
    ));
    let half = env.shares(Asset::Meta, cy) / 2;
    expect(env.withdraw(Asset::Meta, "cy", half));
    // A regular dividend's step does not stop deposits.
    env.set_multiplier(Asset::Meta, multiplier, multiplier * 1.0007, now + 3_600);
    expect(env.deposit(Asset::Meta, "cy", 10 * STOCK, 7_500 * USDC));
}

/// M-07: the admin could set a price band of zero, where no order fills and nobody can leave.
#[test]
fn m07_the_price_band_has_a_floor() {
    let mut env = Env::new();
    for slippage_bps in [0, 9] {
        let params = VaultParams {
            slippage_bps,
            keeper_band_bps: 1,
            ..default_params()
        };
        assert!(failed_with(
            env.try_update_vault(Asset::Eth, params),
            "InvalidParameter"
        ));
    }
}

/// M-08: a deposit off the vault's mix (USDC alone, or the asset alone) without a swap traded
/// the vault's hedged asset at Phoenix's mark, which can sit away from spot. It must swap.
#[test]
fn m08_a_deposit_without_a_swap_must_match_the_mix() {
    let mut env = Env::new();
    let dee = env.add_user("dee", 10 * SOL);
    env.mint_asset(Asset::Eth, dee, 2 * ETH);
    env.mint_usdc(dee, 10_000 * USDC);
    assert!(failed_with(
        env.deposit(Asset::Eth, "dee", 0, 3_500 * USDC),
        "DepositOffMix"
    ));
    assert!(failed_with(
        env.deposit(Asset::Eth, "dee", ETH, 2_000 * USDC),
        "DepositOffMix"
    ));
    // Within 5% of the mix is fine.
    expect(env.deposit(Asset::Eth, "dee", ETH, 3_600 * USDC));
    // The SOL vault's mix is SOL alone.
    assert!(failed_with(
        env.deposit(Asset::Sol, "dee", 0, 1_000 * USDC),
        "DepositOffMix"
    ));
}

/// L-01: deposits are refused while the vault owes more than it holds but has shares out: a
/// new depositor would pay the old holders' debt.
#[test]
fn l01_no_deposits_into_a_wiped_out_vault() {
    let mut env = Env::new();
    env.add_user("bo", 100 * SOL);
    expect(env.deposit(Asset::Sol, "bo", 10 * SOL, 0));
    // The trader account loses its SOL (as in a liquidation): the vault's equity is negative.
    env.drain_sol_vault();
    assert!(failed_with(
        env.deposit(Asset::Sol, "bo", 10 * SOL, 0),
        "VaultInsolvent"
    ));
}

/// L-02: the deposit priced shares with the virtual offset but the withdrawal did not, so in a
/// nearly empty vault a large deposit-and-withdraw took value from the holder there. Both
/// sides now use the same price.
#[test]
fn l02_a_round_trip_in_a_tiny_vault_takes_nothing() {
    let mut env = Env::new();
    let dot = env.add_user("dot", 10 * SOL);
    let eve = env.add_user("eve", 10 * SOL);
    env.mint_asset(Asset::Eth, dot, ETH / 50);
    env.mint_usdc(dot, USDC);
    // Dot holds 0.7 shares of a vault worth $35.70: a share price near $51.
    expect(env.deposit(Asset::Eth, "dot", ETH / 10_000, 350_000));
    env.donate_asset(Asset::Eth, "dot", ETH / 100);
    let dot_before = env.view(Asset::Eth).nav_usd;
    // Eve deposits $70,000 and leaves at once.
    env.mint_asset(Asset::Eth, eve, 10 * ETH);
    env.mint_usdc(eve, 35_000 * USDC);
    expect(env.deposit(Asset::Eth, "eve", 10 * ETH, 35_000 * USDC));
    let shares = env.shares(Asset::Eth, eve);
    expect(env.withdraw(Asset::Eth, "eve", shares));
    let dot_after = env.view(Asset::Eth).nav_usd;
    eprintln!("dot's holding: ${dot_before:.4} before eve, ${dot_after:.4} after");
    assert!(dot_after >= dot_before - 0.05, "eve took nothing from dot");
}

/// First deposit: the classic share-inflation attack. The attacker makes a dust deposit,
/// donates a large amount to make each share expensive, and hopes the victim's deposit rounds
/// down to few shares. Here the donation is unhedged, so the victim's deposit either fails
/// (and they keep their money) or prices the donation into their shares; either way the
/// attacker only loses.
#[test]
fn first_deposit_inflation_attack_fails() {
    let mut env = Env::new();
    let atk = env.add_user("attacker", 10 * SOL);
    let vic = env.add_user("victim", 10 * SOL);
    // 1. A dust deposit at the vault's mix: 0.001 ETH and $3.50.
    env.mint_asset(Asset::Eth, atk, ETH / 1_000 + 10 * ETH);
    env.mint_usdc(atk, 4 * USDC);
    let meta = expect(env.deposit(Asset::Eth, "attacker", ETH / 1_000, 3_500_000));
    let dust = events::<Deposited>(&meta)[0].shares;
    // 2. 10 ETH ($35,000) donated straight to the vault's token account.
    env.donate_asset(Asset::Eth, "attacker", 10 * ETH);
    // 3. The victim deposits $3,500.
    env.mint_asset(Asset::Eth, vic, ETH / 2);
    env.mint_usdc(vic, 1_750 * USDC);
    match env.deposit(Asset::Eth, "victim", ETH / 2, 1_750 * USDC) {
        Err(_) => {
            // The donation is unhedged: hedging it with the victim's margin is refused.
            eprintln!("the victim's deposit reverts: they keep their $3,500");
            assert_eq!(env.asset_balance(Asset::Eth, vic), ETH / 2);
            assert_eq!(env.usdc(vic), 1_750 * USDC);
        }
        Ok(meta) => {
            let dep = &events::<Deposited>(&meta)[0];
            let shares = env.shares(Asset::Eth, vic);
            let out = expect(env.withdraw(Asset::Eth, "victim", shares));
            let back = usd(events::<Withdrawn>(&out)[0].payout_usd);
            assert!(back > usd(dep.value_usd) * 0.99, "the victim loses nothing");
        }
    }
    // 4. The attacker leaves with less than they put in.
    let meta = expect(env.withdraw(Asset::Eth, "attacker", dust));
    let out = usd(events::<Withdrawn>(&meta)[0].payout_usd);
    eprintln!("attacker: put in $35,007, took out ${out:.2}");
    assert!(out < 35_007.0);
}
