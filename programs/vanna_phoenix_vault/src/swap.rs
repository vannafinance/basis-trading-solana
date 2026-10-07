//! Swaps through an outside venue (Jupiter on mainnet), so a user can deposit or withdraw a
//! single asset.
//!
//! The vault plans how much to swap; the client builds a route for that amount with the vault
//! authority as the trader and passes the route's accounts after Phoenix's index accounts.
//! The vault signs the route, then checks what actually moved: the side it sold went down by
//! about the planned amount, the side it bought went up at no worse than `slippage_bps` from
//! Phoenix's mark price, and nothing else it owns changed.

use crate::constants::*;
use crate::errors::VaultError;
use crate::math::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;

/// A venue instruction built off-chain for the vault authority.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct SwapRoute {
    /// How many `remaining_accounts` after Phoenix's index accounts the route takes.
    pub accounts: u8,
    pub data: Vec<u8>,
}

/// How a withdrawal is paid.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receive {
    /// The leaver's part of the asset, and the rest in USDC. No swap.
    Both,
    /// Everything in the asset: the USDC part is swapped into it.
    Asset,
    /// Everything in USDC: the asset part is swapped into it.
    Usdc,
}

/// What a swap sells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sell {
    Usdc,
    Asset,
}

/// How much of a deposit to swap so it lands at the vault's target mix: `target_margin_bps`
/// of USDC per dollar of asset. USDC alone keeps `t / (1 + t)` as margin and buys the asset
/// with the rest; the asset alone keeps `1 / (1 + t)` and sells the rest for margin. Both
/// splits are fixed fractions of the input, so the client can build the route for the exact
/// amount. A mixed deposit swaps whatever side is over the mix, valued at the mark price.
pub fn plan_deposit_swap(
    price: &Price,
    decimals: u8,
    target_margin_bps: u16,
    asset_amount: u64,
    usdc_amount: u64,
) -> Result<Option<(Sell, u64)>> {
    let t = target_margin_bps as u128;
    let plan = if asset_amount == 0 {
        (
            Sell::Usdc,
            mul_div(usdc_amount as u128, BPS_SCALE, BPS_SCALE + t, false)?,
        )
    } else if usdc_amount == 0 {
        (
            Sell::Asset,
            mul_div(asset_amount as u128, t, BPS_SCALE + t, false)?,
        )
    } else {
        let usdc = usdc_amount as u128 * BPS_SCALE;
        let wanted = price.usd_value(asset_amount as u128, decimals, false)? * t;
        if usdc >= wanted {
            (Sell::Usdc, (usdc - wanted) / (BPS_SCALE + t))
        } else {
            let usd = (wanted - usdc) / (BPS_SCALE + t);
            (Sell::Asset, price.tokens_for_usd(usd, decimals, false)?)
        }
    };
    Ok(match plan {
        (_, 0) => None,
        (sell, amount) => Some((sell, to_u64(amount)?)),
    })
}

/// Checks a swap: it spent between `planned` less the tolerance and `planned`, and received at
/// least the mark-price value of what it spent, less `slippage_bps`.
pub fn check_fill(
    sell: Sell,
    planned: u64,
    spent: u64,
    received: u64,
    price: &Price,
    decimals: u8,
    slippage_bps: u16,
) -> Result<()> {
    let floor = planned as u128 - planned as u128 * SWAP_TOLERANCE_BPS / BPS_SCALE;
    require!(
        spent as u128 >= floor && spent <= planned,
        VaultError::SwapAmountOff
    );
    let fair = match sell {
        Sell::Usdc => price.tokens_for_usd(spent as u128, decimals, false)?,
        Sell::Asset => price.usd_value(spent as u128, decimals, false)?,
    };
    require!(
        received as u128 * BPS_SCALE >= fair * (BPS_SCALE - slippage_bps as u128),
        VaultError::SwapPriceTooLow
    );
    Ok(())
}

/// Runs the venue instruction with the vault authority as its only signer.
pub fn invoke_route<'info>(
    venue: &AccountInfo<'info>,
    accounts: &[AccountInfo<'info>],
    authority: &Pubkey,
    data: &[u8],
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let metas = accounts
        .iter()
        .map(|a| AccountMeta {
            pubkey: *a.key,
            is_signer: a.is_signer || a.key == authority,
            is_writable: a.is_writable,
        })
        .collect();
    let ix = Instruction {
        program_id: *venue.key,
        accounts: metas,
        data: data.to_vec(),
    };
    // Jupiter's routes list Jupiter among their accounts; copy the list only if not.
    if accounts.iter().any(|a| a.key == venue.key) {
        invoke_signed(&ix, accounts, signer_seeds)?;
    } else {
        let mut infos = accounts.to_vec();
        infos.push(venue.clone());
        invoke_signed(&ix, &infos, signer_seeds)?;
    }
    Ok(())
}

/// A token account the route left owned by `owner`, with no delegate and no close authority.
pub fn require_plain_token_account(info: &AccountInfo, owner: &Pubkey) -> Result<()> {
    let data = info.try_borrow_data()?;
    require!(data.len() >= 165, VaultError::InvalidTokenAccount);
    // `owner` (offset 32), and the COption tags of `delegate` (72) and `close_authority` (129).
    require!(
        data[32..64] == owner.to_bytes() && data[72..76] == [0; 4] && data[129..133] == [0; 4],
        VaultError::SwapMovedVaultFunds
    );
    Ok(())
}

/// The mint authority of the share mint (a classic SPL mint).
pub fn share_mint_authority(mint: &AccountInfo) -> Result<Option<Pubkey>> {
    let data = mint.try_borrow_data()?;
    require!(data.len() >= 36, VaultError::InvalidMint);
    Ok((data[0..4] == [1, 0, 0, 0])
        .then(|| Pubkey::new_from_array(data[4..36].try_into().unwrap())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ETH: Price = Price {
        value: 3_500_000_000,
        expo: -6,
    };

    #[test]
    fn usdc_alone_buys_the_asset_with_what_is_not_margin() {
        // $35,000 at 1x: $17,500 buys ETH, $17,500 stays as margin.
        let plan = plan_deposit_swap(&ETH, 8, 10_000, 0, 35_000_000_000).unwrap();
        assert_eq!(plan, Some((Sell::Usdc, 17_500_000_000)));
        // The SOL vault needs no USDC margin: all of it buys SOL.
        let plan = plan_deposit_swap(&ETH, 9, 0, 0, 1_500_000_000).unwrap();
        assert_eq!(plan, Some((Sell::Usdc, 1_500_000_000)));
    }

    #[test]
    fn the_asset_alone_sells_what_becomes_margin() {
        // 10 ETH at 1x: 5 ETH stay, 5 ETH become USDC margin.
        let plan = plan_deposit_swap(&ETH, 8, 10_000, 1_000_000_000, 0).unwrap();
        assert_eq!(plan, Some((Sell::Asset, 500_000_000)));
        // SOL into the SOL vault: nothing to swap.
        assert_eq!(
            plan_deposit_swap(&ETH, 9, 0, 10_000_000_000, 0).unwrap(),
            None
        );
    }

    #[test]
    fn a_mixed_deposit_swaps_only_what_is_over_the_mix() {
        assert_eq!(
            plan_deposit_swap(&ETH, 8, 10_000, 500_000_000, 17_500_000_000).unwrap(),
            None
        );
        // 5 ETH + $27,500: $10,000 too much USDC, half of it buys ETH.
        let plan = plan_deposit_swap(&ETH, 8, 10_000, 500_000_000, 27_500_000_000).unwrap();
        assert_eq!(plan, Some((Sell::Usdc, 5_000_000_000)));
    }

    #[test]
    fn fills_must_match_the_plan_and_the_mark() {
        // $17,500 for 5 ETH at $3,500, with 1% allowed.
        assert!(check_fill(
            Sell::Usdc,
            17_500_000_000,
            17_500_000_000,
            495_000_000,
            &ETH,
            8,
            100
        )
        .is_ok());
        assert!(check_fill(
            Sell::Usdc,
            17_500_000_000,
            17_500_000_000,
            494_999_999,
            &ETH,
            8,
            100
        )
        .is_err());
        assert!(check_fill(
            Sell::Usdc,
            17_500_000_000,
            17_324_999_999,
            495_000_000,
            &ETH,
            8,
            100
        )
        .is_err());
        assert!(check_fill(
            Sell::Usdc,
            17_500_000_000,
            17_500_000_001,
            500_000_000,
            &ETH,
            8,
            100
        )
        .is_err());
        assert!(check_fill(
            Sell::Asset,
            500_000_000,
            500_000_000,
            17_325_000_000,
            &ETH,
            8,
            100
        )
        .is_ok());
    }
}
