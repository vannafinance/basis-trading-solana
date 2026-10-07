//! Deposits and withdrawals through real Jupiter. The user brings USDC and gets USDC back; the
//! vault runs a Jupiter v6 route over a real Orca Whirlpool pool (snapshotted from mainnet by
//! `scripts/jupiter_fork.py`), resized to the exact amount it sells, and books what Jupiter
//! actually delivered, never the quote: the asset it holds, the short, the margin and the
//! shares.

use crate::env::*;
use anchor_spl::associated_token::get_associated_token_address;
use vanna_phoenix_vault::constants::NATIVE_MINT;
use vanna_phoenix_vault::events::{Deposited, Swapped, Withdrawn};
use vanna_phoenix_vault::instructions::VaultParams;
use vanna_phoenix_vault::swap::Sell;

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
fn eth_vault_buys_half_in_eth_and_shorts_what_arrived() {
    let mut env = jupiter_env();
    let frank = env.add_user("frank", 10 * SOL);
    let quoted = env
        .jupiter_route_for(Asset::Eth, Sell::Usdc, 3_500 * USDC)
        .quoted_out;

    let meta = expect(env.deposit(Asset::Eth, "frank", 7_000 * USDC));
    let swap = &events::<Swapped>(&meta)[0];
    let dep = &events::<Deposited>(&meta)[0];
    report(
        "USDC -> ETH",
        quoted,
        swap,
        SPOT_DECIMALS,
        meta.compute_units_consumed,
    );
    let v = env.print(Asset::Eth, "frank deposits $7,000");

    assert!(swap.sold_usdc);
    assert_eq!(swap.spent, 3_500 * USDC, "half the USDC buys ETH");
    assert!(
        swap.received * 1_000 >= quoted * 995,
        "Jupiter delivered its quote"
    );
    assert_eq!(dep.asset_bought, swap.received);
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
fn btc_vault_two_usdc_deposits_at_one_share_price() {
    let mut env = jupiter_env();
    env.add_user("dan", 10 * SOL);
    env.add_user("erin", 10 * SOL);

    let quoted = env
        .jupiter_route_for(Asset::Btc, Sell::Usdc, 10_000 * USDC)
        .quoted_out;
    let meta = expect(env.deposit(Asset::Btc, "dan", 20_000 * USDC));
    let swap = &events::<Swapped>(&meta)[0];
    report(
        "USDC -> cbBTC",
        quoted,
        swap,
        SPOT_DECIMALS,
        meta.compute_units_consumed,
    );
    let one = env.print(Asset::Btc, "dan deposits $20,000");
    assert_eq!(one.asset_units, swap.received);

    expect(env.deposit(Asset::Btc, "erin", 10_000 * USDC));
    let both = env.print(Asset::Btc, "erin deposits $10,000");
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
fn sol_vault_buys_sol_with_all_of_it_and_shorts_all_of_it() {
    let mut env = jupiter_env();
    let gina = env.add_user("gina", 10 * SOL);
    let sol_before = env.asset_balance(Asset::Sol, gina);
    let quoted = env
        .jupiter_route_for(Asset::Sol, Sell::Usdc, 1_000 * USDC)
        .quoted_out;

    let meta = expect(env.deposit(Asset::Sol, "gina", 1_000 * USDC));
    let swap = &events::<Swapped>(&meta)[0];
    report("USDC -> SOL", quoted, swap, 9, meta.compute_units_consumed);
    let v = env.print(Asset::Sol, "gina deposits $1,000");

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
}

#[test]
fn eth_vault_pays_out_in_usdc_and_sells_exactly_the_leavers_eth() {
    let mut env = jupiter_env();
    let carol = env.add_user("carol", 10 * SOL);
    expect(env.deposit(Asset::Eth, "carol", 7_000 * USDC));
    let start = env.print(Asset::Eth, "carol deposits $7,000");
    let half = env.shares(Asset::Eth, carol) / 2;

    let meta = expect(env.withdraw(Asset::Eth, "carol", half));
    let swap = &events::<Swapped>(&meta)[0];
    let out = &events::<Withdrawn>(&meta)[0];
    env.print(Asset::Eth, "carol withdraws half");
    eprintln!(
        "USDC out: ${:.2} (Jupiter sold {:.8} ETH)",
        usd(out.usdc_out),
        swap.spent as f64 / ETH as f64,
    );
    assert_eq!(swap.spent, out.asset_sold, "the route sold all of it");
    assert!(usd(out.usdc_out) > start.nav_usd / 2.0 * 0.98);

    let rest = env.shares(Asset::Eth, carol);
    let meta = expect(env.withdraw(Asset::Eth, "carol", rest));
    let swap = &events::<Swapped>(&meta)[0];
    let out = &events::<Withdrawn>(&meta)[0];
    let end = env.print(Asset::Eth, "carol withdraws the rest");
    assert_eq!(swap.spent, out.asset_sold);
    assert_eq!(end.supply, 0);
    assert_eq!(end.position_lots, 0);
    assert_eq!(env.asset_balance(Asset::Eth, carol), 0, "no ETH comes back");
    let held = usd(env.usdc(carol));
    eprintln!("carol: put in $7,000, holds ${held:.2} USDC after two swaps each way and two trades");
    assert!(held > 7_000.0 * 0.98);
}

#[test]
fn sol_vault_pays_out_in_usdc() {
    let mut env = jupiter_env();
    let alice = env.add_user("alice", 10 * SOL);
    expect(env.deposit(Asset::Sol, "alice", 1_500 * USDC));
    let shares = env.shares(Asset::Sol, alice);
    let sol_before = env.asset_balance(Asset::Sol, alice);
    let meta = expect(env.withdraw(Asset::Sol, "alice", shares / 2));
    let swap = &events::<Swapped>(&meta)[0];
    let out = &events::<Withdrawn>(&meta)[0];
    let v = env.print(Asset::Sol, "alice withdraws half");
    eprintln!(
        "USDC out: ${:.2} (Jupiter sold {:.4} SOL)",
        usd(out.usdc_out),
        swap.spent as f64 / SOL as f64,
    );
    assert_eq!(swap.spent, out.asset_sold, "the route sold all of it");
    assert_eq!(env.usdc(alice), out.usdc_out);
    assert!(usd(out.usdc_out) > 1_500.0 / 2.0 * 0.98);
    assert_eq!(
        sol_before - env.asset_balance(Asset::Sol, alice),
        meta.fee,
        "no SOL comes back: the wrapped-SOL account's rent is returned, and only the fee is paid"
    );
    // Hedged to the lot; the leaver's trading cost can leave one lot until the next trade.
    assert!((v.position_lots + v.asset_units as i64 / 10_000_000).abs() <= 1);
}

#[test]
fn routes_are_resized_and_bad_routes_are_refused() {
    let mut env = jupiter_env();
    let frank = env.add_user("frank", 10 * SOL);
    env.mint_usdc(frank, 100_000 * USDC);
    let usdc = 7_000 * USDC;
    let plan = 3_500 * USDC;

    // An honest deposit first, so the vault holds ETH a bad route could reach for. Its route
    // was built for a third of the plan; resized, the vault buys with exactly half the USDC.
    let third = env
        .jupiter_route_for(Asset::Eth, Sell::Usdc, plan / 3)
        .route;
    let ix = env.deposit_ix_with(Asset::Eth, frank, usdc, 0, third);
    let meta = expect(env.send("frank", vec![ix], "a third of the plan"));
    assert_eq!(events::<Swapped>(&meta)[0].spent, plan);

    // The route's output paid to someone else: the vault receives nothing.
    let mut stolen = env.jupiter_route_for(Asset::Eth, Sell::Usdc, plan).route;
    let vault_eth = get_associated_token_address(&env.eth.authority, &env.eth.asset_mint);
    let thief = get_associated_token_address(&frank, &env.eth.asset_mint);
    for meta in stolen.metas.iter_mut().filter(|m| m.pubkey == vault_eth) {
        meta.pubkey = thief;
    }
    let ix = env.deposit_ix_with(Asset::Eth, frank, usdc, 0, stolen);
    assert!(failed_with(
        env.send("frank", vec![ix], "stolen output"),
        "SwapPriceTooLow"
    ));

    // A route selling the vault's own ETH when a deposit buys ETH with USDC. A $1 deposit, so
    // the resized route ($0.50 in ETH units: 0.005 ETH) is within what the vault holds.
    let wrong_side = env.jupiter_route_for(Asset::Eth, Sell::Asset, ETH).route;
    let ix = env.deposit_ix_with(Asset::Eth, frank, USDC, 0, wrong_side);
    assert!(failed_with(
        env.send("frank", vec![ix], "wrong side"),
        "SwapMovedVaultFunds"
    ));

    // Only the whitelisted venue.
    let route = env.jupiter_route_for(Asset::Eth, Sell::Usdc, plan).route;
    let mut ix = env.deposit_ix_with(Asset::Eth, frank, usdc, 0, route);
    for meta in ix.accounts.iter_mut().filter(|m| m.pubkey == JUPITER) {
        meta.pubkey = anchor_spl::token::ID;
    }
    assert!(failed_with(
        env.send("frank", vec![ix], "other venue"),
        "InvalidSwapAccounts"
    ));

    // Only Jupiter's own route instructions, which the vault knows how to resize.
    let mut other = env.jupiter_route_for(Asset::Eth, Sell::Usdc, plan).route;
    other.args.data[0] ^= 1;
    let ix = env.deposit_ix_with(Asset::Eth, frank, usdc, 0, other);
    assert!(failed_with(
        env.send("frank", vec![ix], "other instruction"),
        "UnsupportedRoute"
    ));

    // Phoenix marking ETH 2.5% under what Jupiter charges: buying there would cost the vault.
    let price = env.jupiter_price(Asset::Eth, Sell::Usdc);
    env.move_price(Asset::Eth, (price * 0.975).round());
    assert!(failed_with(
        env.deposit(Asset::Eth, "frank", usdc),
        "SwapPriceTooLow"
    ));
}
