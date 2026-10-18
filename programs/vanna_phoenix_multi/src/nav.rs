//! Vault valuation, hedge sizing and share pricing, in USD with 6 decimals (Phoenix quote
//! lots). Every leg is priced at its Phoenix mark: the same price Phoenix values its short and
//! the vault's margin with. The share math is the single-asset vault's, unchanged.

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

/// What Phoenix's perp asset map says about a market.
#[derive(Clone, Copy, Debug, Default)]
pub struct MarketState {
    pub open_interest_lots: u64,
    /// A stock market outside its session (weekends, holidays) or reopening after one.
    pub after_hours: bool,
}

/// Phoenix's margin view of the vault's trader account, in quote lots (USD, 6 dp). One
/// cross-margin account holds every leg's short.
#[derive(Clone, Copy, Debug, Default)]
pub struct Margin {
    /// USDC collateral, unrealized PnL and unsettled funding over every position: what the
    /// vault would have in USDC on Phoenix if it closed them all now. Negative when it owes.
    pub equity: i128,
    /// Phoenix's risk-weighted collateral, the measure its margin rules use.
    pub effective: i128,
    pub initial: u64,
    pub withdrawable: u64,
}

/// One leg as the vault values it.
#[derive(Clone, Copy, Debug)]
pub struct LegState {
    pub market: Market,
    pub mark_ticks: u64,
    pub price: Price,
    /// The leg's position in base lots; negative is short.
    pub position_lots: i64,
    /// Asset held: native SOL collateral in the trader account, or the asset tokens.
    pub units: u64,
}

impl LegState {
    pub fn short_lots(&self) -> u64 {
        (-self.position_lots).max(0) as u64
    }

    /// What the asset held is worth at the mark.
    pub fn value(&self) -> Result<u128> {
        self.price
            .usd_value(self.units as u128, self.market.decimals, false)
    }

    /// The short's notional at the mark (a long's, if Phoenix ever left one).
    pub fn notional(&self) -> Result<u128> {
        self.market
            .notional(self.position_lots.unsigned_abs(), self.mark_ticks)
    }
}

/// Everything the vault owns, read from Phoenix and its own accounts.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub legs: Vec<LegState>,
    /// The authority's USDC: the buffer.
    pub idle_usdc: u64,
    pub margin: Margin,
    pub supply: u64,
    /// Shares of live withdrawal requests (`Vault::pending_shares`).
    pub pending_shares: u64,
}

impl Snapshot {
    /// The vault's value before the floor at zero: negative when it owes Phoenix more than it
    /// holds.
    pub fn equity(&self) -> Result<i128> {
        let mut equity = self.margin.equity + self.idle_usdc as i128;
        for leg in &self.legs {
            equity = equity
                .checked_add(i128::try_from(leg.value()?).map_err(|_| VaultError::MathOverflow)?)
                .ok_or(VaultError::MathOverflow)?;
        }
        Ok(equity)
    }

    pub fn nav(&self) -> Result<u128> {
        Ok(self.equity()?.max(0) as u128)
    }

    /// Every leg's short notional together: what the vault's margin is held against.
    pub fn total_notional(&self) -> Result<u128> {
        self.legs.iter().try_fold(0u128, |sum, leg| {
            sum.checked_add(leg.notional()?)
                .ok_or_else(|| VaultError::MathOverflow.into())
        })
    }

    /// What the shares of live requests are worth now: USDC the keeper must keep for them.
    pub fn pending_value(&self) -> Result<u128> {
        if self.pending_shares == 0 {
            return Ok(0);
        }
        let nav = self.nav()?;
        withdrawal_payout(nav, nav, self.pending_shares, self.supply)
    }

    /// Idle USDC the keeper may not spend: the buffer and the pending requests' value.
    pub fn reserved_usdc(&self, buffer_bps: u16) -> Result<u128> {
        let buffer = mul_div(self.nav()?, buffer_bps as u128, BPS_SCALE, true)?;
        buffer
            .checked_add(self.pending_value()?)
            .ok_or_else(|| VaultError::MathOverflow.into())
    }
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

/// What a withdrawal of `shares` pays: the NAV after its own trade (here, after nothing: the
/// same NAV), less the part of the NAV before it that stays with the other holders.
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

/// The short to close with `units` of a leg's asset leaving: the same share of the short as
/// they are of the asset held, so the rest stays hedged as it was.
pub fn lots_to_close(short_lots: u64, held_units: u64, leaving: u64) -> u64 {
    if held_units == 0 {
        return 0;
    }
    let (short, held) = (short_lots as u128, held_units as u128);
    ((short * leaving as u128 + held / 2) / held).min(short) as u64
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
    /// METAx: a token worth 1.0028515 shares after reinvested dividends.
    const META: Market = Market {
        decimals: 8,
        base_lot_decimals: 3,
        tick_size: 10,
        multiplier: 1_002_851_543_327,
    };

    fn leg(market: Market, ticks: u64, position_lots: i64, units: u64) -> LegState {
        LegState {
            market,
            mark_ticks: ticks,
            price: market.price(ticks).unwrap(),
            position_lots,
            units,
        }
    }

    #[test]
    fn marks_become_prices_and_shorts_match_the_asset() {
        assert_eq!(SOL.price(150_000).unwrap().value, 150_000_000);
        assert_eq!(ETH.price(350_000).unwrap().value, 3_500_000_000);
        assert_eq!(SOL.hedge_lots(10_000_000_000).unwrap(), 1_000);
        assert_eq!(SOL.hedge_lots(10_005_000_000).unwrap(), 1_001);
        // 100 METAx hedge 100.285 META.
        assert_eq!(META.hedge_lots(10_000_000_000).unwrap(), 100_285);
    }

    #[test]
    fn nav_is_the_buffer_plus_every_leg_plus_phoenix_equity() {
        // $1,000 idle; 10 SOL held and short; 5 ETH held and short with $17,500 of USDC margin.
        let snap = Snapshot {
            legs: vec![
                leg(SOL, 150_000, -1_000, 10_000_000_000),
                leg(ETH, 350_000, -5_000, 500_000_000),
            ],
            idle_usdc: 1_000_000_000,
            margin: Margin {
                equity: 17_500_000_000,
                ..Default::default()
            },
            supply: 0,
            pending_shares: 0,
        };
        assert_eq!(
            snap.nav().unwrap(),
            1_000_000_000 + 1_500_000_000 + 17_500_000_000 + 17_500_000_000
        );
        assert_eq!(snap.total_notional().unwrap(), 1_500_000_000 + 17_500_000_000);
        // Owing Phoenix more than everything is worth: the NAV is zero, not negative.
        let under = Snapshot {
            margin: Margin {
                equity: -40_000_000_000,
                ..Default::default()
            },
            ..snap
        };
        assert!(under.equity().unwrap() < 0);
        assert_eq!(under.nav().unwrap(), 0);
    }

    #[test]
    fn pending_requests_and_the_buffer_reserve_idle_usdc() {
        // $10,000 vault, 10,000 shares, 1,000 of them in escrow: $1,000 pending, plus a 5% buffer.
        let snap = Snapshot {
            legs: vec![],
            idle_usdc: 10_000_000_000,
            margin: Margin::default(),
            supply: 10_000_000_000,
            pending_shares: 1_000_000_000,
        };
        assert_eq!(snap.pending_value().unwrap(), 1_000_000_000);
        assert_eq!(snap.reserved_usdc(500).unwrap(), 1_500_000_000);
    }

    #[test]
    fn limits_move_against_the_vault() {
        assert_eq!(limit_ticks(150_000, 100, true).unwrap(), 148_500);
        assert_eq!(limit_ticks(150_000, 100, false).unwrap(), 151_500);
    }

    #[test]
    fn margin_must_reach_the_target_or_not_get_thinner() {
        assert!(margin_ok((0, 0), (9_000, 10_000), 9_000, true));
        assert!(!margin_ok((0, 0), (8_999, 10_000), 9_000, true));
        assert!(margin_ok((800, 1_000), (1_600, 2_000), 9_000, true));
        assert!(!margin_ok((800, 1_000), (1_600, 2_000), 9_000, false));
        assert!(!margin_ok((800, 1_000), (1_500, 2_000), 9_000, true));
        assert!(margin_ok((800, 1_000), (0, 0), 9_000, false));
    }

    #[test]
    fn a_leaver_gets_their_part_and_a_round_trip_moves_no_value() {
        let usd = 1_000_000u128;
        assert_eq!(
            withdrawal_payout(1_000 * usd, 1_000 * usd, 500 * usd as u64, 1_000 * usd as u64)
                .unwrap(),
            500 * usd
        );
        assert!(withdrawal_payout(1_000, 1_000, 101, 100).is_err());
        // One share of a vault worth $50: depositing and leaving at once takes nothing.
        let (nav, supply) = (50_000_000u128, 1_000_000u64);
        for deposit in [1_000_000u128, 1_000_000_000, 100_000_000_000] {
            let shares = to_u64(shares_for_value(deposit, nav, supply).unwrap()).unwrap();
            let payout =
                withdrawal_payout(nav + deposit, nav + deposit, shares, supply + shares).unwrap();
            assert!(payout <= deposit, "${deposit}: paid {payout}");
        }
    }

    #[test]
    fn an_unwind_closes_its_share_of_the_short() {
        assert_eq!(lots_to_close(1_000, 10_000_000_000, 2_500_000_000), 250);
        assert_eq!(lots_to_close(1_000, 10_000_000_000, 10_000_000_000), 1_000);
        assert_eq!(lots_to_close(1_000, 10_000_000_000, 20_000_000_000), 1_000);
        assert_eq!(lots_to_close(1_000, 0, 1), 0);
    }
}
