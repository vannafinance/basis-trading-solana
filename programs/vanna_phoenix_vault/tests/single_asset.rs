//! Single-asset deposits and withdrawals through real Jupiter. The user brings only USDC or
//! only the asset; the vault plans the swap to its target mix, runs a Jupiter v6 route over a
//! real Orca Whirlpool pool (snapshotted from mainnet by `scripts/jupiter_fork.py`) and books
//! what Jupiter actually delivered, never the quote: the asset it holds, the short, the margin
//! and the shares. Withdrawals can pay out in one asset the same way.

use crate::env::*;
use anchor_spl::associated_token::get_associated_token_address;
use vanna_phoenix_vault::constants::NATIVE_MINT;
use vanna_phoenix_vault::events::{Deposited, Swapped, Withdrawn};
use vanna_phoenix_vault::instructions::VaultParams;
use vanna_phoenix_vault::swap::{Receive, Sell};

fn spot_params() -> VaultParams {
    VaultParams {
        margin_bps: 9_000,
        ..default_params()
    }
}

fn expect<T>(result: Result<T, Vec<String>>) -> T {
    result.unwrap_or_else(|logs| panic!("{}", logs.join("\n")))
}

/// Jupiter loaded, and Phoenix marked between the pools' buy and sell prices.
fn jupiter_env() -> Env {
    let mut env = Env::new();
    env.update_vault(Asset::Eth, spot_params());
    env.update_vault(Asset::Btc, spot_params());
    env.use_jupiter();
    env
}

/// What the vault sold, Jupiter's quote, and what its route actually delivered.
fn report(label: &str, quoted: u64, swap: &Swapped, decimals: u8, cu: u64) {
    let asset = 10f64.powi(decimals as i32);
    let usdc = USDC as f64;
    let (sold, bought) = if swap.sold_usdc {
        (usdc, asset)
    } else {
        (asset, usdc)
    };
    eprintln!(
        "{label}: sold {:.8}, Jupiter quoted {:.8}, delivered {:.8} ({:+.4}%), {cu} CU",
        swap.spent as f64 / sold,
        quoted as f64 / bought,
        swap.received as f64 / bought,
        (swap.received as f64 / quoted as f64 - 1.0) * 100.0
    );
}

#[test]
fn eth_vault_usdc_only_buys_half_in_eth_and_shorts_what_arrived() {
    let mut env = jupiter_env();
    let frank = env.add_user("frank", 10 * SOL);
    env.mint_usdc(frank, 7_000 * USDC);
    let quoted = env
        .jupiter_route_for(Asset::Eth, Sell::Usdc, 3_500 * USDC)
        .quoted_out;

    let meta = expect(env.deposit_single(Asset::Eth, "frank", 0, 7_000 * USDC));
    let swap = &events::<Swapped>(&meta)[0];
    let dep = &events::<Deposited>(&meta)[0];
    report(
        "USDC -> ETH",
        quoted,
        swap,
        SPOT_DECIMALS,
        meta.compute_units_consumed,
    );
    let v = env.print(Asset::Eth, "frank deposits $7,000 USDC");

    assert!(swap.sold_usdc);
    assert_eq!(swap.spent, 3_500 * USDC, "half the USDC buys ETH");
    assert!(
        swap.received * 1_000 >= quoted * 995,
        "Jupiter delivered its quote"
    );
    assert_eq!(
        v.asset_units, swap.received,
        "the vault holds exactly what Jupiter delivered"
    );
    assert_eq!(
        v.position_lots,
        -((swap.received as i64 + 50_000) / 100_000),
        "and is short exactly that much ETH"
    );
    assert!(
        (v.collateral_usd - 3_500.0).abs() < 5.0,
        "the other half is margin"
    );
    // Shares are priced on the value that actually arrived: Jupiter's fill, then the short.
    let arrived = swap.received as f64 / ETH as f64 * v.mark_usd + 3_500.0;
    eprintln!(
        "value: ${arrived:.2} after Jupiter at the mark, ${:.2} added after the short, {:.2} shares",
        usd(dep.value_usd),
        usd(dep.shares)
    );
    assert!(usd(dep.value_usd) < arrived && usd(dep.value_usd) > arrived - 5.0);
    assert!((usd(dep.shares) - usd(dep.value_usd)).abs() < 0.01);
    assert_eq!(env.usdc(frank), 0);
}

#[test]
fn eth_vault_eth_only_sells_half_for_margin() {
    let mut env = jupiter_env();
    let carol = env.add_user("carol", 10 * SOL);
    env.mint_asset(Asset::Eth, carol, 2 * ETH);
    let quoted = env
        .jupiter_route_for(Asset::Eth, Sell::Asset, ETH)
        .quoted_out;

    let meta = expect(env.deposit_single(Asset::Eth, "carol", 2 * ETH, 0));
    let swap = &events::<Swapped>(&meta)[0];
    report(
        "ETH -> USDC",
        quoted,
        swap,
        SPOT_DECIMALS,
        meta.compute_units_consumed,
    );
    let v = env.print(Asset::Eth, "carol deposits 2 ETH");

    assert!(!swap.sold_usdc);
    assert_eq!(swap.spent, ETH, "half the ETH is sold for margin");
    assert_eq!(v.asset_units, ETH, "1 ETH stays in the vault");
    assert_eq!(v.position_lots, -1_000, "and the vault is short 1 ETH");
    assert!(
        (v.collateral_usd - usd(swap.received)).abs() < 2.0,
        "the margin is what Jupiter delivered"
    );
}

#[test]
fn btc_vault_takes_usdc_or_btc() {
    let mut env = jupiter_env();
    let dan = env.add_user("dan", 10 * SOL);
    let erin = env.add_user("erin", 10 * SOL);
    env.mint_usdc(dan, 20_000 * USDC);
    env.mint_asset(Asset::Btc, erin, BTC / 5);

    let quoted = env
        .jupiter_route_for(Asset::Btc, Sell::Usdc, 10_000 * USDC)
        .quoted_out;
    let meta = expect(env.deposit_single(Asset::Btc, "dan", 0, 20_000 * USDC));
    let swap = &events::<Swapped>(&meta)[0];
    report(
        "USDC -> cbBTC",
        quoted,
        swap,
        SPOT_DECIMALS,
        meta.compute_units_consumed,
    );
    let one = env.print(Asset::Btc, "dan deposits $20,000 USDC");
    assert_eq!(one.asset_units, swap.received);

    let quoted = env
        .jupiter_route_for(Asset::Btc, Sell::Asset, BTC / 10)
        .quoted_out;
    let meta = expect(env.deposit_single(Asset::Btc, "erin", BTC / 5, 0));
    let swap = &events::<Swapped>(&meta)[0];
    report(
        "cbBTC -> USDC",
        quoted,
        swap,
        SPOT_DECIMALS,
        meta.compute_units_consumed,
    );
    let both = env.print(Asset::Btc, "erin deposits 0.2 BTC");
    assert!((both.share_price() - one.share_price()).abs() < 1e-4);
    assert_eq!(
        both.position_lots,
        -((both.asset_units as i64 + 5_000) / 10_000)
    );
    let ratio = both.effective_usd / (-both.position_lots as f64 / 10_000.0 * both.mark_usd);
    eprintln!("vault: margin {:.1}% of the short", ratio * 100.0);
    assert!(ratio > 0.99);
}

#[test]
fn sol_vault_usdc_only_buys_sol_and_shorts_all_of_it() {
    let mut env = jupiter_env();
    let gina = env.add_user("gina", 10 * SOL);
    env.mint_usdc(gina, 1_000 * USDC);
    let sol_before = env.asset_balance(Asset::Sol, gina);
    let quoted = env
        .jupiter_route_for(Asset::Sol, Sell::Usdc, 1_000 * USDC)
        .quoted_out;

    let meta = expect(env.deposit_single(Asset::Sol, "gina", 0, 1_000 * USDC));
    let swap = &events::<Swapped>(&meta)[0];
    report("USDC -> SOL", quoted, swap, 9, meta.compute_units_consumed);
    let v = env.print(Asset::Sol, "gina deposits $1,000 USDC");

    assert_eq!(
        swap.spent,
        1_000 * USDC,
        "all of it buys SOL: SOL is its own margin"
    );
    assert_eq!(
        v.asset_units, swap.received,
        "the unwrapped SOL is Phoenix collateral"
    );
    assert_eq!(
        v.position_lots,
        -((swap.received as i64 + 5_000_000) / 10_000_000)
    );
    assert!(v.collateral_usd.abs() < 1.0, "no USDC margin needed");
    let authority = env.sol.authority;
    let wsol = get_associated_token_address(&authority, &NATIVE_MINT);
    assert!(
        env.ctx
            .svm
            .get_account(&wsol)
            .is_none_or(|a| a.lamports == 0),
        "wrapped SOL is unwrapped and closed"
    );
    assert_eq!(
        env.ctx.svm.get_balance(&authority).unwrap_or(0),
        0,
        "the authority keeps nothing"
    );
    // Gina paid the fee and rent for her share account and the trader account's first native
    // SOL entry, and got the wrapped-SOL account's rent back.
    let rent = |len| env.ctx.svm.minimum_balance_for_rent_exemption(len);
    let paid = sol_before - env.asset_balance(Asset::Sol, gina);
    assert!(paid < meta.fee + rent(165) + rent(165) / 2);

    // SOL alone needs no swap.
    let before = v.position_lots;
    expect(env.deposit_single(Asset::Sol, "gina", 2 * SOL, 0));
    let more = env.print(Asset::Sol, "gina deposits 2 SOL");
    assert_eq!(more.position_lots, before - 200);
}

#[test]
fn withdraw_in_usdc_only_or_eth_only() {
    let mut env = jupiter_env();
    let carol = env.add_user("carol", 10 * SOL);
    env.mint_asset(Asset::Eth, carol, 2 * ETH);
    expect(env.deposit_single(Asset::Eth, "carol", 2 * ETH, 0));
    let start = env.print(Asset::Eth, "carol deposits 2 ETH");
    let half = env.shares(Asset::Eth, carol) / 2;

    let meta = expect(env.withdraw_single(Asset::Eth, "carol", half, Receive::Usdc));
    let swap = &events::<Swapped>(&meta)[0];
    let out = &events::<Withdrawn>(&meta)[0];
    env.print(Asset::Eth, "carol takes half in USDC");
    eprintln!(
        "USDC out: ${:.2} (Jupiter sold {:.6} ETH) + {:.6} ETH the route left unsold",
        usd(out.usdc_out),
        swap.spent as f64 / ETH as f64,
        out.asset_out as f64 / ETH as f64
    );
    assert!(usd(out.usdc_out) > start.nav_usd / 2.0 * 0.98);
    assert!(
        out.asset_out < ETH / 500,
        "only what the route left unsold comes as ETH"
    );

    let usdc_before = env.usdc(carol);
    let rest = env.shares(Asset::Eth, carol);
    let meta = expect(env.withdraw_single(Asset::Eth, "carol", rest, Receive::Asset));
    let swap = &events::<Swapped>(&meta)[0];
    let out = &events::<Withdrawn>(&meta)[0];
    let end = env.print(Asset::Eth, "carol takes the rest in ETH");
    eprintln!(
        "ETH out: {:.6} ETH (Jupiter bought {:.6} for ${:.2}) + ${:.2} USDC left over",
        out.asset_out as f64 / ETH as f64,
        swap.received as f64 / ETH as f64,
        usd(swap.spent),
        usd(env.usdc(carol) - usdc_before)
    );
    assert!(out.asset_out > 98 * ETH / 100, "about 1 ETH");
    assert!(
        usd(out.usdc_out) < 10.0,
        "only what the route left unsold comes as USDC"
    );
    assert_eq!(end.supply, 0);
    assert_eq!(end.position_lots, 0);
    let eth = env.asset_balance(Asset::Eth, carol) as f64 / ETH as f64;
    let held = usd(env.usdc(carol)) + eth * end.mark_usd;
    eprintln!(
        "carol: put in 2 ETH (${:.2}), holds ${held:.2} after four swaps and two trades",
        2.0 * end.mark_usd
    );
    assert!(held > 2.0 * end.mark_usd * 0.98);
}

#[test]
fn sol_vault_withdraw_in_usdc() {
    let mut env = jupiter_env();
    let alice = env.add_user("alice", 1_000 * SOL);
    expect(env.deposit_single(Asset::Sol, "alice", 10 * SOL, 0));
    let shares = env.shares(Asset::Sol, alice);
    let sol_before = env.asset_balance(Asset::Sol, alice);
    let meta = expect(env.withdraw_single(Asset::Sol, "alice", shares / 2, Receive::Usdc));
    let swap = &events::<Swapped>(&meta)[0];
    let out = &events::<Withdrawn>(&meta)[0];
    let v = env.print(Asset::Sol, "alice takes half in USDC");
    let sol_back = env.asset_balance(Asset::Sol, alice) + meta.fee - sol_before;
    eprintln!(
        "USDC out: ${:.2} (Jupiter sold {:.4} SOL); {:.6} SOL left over came back as SOL",
        usd(out.usdc_out),
        swap.spent as f64 / SOL as f64,
        sol_back as f64 / SOL as f64
    );
    assert!(usd(out.usdc_out) > 5.0 * v.mark_usd * 0.98);
    assert!(sol_back < SOL / 10);
    // Hedged to the lot; the leaver's trading cost can leave one lot until the next trade.
    assert!((v.position_lots + v.asset_units as i64 / 10_000_000).abs() <= 1);
}

#[test]
fn bad_routes_are_refused() {
    let mut env = jupiter_env();
    let frank = env.add_user("frank", 10 * SOL);
    env.mint_usdc(frank, 100_000 * USDC);
    // An honest deposit first, so the vault holds ETH a bad route could reach for.
    expect(env.deposit_single(Asset::Eth, "frank", 0, 7_000 * USDC));
    let usdc = 7_000 * USDC;
    let plan = 3_500 * USDC;

    // A third of the plan: refused.
    let third = env
        .jupiter_route_for(Asset::Eth, Sell::Usdc, plan / 3)
        .route;
    let ix = env.deposit_ix_with(Asset::Eth, frank, 0, usdc, 0, Some(third));
    assert!(failed_with(
        env.send("frank", vec![ix], "wrong amount"),
        "SwapAmountOff"
    ));

    // The route's output paid to someone else: the vault receives nothing.
    let mut stolen = env.jupiter_route_for(Asset::Eth, Sell::Usdc, plan).route;
    let vault_eth = get_associated_token_address(&env.eth.authority, &env.eth.asset_mint);
    let thief = get_associated_token_address(&frank, &env.eth.asset_mint);
    for meta in stolen.metas.iter_mut().filter(|m| m.pubkey == vault_eth) {
        meta.pubkey = thief;
    }
    let ix = env.deposit_ix_with(Asset::Eth, frank, 0, usdc, 0, Some(stolen));
    assert!(failed_with(
        env.send("frank", vec![ix], "stolen output"),
        "SwapPriceTooLow"
    ));

    // A route selling the vault's own ETH when the plan sells the deposit's USDC.
    let wrong_side = env.jupiter_route_for(Asset::Eth, Sell::Asset, ETH).route;
    let ix = env.deposit_ix_with(Asset::Eth, frank, 0, usdc, 0, Some(wrong_side));
    assert!(failed_with(
        env.send("frank", vec![ix], "wrong side"),
        "SwapMovedVaultFunds"
    ));

    // Only the whitelisted venue.
    let route = env.jupiter_route_for(Asset::Eth, Sell::Usdc, plan).route;
    let mut ix = env.deposit_ix_with(Asset::Eth, frank, 0, usdc, 0, Some(route));
    for meta in ix.accounts.iter_mut().filter(|m| m.pubkey == JUPITER) {
        meta.pubkey = anchor_spl::token::ID;
    }
    assert!(failed_with(
        env.send("frank", vec![ix], "other venue"),
        "InvalidSwapAccounts"
    ));

    // A route on a deposit already at the vault's mix.
    env.mint_asset(Asset::Eth, frank, ETH);
    let value = env.mark_price(Asset::Eth).value;
    let route = env.jupiter_route_for(Asset::Eth, Sell::Usdc, plan).route;
    let ix = env.deposit_ix_with(Asset::Eth, frank, ETH, value, 0, Some(route));
    assert!(failed_with(
        env.send("frank", vec![ix], "needless route"),
        "SwapNotNeeded"
    ));

    // Phoenix marking ETH 2.5% under what Jupiter charges: buying there would cost the vault.
    let price = env.jupiter_price(Asset::Eth, Sell::Usdc);
    env.move_price(Asset::Eth, (price * 0.975).round());
    assert!(failed_with(
        env.deposit_single(Asset::Eth, "frank", 0, usdc),
        "SwapPriceTooLow"
    ));
}
