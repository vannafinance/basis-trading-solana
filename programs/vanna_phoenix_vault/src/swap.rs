//! Swaps through Jupiter, so users deposit and withdraw USDC only.
//!
//! A deposit sells USDC for the asset: the part that is not margin for a spot vault, all of it
//! for the SOL vault. A withdrawal sells the leaver's part of the asset for USDC. The vault
//! decides how much to sell; the client builds a Jupiter route for about that amount with the
//! vault authority as the trader and passes the route's accounts after Phoenix's index
//! accounts. The vault resizes the route to the exact amount (`resize_route`), signs it, then
//! checks what actually moved: the side it sold went down by exactly that amount, the side it
//! bought went up at no worse than `slippage_bps` from Phoenix's mark price, and nothing else
//! it owns changed.

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

/// What a swap sells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sell {
    Usdc,
    Asset,
}

/// How much of a USDC deposit buys the asset: all but the margin the vault keeps,
/// `1 / (1 + t)` of it with `t` = `target_margin_bps`. Half at 1x; all of it for the SOL vault
/// (`t` = 0), whose SOL is its own margin. A fixed fraction, so the client can build the route
/// for the exact amount.
pub fn deposit_swap_amount(usdc_amount: u64, target_margin_bps: u16) -> Result<u64> {
    to_u64(mul_div(
        usdc_amount as u128,
        BPS_SCALE,
        BPS_SCALE + target_margin_bps as u128,
        false,
    )?)
}

/// Jupiter v6's `route` and `shared_accounts_route`: the routes the vault runs. Both end with
/// `in_amount: u64, quoted_out_amount: u64, slippage_bps: u16, platform_fee_bps: u8`.
const JUPITER_ROUTE: [u8; 8] = [229, 23, 203, 151, 122, 227, 173, 42];
const JUPITER_SHARED_ACCOUNTS_ROUTE: [u8; 8] = [193, 32, 155, 51, 65, 214, 156, 129];
const ROUTE_TAIL: usize = 19;

/// The route's data resized to sell exactly `amount`: its input amount set to `amount` and its
/// quoted output scaled with it, as Jupiter quotes the same path for a new amount. A
/// withdrawal's amount depends on its own trade, so the client can only build the route for
/// about it; resized, nothing is left unsold. Price impact the new size adds is still held to
/// `slippage_bps` from the mark by `check_fill`.
pub fn resize_route(data: &[u8], amount: u64) -> Result<Vec<u8>> {
    require!(
        data.len() >= 8 + ROUTE_TAIL
            && (data[..8] == JUPITER_ROUTE || data[..8] == JUPITER_SHARED_ACCOUNTS_ROUTE),
        VaultError::UnsupportedRoute
    );
    let at = data.len() - ROUTE_TAIL;
    let field = |i: usize| u64::from_le_bytes(data[i..i + 8].try_into().unwrap());
    let (in_amount, quoted) = (field(at), field(at + 8));
    require!(in_amount > 0, VaultError::UnsupportedRoute);
    let quoted = to_u64(mul_div(
        quoted as u128,
        amount as u128,
        in_amount as u128,
        false,
    )?)?;
    let mut data = data.to_vec();
    data[at..at + 8].copy_from_slice(&amount.to_le_bytes());
    data[at + 8..at + 16].copy_from_slice(&quoted.to_le_bytes());
    Ok(data)
}

/// Checks a swap: it spent exactly `planned` (the route was resized to it), and received at
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
    require!(spent == planned, VaultError::SwapAmountOff);
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
    fn usdc_buys_the_asset_with_what_is_not_margin() {
        // $35,000 at 1x: $17,500 buys ETH, $17,500 stays as margin.
        assert_eq!(
            deposit_swap_amount(35_000_000_000, 10_000).unwrap(),
            17_500_000_000
        );
        // The SOL vault needs no USDC margin: all of it buys SOL.
        assert_eq!(
            deposit_swap_amount(1_500_000_000, 0).unwrap(),
            1_500_000_000
        );
    }

    fn shared_route(in_amount: u64, quoted: u64) -> Vec<u8> {
        let mut data = JUPITER_SHARED_ACCOUNTS_ROUTE.to_vec();
        data.extend_from_slice(&[3, 1, 2, 3]);
        data.extend_from_slice(&in_amount.to_le_bytes());
        data.extend_from_slice(&quoted.to_le_bytes());
        data.extend_from_slice(&50u16.to_le_bytes());
        data.push(0);
        data
    }

    #[test]
    fn a_route_is_resized_to_the_exact_amount() {
        // Built for 4.98 SOL ($600), the vault sells 5 SOL: Jupiter's quote grows with it.
        let data = resize_route(&shared_route(4_980_000_000, 600_000_000), 5_000_000_000).unwrap();
        assert_eq!(data, shared_route(5_000_000_000, 602_409_638));
        // Only Jupiter's own route instructions.
        let mut other = shared_route(1, 1);
        other[0] ^= 1;
        assert!(resize_route(&other, 1).is_err());
        assert!(resize_route(&shared_route(0, 1), 1).is_err());
        assert!(resize_route(&JUPITER_ROUTE, 1).is_err());
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
            17_499_999_999,
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
