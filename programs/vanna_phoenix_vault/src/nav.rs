//! Vault valuation, hedge sizing and share pricing, in USD with 6 decimals (Phoenix quote
//! lots). Everything is priced at Phoenix's mark price: the same price Phoenix values the
//! short and its margin with.

use crate::constants::*;
use crate::errors::VaultError;
use crate::math::*;
use anchor_lang::prelude::*;

/// A Phoenix market's lot and tick sizes, with the asset's token decimals and how many of
/// the market's units (shares, for a stock) one token is worth.
#[derive(Clone, Copy, Debug)]
pub struct Market {
    pub decimals: u8,
    pub base_lot_decimals: u8,
    /// Quote lots per base lot per tick.
    pub tick_size: u64,
    /// The token's UI multiplier over `MULTIPLIER_SCALE`: `MULTIPLIER_SCALE` for plain tokens;
    /// above it for a tokenized stock that reinvested dividends.
    pub multiplier: u128,
}

impl Market {
    /// USD (6 dp) per whole token at `ticks`: the market's price times the multiplier.
    pub fn price(&self, ticks: u64) -> Result<Price> {
        require!(ticks > 0, VaultError::InvalidPrice);
        let value = (ticks as u128)
            .checked_mul(self.tick_size as u128)
            .and_then(|v| v.checked_mul(10u128.pow(self.base_lot_decimals as u32)))
            .ok_or(VaultError::MathOverflow)?;
        Ok(Price {
            value: to_u64(mul_div(value, self.multiplier, MULTIPLIER_SCALE, false)?)?,
            expo: -(USD_DECIMALS as i32),
        })
    }

    /// Smallest token units in one base lot, before the multiplier.
    pub fn units_per_lot(&self) -> Result<u64> {
        require!(
            self.decimals >= self.base_lot_decimals,
            VaultError::InvalidParameter
        );
        to_u64(pow10((self.decimals - self.base_lot_decimals) as u32)?)
    }

    /// The short, in base lots, that hedges `units` of the token: what they are worth in the
    /// market's units, rounded to the nearest lot.
    pub fn hedge_lots(&self, units: u64) -> Result<u64> {
        let per_lot = self.units_per_lot()? as u128 * MULTIPLIER_SCALE;
        let worth = (units as u128)
            .checked_mul(self.multiplier)
            .ok_or(VaultError::MathOverflow)?;
        to_u64((worth + per_lot / 2) / per_lot)
    }

    /// USD notional (6 dp) of `lots` base lots at `ticks`.
    pub fn notional(&self, lots: u64, ticks: u64) -> Result<u128> {
        (lots as u128)
            .checked_mul(ticks as u128)
            .and_then(|v| v.checked_mul(self.tick_size as u128))
            .ok_or_else(|| VaultError::MathOverflow.into())
    }
}

/// What the vault owns, read from Phoenix and its own accounts.
#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub mark_ticks: u64,
    pub price: Price,
    /// The vault's position in base lots; negative is short.
    pub position_lots: i64,
    /// Asset held: native SOL collateral in the trader account, or the vault's asset tokens.
    pub asset_units: u64,
    pub margin: Margin,
    /// The market's open interest in base lots and whether Phoenix trades it after hours.
    pub market: MarketState,
}

/// What Phoenix's perp asset map says about the vault's market.
#[derive(Clone, Copy, Debug, Default)]
pub struct MarketState {
    pub open_interest_lots: u64,
    /// A stock market outside its session (weekends, holidays) or reopening after one.
    pub after_hours: bool,
}

/// Phoenix's margin view of the vault, in quote lots (USD, 6 dp).
#[derive(Clone, Copy, Debug, Default)]
pub struct Margin {
    /// USDC collateral, unrealized PnL and unsettled funding: what the vault would have in
    /// USDC if it closed its position now. Negative when it owes USDC.
    pub equity: i128,
    /// Phoenix's risk-weighted collateral, the measure its margin rules use.
    pub effective: i128,
    /// The part of `effective` that native SOL collateral adds (after Phoenix's haircut).
    pub sol_effective: i128,
    pub initial: u64,
    /// USDC Phoenix lets the vault take out now. Native SOL never backs a USDC withdrawal.
    pub withdrawable: u64,
}

impl Snapshot {
    pub fn short_lots(&self) -> u64 {
        (-self.position_lots).max(0) as u64
    }

    pub fn nav(&self, market: &Market) -> Result<u128> {
        Ok(self.equity(market)?.max(0) as u128)
    }

    /// The vault's value before the floor at zero: negative when it owes Phoenix more than it
    /// holds.
    pub fn equity(&self, market: &Market) -> Result<i128> {
        equity(market, &self.price, self.asset_units, &self.margin)
    }

    pub fn notional(&self, market: &Market) -> Result<u128> {
        market.notional(self.position_lots.unsigned_abs(), self.mark_ticks)
    }

    /// Native SOL Phoenix lets the vault withdraw: what stays must still cover the initial
    /// margin. Each lamport out takes its haircut value out of the effective collateral and,
    /// when the short shrinks with it (`short_shrinks`), frees its share of the short's
    /// initial margin. Kept 0.1% short of the limit for the gap between Phoenix's index
    /// price and the mark price.
    pub fn native_withdrawable(&self, market: &Market, short_shrinks: bool) -> Result<u64> {
        let m = &self.margin;
        let units = self.asset_units as u128;
        if m.sol_effective <= 0 || units == 0 {
            return Ok(self.asset_units);
        }
        let free = (m.effective - m.initial as i128).max(0) as u128;
        // What taking out the whole holding would cost in collateral.
        let mut cost = m.sol_effective as u128;
        let short_units = self.short_lots() as u128 * market.units_per_lot()? as u128;
        if short_shrinks && short_units > 0 {
            let freed = mul_div(m.initial as u128, units, short_units, false)?;
            if freed >= cost {
                return Ok(self.asset_units);
            }
            cost -= freed;
        }
        let limit = mul_div(free, units, cost, false)?;
        Ok(mul_div(limit, 999, 1_000, false)?.min(units) as u64)
    }
}

/// The vault's value: the asset it holds plus its Phoenix equity, floored at zero.
pub fn nav(market: &Market, price: &Price, asset_units: u64, margin: &Margin) -> Result<u128> {
    Ok(equity(market, price, asset_units, margin)?.max(0) as u128)
}

fn equity(market: &Market, price: &Price, asset_units: u64, margin: &Margin) -> Result<i128> {
    let asset = price.usd_value(asset_units as u128, market.decimals, false)?;
    i128::try_from(asset)
        .ok()
        .and_then(|asset| asset.checked_add(margin.equity))
        .ok_or_else(|| VaultError::MathOverflow.into())
}

/// Phoenix limit price for a market order `bps` worse than the mark price.
pub fn limit_ticks(mark_ticks: u64, bps: u16, sell: bool) -> Result<u64> {
    let factor = if sell {
        BPS_SCALE.saturating_sub(bps as u128)
    } else {
        BPS_SCALE + bps as u128
    };
    to_u64(mul_div(mark_ticks as u128, factor, BPS_SCALE, sell)?)
}

/// Whether the vault keeps at least `margin_bps` of effective collateral per dollar of short
/// after a change, or at least does not end up thinner than before it.
pub fn margin_ok(
    before: (i128, u128),
    after: (i128, u128),
    margin_bps: u16,
    allow_not_worse: bool,
) -> bool {
    let (eff_before, notional_before) = before;
    let (eff_after, notional_after) = after;
    if notional_after == 0 {
        return true;
    }
    let target = eff_after.saturating_mul(BPS_SCALE as i128)
        >= (notional_after as i128).saturating_mul(margin_bps as i128);
    let not_worse = notional_before > 0
        && eff_after.saturating_mul(notional_before as i128)
            >= eff_before.saturating_mul(notional_after as i128);
    target || (allow_not_worse && not_worse)
}

/// What a withdrawal of `shares` pays: the NAV after its own trade, less the part of the NAV
/// before it that stays with the other holders. The leaver carries their own trading cost.
///
/// The part that stays is priced like a deposit (`shares_for_value`), with the virtual shares
/// and value, so depositing and withdrawing at once never moves value between holders, even in
/// a nearly empty vault whose share price is far from a dollar.
pub fn withdrawal_payout(
    nav_before: u128,
    nav_after_trade: u128,
    shares: u64,
    supply: u64,
) -> Result<u128> {
    require!(shares > 0 && shares <= supply, VaultError::NotEnoughShares);
    let retained = mul_div(
        nav_before
            .checked_add(VIRTUAL_USD)
            .ok_or(VaultError::MathOverflow)?,
        (supply - shares) as u128 + VIRTUAL_SHARES,
        supply as u128 + VIRTUAL_SHARES,
        true,
    )?
    .saturating_sub(VIRTUAL_USD);
    Ok(nav_after_trade.saturating_sub(retained))
}

/// Rounding the payout into asset units and USDC atoms can leave it a few quote lots short;
/// that still counts as paid in full.
pub const PAYOUT_DUST: u128 = 100;

/// Shares a withdrawal burns when Phoenix lets it take out only `paid` of its `payout`: the
/// leaver keeps the rest and can take it once the vault's margin allows. `cost` is what the
/// leaver's trade already took out of the vault; the leaver carries all of it, so it counts as
/// paid. Rounds up, for the vault.
pub fn shares_burned(shares: u64, paid: u128, payout: u128, cost: u128) -> Result<u64> {
    if paid + PAYOUT_DUST >= payout {
        return Ok(shares);
    }
    to_u64(mul_div(shares as u128, paid + cost, payout + cost, true)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOL: Market = Market {
        decimals: 9,
        base_lot_decimals: 2,
        tick_size: 10,
        multiplier: MULTIPLIER_SCALE,
    };
    const ETH: Market = Market {
        decimals: 8,
        base_lot_decimals: 3,
        tick_size: 10,
        multiplier: MULTIPLIER_SCALE,
    };
    /// METAx: 8 decimals, Phoenix's META market in 0.001-share lots, and a token worth
    /// 1.0028515 shares after reinvested dividends.
    const META: Market = Market {
        decimals: 8,
        base_lot_decimals: 3,
        tick_size: 10,
        multiplier: 1_002_851_543_327,
    };

    #[test]
    fn mark_ticks_become_usd_prices() {
        // SOL at 150 USD is 150,000 ticks: 0.01 SOL lots, 10 quote lots per tick.
        assert_eq!(SOL.price(150_000).unwrap().value, 150_000_000);
        // ETH at 3,500 USD is 350,000 ticks with 0.001 ETH lots.
        assert_eq!(ETH.price(350_000).unwrap().value, 3_500_000_000);
    }

    #[test]
    fn the_hedge_matches_the_asset_to_the_nearest_lot() {
        assert_eq!(SOL.units_per_lot().unwrap(), 10_000_000);
        assert_eq!(SOL.hedge_lots(10_000_000_000).unwrap(), 1_000);
        assert_eq!(SOL.hedge_lots(10_004_999_999).unwrap(), 1_000);
        assert_eq!(SOL.hedge_lots(10_005_000_000).unwrap(), 1_001);
        assert_eq!(ETH.hedge_lots(500_000_000).unwrap(), 5_000);
        // 10 SOL short at 150 USD is 1,500 USD.
        assert_eq!(SOL.notional(1_000, 150_000).unwrap(), 1_500_000_000);
    }

    #[test]
    fn a_stock_token_is_priced_and_hedged_at_its_multiplier() {
        // META at 746 USD is 74,600 ticks; one METAx is 1.0028515 shares.
        assert_eq!(META.price(74_600).unwrap().value, 748_127_251);
        // 100 METAx hedge 100.285 META: 100,285 lots, not 100,000.
        assert_eq!(META.hedge_lots(10_000_000_000).unwrap(), 100_285);
        // The short and the tokens are worth the same at the mark.
        let tokens = META
            .price(74_600)
            .unwrap()
            .usd_value(10_000_000_000, 8, false)
            .unwrap();
        let short = META.notional(100_285, 74_600).unwrap();
        assert!(tokens.abs_diff(short) < 1_000_000, "{tokens} vs {short}");
    }

    #[test]
    fn nav_is_the_asset_plus_phoenix_equity() {
        let price = SOL.price(150_000).unwrap();
        let margin = Margin {
            equity: -300_000_000,
            ..Default::default()
        };
        // 10 SOL (1,500 USD) less 300 USD owed.
        assert_eq!(
            nav(&SOL, &price, 10_000_000_000, &margin).unwrap(),
            1_200_000_000
        );
        let underwater = Margin {
            equity: -2_000_000_000,
            ..Default::default()
        };
        assert_eq!(nav(&SOL, &price, 10_000_000_000, &underwater).unwrap(), 0);
    }

    #[test]
    fn limits_move_against_the_vault() {
        assert_eq!(limit_ticks(150_000, 100, true).unwrap(), 148_500);
        assert_eq!(limit_ticks(150_000, 100, false).unwrap(), 151_500);
    }

    #[test]
    fn margin_must_reach_the_target_or_not_get_thinner() {
        // 9,000 USD effective on a 10,000 USD short is 90%.
        assert!(margin_ok((0, 0), (9_000, 10_000), 9_000, true));
        assert!(!margin_ok((0, 0), (8_999, 10_000), 9_000, true));
        // Below target but no thinner than before: allowed for deposits only.
        assert!(margin_ok((800, 1_000), (1_600, 2_000), 9_000, true));
        assert!(!margin_ok((800, 1_000), (1_600, 2_000), 9_000, false));
        assert!(!margin_ok((800, 1_000), (1_500, 2_000), 9_000, true));
    }

    #[test]
    fn a_partial_payout_burns_only_what_it_paid_for() {
        assert_eq!(shares_burned(100, 990_000, 990_000, 0).unwrap(), 100);
        assert_eq!(
            shares_burned(100, 989_901, 990_000, 0).unwrap(),
            100,
            "rounding dust"
        );
        assert_eq!(shares_burned(100, 495_000, 990_000, 0).unwrap(), 50);
        assert_eq!(shares_burned(100, 1, 990_000, 0).unwrap(), 1);
        assert_eq!(shares_burned(100, 0, 990_000, 0).unwrap(), 0);
        // Half paid out of a $990 payout whose trade cost $10: the leaver's $10 counts as
        // paid, so they burn 50.5 shares, not 50, and the others carry none of it.
        assert_eq!(shares_burned(100, 495_000, 990_000, 10_000).unwrap(), 51);
    }

    #[test]
    fn sol_collateral_withdrawable_keeps_the_initial_margin_covered() {
        // 10 SOL at 150, 80% counted: 1,200 effective, less 400 USDC owed. Short 10 SOL with
        // 75 USD (5%) initial margin, so 725 USD is free.
        let mut snap = Snapshot {
            mark_ticks: 150_000,
            price: SOL.price(150_000).unwrap(),
            position_lots: -1_000,
            asset_units: 10_000_000_000,
            margin: Margin {
                equity: -400_000_000,
                effective: 800_000_000,
                sol_effective: 1_200_000_000,
                initial: 75_000_000,
                withdrawable: 0,
            },
            market: MarketState::default(),
        };
        // Each SOL out costs 120 USD of collateral and frees 7.5 USD of margin: 725 / 112.5.
        assert_eq!(snap.native_withdrawable(&SOL, true).unwrap(), 6_437_999_999);
        // If the short stays, nothing is freed: 725 / 120.
        assert_eq!(
            snap.native_withdrawable(&SOL, false).unwrap(),
            6_035_624_999
        );
        // With no short left, all of the free collateral backs SOL leaving.
        snap.position_lots = 0;
        snap.margin.initial = 0;
        assert_eq!(
            snap.native_withdrawable(&SOL, false).unwrap(),
            6_659_999_999
        );
    }

    #[test]
    fn a_leaver_gets_their_part_less_their_own_cost() {
        // Half the shares of a 1,000 USD vault, after a 1 USD trade cost.
        let usd = 1_000_000;
        assert_eq!(
            withdrawal_payout(1_000 * usd, 999 * usd, 500 * usd as u64, 1_000 * usd as u64)
                .unwrap(),
            499 * usd
        );
        assert_eq!(
            withdrawal_payout(
                1_000 * usd,
                990 * usd,
                1_000 * usd as u64,
                1_000 * usd as u64
            )
            .unwrap(),
            990 * usd
        );
        assert!(withdrawal_payout(1_000, 990, 101, 100).is_err());
    }

    #[test]
    fn a_deposit_and_withdrawal_at_once_move_no_value_even_in_a_tiny_vault() {
        // One share of a vault worth $50 (a donation made the share price 50x a dollar).
        let (nav, supply) = (50_000_000u128, 1_000_000u64);
        for deposit in [1_000_000u128, 1_000_000_000, 100_000_000_000] {
            let shares = to_u64(shares_for_value(deposit, nav, supply).unwrap()).unwrap();
            let payout =
                withdrawal_payout(nav + deposit, nav + deposit, shares, supply + shares).unwrap();
            assert!(payout <= deposit, "${deposit}: paid {payout}");
        }
        // A share at a normal price comes back at that price.
        let shares =
            to_u64(shares_for_value(500_000_000, 1_003_000_000, 1_000_000_000).unwrap()).unwrap();
        let payout =
            withdrawal_payout(1_503_000_000, 1_503_000_000, shares, 1_000_000_000 + shares)
                .unwrap();
        assert!(500_000_000 - payout < 2, "{payout}");
    }
}
