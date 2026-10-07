//! Reinvesting funding and keeping margin.
//!
//! Phoenix pays funding in USDC, into the vault's Phoenix collateral. The NAV counts it at
//! once, but it only compounds once it is turned into more of the asset with a bigger short
//! on top. The keeper does that swap with the vault at Phoenix's mark price: it hands the
//! vault the asset and takes the same value in USDC out of Phoenix, and the vault grows its
//! short to match. The reverse swap tops up the USDC margin of a spot vault after the asset
//! rose (its short lost USDC while its tokens gained), or pays down the USDC the SOL vault
//! owes after realized losses.
//!
//! Phoenix never lets native SOL back a USDC withdrawal, so USDC leaves the SOL vault only
//! from collateral above its short's initial margin. The keeper keeps such a USDC buffer in
//! the SOL vault (with the reverse swap) before it reinvests funding there.
//!
//! The keeper is the vault's counterparty here, so its key is limited: it trades within its own
//! band (`keeper_band_bps`, tighter than users' `slippage_bps`) and at most `max_rebalance_usd`
//! per day. A stolen keeper key can cost the vault at most the band on that volume a day. With
//! no asset and no USDC, `rebalance` only re-hedges: after a donation, or after Phoenix
//! liquidated or deleveraged part of the short, which withdrawals leave to the keeper.

use crate::constants::*;
use crate::errors::VaultError;
use crate::events::Rebalanced;
use crate::instructions::common::*;
use crate::instructions::vault_core::*;
use crate::math::*;
use crate::nav::*;
use anchor_lang::prelude::*;
use anchor_spl::token::{Token, TokenAccount};
use anchor_spl::token_interface::TokenAccount as AnyTokenAccount;

#[derive(Accounts)]
pub struct Rebalance<'info> {
    #[account(mut)]
    pub keeper: Signer<'info>,
    pub core: VaultCore<'info>,
    /// The keeper's asset tokens (spot vaults); the SOL vault swaps lamports with `keeper`.
    #[account(mut, token::mint = core.basis_vault.asset_mint, token::authority = keeper)]
    pub keeper_asset: Option<Box<InterfaceAccount<'info, AnyTokenAccount>>>,
    #[account(mut, token::mint = core.usdc_mint, token::authority = keeper)]
    pub keeper_usdc: Box<Account<'info, TokenAccount>>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

/// Swaps `asset_amount` of the asset for `usdc_amount` USDC with the keeper, within
/// `keeper_band_bps` of the mark price, then trades the short to match the asset held.
/// `buy_asset` reinvests USDC into the asset; it may not take the vault's margin below
/// `margin_bps` or the vault past its caps. Both amounts zero: re-hedge only.
pub fn rebalance<'info>(
    ctx: Context<'info, Rebalance<'info>>,
    gti: u8,
    atb: u8,
    buy_asset: bool,
    asset_amount: u64,
    usdc_amount: u64,
) -> Result<()> {
    let a = &ctx.accounts;
    let core = &a.core;
    let vault = &core.basis_vault;
    core.config.require_live()?;
    core.config.require_keeper(a.keeper.key)?;
    let swaps = asset_amount > 0 || usdc_amount > 0;
    require!(
        !swaps || (asset_amount > 0 && usdc_amount > 0),
        VaultError::ZeroAmount
    );

    let vault_key = vault.key();
    let seeds = vault.authority_seeds(&vault_key);
    let signer: &[&[&[u8]]] = &[&seeds];
    let token_program = a.token_program.to_account_info();
    let system_program = a.system_program.to_account_info();
    let keeper = a.keeper.to_account_info();
    let authority = core.authority.to_account_info();
    let keeper_usdc = a.keeper_usdc.to_account_info();
    let trader = core.trader(
        ctx.remaining_accounts,
        gti,
        atb,
        &token_program,
        &system_program,
        &[&keeper],
    )?;
    let market = core.market()?;

    let before = core.snapshot(&trader, &market, true)?;
    let nav_before = before.nav(&market)?;
    let notional_before = before.notional(&market)?;

    let fair = before
        .price
        .usd_value(asset_amount as u128, market.decimals, false)?;
    // The day's volume, this trade included.
    let now = Clock::get()?.unix_timestamp;
    let (window_start, window_usd) = if now
        >= vault
            .rebalance_window_start
            .saturating_add(REBALANCE_WINDOW_SECONDS)
    {
        (now, fair)
    } else {
        (
            vault.rebalance_window_start,
            vault.rebalance_window_usd as u128 + fair,
        )
    };
    require!(
        window_usd <= vault.max_rebalance_usd as u128,
        VaultError::RebalanceLimitReached
    );
    let band = vault.keeper_band_bps as u128;
    let usdc = usdc_amount as u128 * BPS_SCALE;
    if !swaps {
        // Re-hedge only.
    } else if buy_asset {
        require!(
            usdc <= fair * (BPS_SCALE + band),
            VaultError::PriceOutOfBand
        );
        match &core.vault_asset {
            None => trader.deposit_native_sol(&keeper, &keeper, asset_amount, &[])?,
            Some(vault_asset) => {
                let source = a
                    .keeper_asset
                    .as_ref()
                    .ok_or(VaultError::InvalidTokenAccount)?;
                core.transfer_asset(
                    source.as_ref().as_ref(),
                    vault_asset,
                    &keeper,
                    asset_amount,
                    &[],
                )?;
            }
        }
        trader.withdraw_usdc(&core.ember(), usdc_amount, signer)?;
        transfer(
            &core.vault_usdc,
            &keeper_usdc,
            &authority,
            usdc_amount,
            signer,
        )?;
    } else {
        require!(
            usdc >= fair * (BPS_SCALE - band),
            VaultError::PriceOutOfBand
        );
        transfer(&keeper_usdc, &core.vault_usdc, &keeper, usdc_amount, &[])?;
        trader.deposit_usdc(&core.ember(), usdc_amount, signer)?;
        match &core.vault_asset {
            None => trader.withdraw_native_sol(&keeper, asset_amount, signer)?,
            Some(vault_asset) => {
                let receiver = a
                    .keeper_asset
                    .as_ref()
                    .ok_or(VaultError::InvalidTokenAccount)?;
                core.transfer_asset(
                    vault_asset,
                    receiver.as_ref().as_ref(),
                    &authority,
                    asset_amount,
                    signer,
                )?;
            }
        }
    }

    core.hedge(&trader, &market, &before, core.asset_units()?, signer)?;
    let after = core.snapshot(&trader, &market, true)?;
    let nav_after = after.nav(&market)?;
    if swaps && buy_asset {
        let notional_after = after.notional(&market)?;
        core.check_caps(&market, &before, &after)?;
        require!(
            margin_ok(
                (before.margin.effective, notional_before),
                (after.margin.effective, notional_after),
                vault.margin_bps,
                false,
            ),
            VaultError::MarginTooThin
        );
    }

    emit!(Rebalanced {
        basis_vault: vault_key,
        buy_asset,
        asset_amount,
        usdc_amount,
        nav_before_usd: to_u64(nav_before)?,
        nav_after_usd: to_u64(nav_after)?,
        position_lots: after.position_lots,
    });
    if swaps {
        let vault = &mut ctx.accounts.core.basis_vault;
        vault.rebalance_window_start = window_start;
        vault.rebalance_window_usd = to_u64(window_usd)?;
    }
    Ok(())
}
