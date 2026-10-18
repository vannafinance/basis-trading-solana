use crate::constants::{USD_DECIMALS, VIRTUAL_SHARES, VIRTUAL_USD};
use crate::errors::VaultError;
use anchor_lang::prelude::*;

#[derive(
    AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, PartialEq, Eq, InitSpace,
)]
pub struct Price {
    pub value: u64,
    pub expo: i32,
}

pub fn pow10(exp: u32) -> Result<u128> {
    require!(exp <= 38, VaultError::MathOverflow);
    Ok(10u128.pow(exp))
}

pub fn mul_div(a: u128, b: u128, denom: u128, round_up: bool) -> Result<u128> {
    require!(denom != 0, VaultError::DivisionByZero);
    let product = a.checked_mul(b).ok_or(VaultError::MathOverflow)?;
    Ok(if round_up {
        product.div_ceil(denom)
    } else {
        product / denom
    })
}

/// `value * 10^exp` for a signed `exp`.
fn scale(value: u128, exp: i64, round_up: bool) -> Result<u128> {
    if exp >= 0 {
        let factor = pow10(u32::try_from(exp).map_err(|_| VaultError::MathOverflow)?)?;
        value
            .checked_mul(factor)
            .ok_or_else(|| VaultError::MathOverflow.into())
    } else {
        let factor = pow10(u32::try_from(-exp).map_err(|_| VaultError::MathOverflow)?)?;
        Ok(if round_up {
            value.div_ceil(factor)
        } else {
            value / factor
        })
    }
}

impl Price {
    /// USD value (6 dp) of `amount` smallest units of a token with `decimals`.
    pub fn usd_value(&self, amount: u128, decimals: u8, round_up: bool) -> Result<u128> {
        let base = amount
            .checked_mul(self.value as u128)
            .ok_or(VaultError::MathOverflow)?;
        scale(
            base,
            self.expo as i64 - decimals as i64 + USD_DECIMALS as i64,
            round_up,
        )
    }

    /// Token amount (smallest units) worth `usd` (6 dp).
    pub fn tokens_for_usd(&self, usd: u128, decimals: u8, round_up: bool) -> Result<u128> {
        require!(self.value > 0, VaultError::InvalidPrice);
        let exp = decimals as i64 - self.expo as i64 - USD_DECIMALS as i64;
        if exp >= 0 {
            mul_div(scale(usd, exp, false)?, 1, self.value as u128, round_up)
        } else {
            mul_div(usd, 1, scale(self.value as u128, -exp, false)?, round_up)
        }
    }
}

/// Shares worth `value` (USD, 6 dp) in a vault worth `nav` with `supply` shares. The virtual
/// offset prices the first share at one dollar and stops share-price inflation attacks.
pub fn shares_for_value(value: u128, nav: u128, supply: u64) -> Result<u128> {
    mul_div(
        value,
        supply as u128 + VIRTUAL_SHARES,
        nav.checked_add(VIRTUAL_USD)
            .ok_or(VaultError::MathOverflow)?,
        false,
    )
}

pub fn to_u64(value: u128) -> Result<u64> {
    u64::try_from(value).map_err(|_| VaultError::MathOverflow.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOL: Price = Price {
        value: 150_000_000,
        expo: -6,
    };

    #[test]
    fn values_convert_both_ways() {
        // 10 SOL at 150 USD = 1,500 USD in 6 dp.
        assert_eq!(
            SOL.usd_value(10_000_000_000, 9, false).unwrap(),
            1_500_000_000
        );
        assert_eq!(
            SOL.tokens_for_usd(1_500_000_000, 9, false).unwrap(),
            10_000_000_000
        );
    }

    #[test]
    fn the_first_dollar_buys_one_share() {
        assert_eq!(shares_for_value(1_000_000, 0, 0).unwrap(), 1_000_000);
    }
}
