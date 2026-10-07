//! Vault caps: the short's share of the market's open interest, the size of one deposit, the
//! short's notional and, for the SOL vault, the native SOL Phoenix counts as collateral. Each
//! test starts from the caps the vault launches with (`mainnet_params`).

use crate::env::*;
use vanna_phoenix_vault::instructions::VaultParams;

fn expect<T>(result: Result<T, Vec<String>>) -> T {
    result.unwrap_or_else(|logs| panic!("{}", logs.join("\n")))
}

/// The vault's short as a share of the market's open interest.
fn oi_share(env: &mut Env, asset: Asset) -> f64 {
    let short = -env.view(asset).position_lots as f64;
    short / env.open_interest_lots(asset) as f64
}

#[test]
fn tsla_caps_open_interest_deposit_size_and_position() {
    let mut env = Env::new();
    env.update_vault(Asset::Tsla, mainnet_params(Asset::Tsla));
    let fay = env.add_user("fay", 10 * SOL);
    // $91,680: 120 TSLAx and $45,840 of margin, under the $100,000 a deposit may add.
    let chunk = |env: &mut Env| env.deposit(Asset::Tsla, "fay", 91_680 * USDC);

    // Alone, the vault would be every short on the market.
    assert!(failed_with(chunk(&mut env), "OpenInterestShareReached"));

    // Other traders short 1,200 TSLA.
    env.outside_trade(Asset::Tsla, true, 1_200_000);
    eprintln!(
        "open interest {:.3} TSLA after others short 1,200",
        env.open_interest_lots(Asset::Tsla) as f64 / 1_000.0
    );
    assert_eq!(env.open_interest_lots(Asset::Tsla), 1_200_000);

    assert!(
        failed_with(
            env.deposit(Asset::Tsla, "fay", 114_600 * USDC),
            "DepositTooLarge"
        ),
        "$114,600 at once"
    );
    for n in 1..=3 {
        expect(chunk(&mut env));
        let share = oi_share(&mut env, Asset::Tsla);
        eprintln!(
            "deposit {n}: vault short is {:.1}% of open interest",
            share * 100.0
        );
        assert!(share <= 0.25);
    }
    // A fourth would make the vault 28.6% of the shorts.
    assert!(failed_with(chunk(&mut env), "OpenInterestShareReached"));
    env.print(Asset::Tsla, "fay, three deposits");

    // A lower position cap stops a deposit that grows the short.
    env.update_vault(
        Asset::Tsla,
        VaultParams {
            max_position_usd: 120_000 * USDC,
            ..mainnet_params(Asset::Tsla)
        },
    );
    assert!(failed_with(
        env.deposit(Asset::Tsla, "fay", 764 * USDC),
        "MaxPositionReached"
    ));

    // Caps never hold anyone in: fay leaves while the vault is over its cap.
    let shares = env.shares(Asset::Tsla, fay);
    expect(env.withdraw(Asset::Tsla, "fay", shares));
    env.print(Asset::Tsla, "fay withdraws everything");
    assert_eq!(env.view(Asset::Tsla).position_lots, 0);
}

#[test]
fn phoenix_counts_only_500_sol_per_trader_so_the_sol_vault_stops_at_450() {
    // Without the cap: SOL past Phoenix's 500 per trader is held but not counted as margin.
    let mut env = Env::new();
    env.add_user("gus", 10 * SOL);
    expect(env.deposit(Asset::Sol, "gus", 75_000 * USDC));
    let at_500 = env.print(Asset::Sol, "gus deposits $75,000 (500 SOL)");
    expect(env.deposit(Asset::Sol, "gus", 15_000 * USDC));
    let at_600 = env.print(Asset::Sol, "gus deposits $15,000 (100 SOL) more");
    let notional = |v: &VaultView| -v.position_lots as f64 / 100.0 * v.mark_usd;
    eprintln!(
        "margin: ${:.0} on a ${:.0} short ({:.1}%) -> ${:.0} on ${:.0} ({:.1}%): the 100 SOL added no margin",
        at_500.effective_usd,
        notional(&at_500),
        at_500.effective_usd / notional(&at_500) * 100.0,
        at_600.effective_usd,
        notional(&at_600),
        at_600.effective_usd / notional(&at_600) * 100.0,
    );
    assert!(at_600.effective_usd - at_500.effective_usd < 100.0);

    // With the launch caps the vault stops at 450 SOL.
    let mut env = Env::new();
    env.update_vault(Asset::Sol, mainnet_params(Asset::Sol));
    env.outside_trade(Asset::Sol, true, 200_000);
    let gus = env.add_user("gus", 10 * SOL);
    expect(env.deposit(Asset::Sol, "gus", 67_500 * USDC));
    env.print(Asset::Sol, "gus deposits $67,500 (450 SOL)");
    assert!(failed_with(
        env.deposit(Asset::Sol, "gus", 150 * USDC),
        "MaxPositionReached"
    ));
    let shares = env.shares(Asset::Sol, gus);
    expect(env.withdraw(Asset::Sol, "gus", shares / 10));
    expect(env.deposit(Asset::Sol, "gus", 1_500 * USDC));
}

#[test]
fn the_keeper_closes_deposits_when_funding_turns_and_reopens_them() {
    let mut env = Env::new();
    let hal = env.add_user("hal", 10 * SOL);
    expect(env.deposit(Asset::Sol, "hal", 1_500 * USDC));

    let keeper = env.keeper_seed();
    expect(env.set_deposits_open(keeper, Asset::Sol, false));
    assert!(failed_with(
        env.deposit(Asset::Sol, "hal", 1_500 * USDC),
        "DepositsClosed"
    ));
    assert!(failed_with(
        env.set_deposits_open("hal", Asset::Sol, true),
        "Unauthorized"
    ));
    // Holders still leave while deposits are closed.
    let shares = env.shares(Asset::Sol, hal);
    expect(env.withdraw(Asset::Sol, "hal", shares / 2));

    expect(env.set_deposits_open(keeper, Asset::Sol, true));
    expect(env.deposit(Asset::Sol, "hal", 1_500 * USDC));
}
