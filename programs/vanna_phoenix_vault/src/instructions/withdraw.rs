use crate::constants::*;
use crate::errors::VaultError;
use crate::events::Withdrawn;
use crate::instructions::common::*;
use crate::instructions::vault_core::*;
use crate::math::*;
use crate::nav::*;
use crate::swap::*;
use anchor_lang::prelude::*;
use anchor_spl::associated_token::{get_associated_token_address, AssociatedToken};
use anchor_spl::token::{Token, TokenAccount};

#[derive(Accounts)]
pub struct Withdraw<'info> {
    /// Gets back the rent of the wrapped-SOL account the SOL vault sells through.
    #[account(mut)]
    pub user: Signer<'info>,
    pub core: VaultCore<'info>,
    #[account(mut, associated_token::mint = core.share_mint, associated_token::authority = user)]
    pub user_shares: Box<Account<'info, TokenAccount>>,
    /// Receives the payout: withdrawals are USDC only.
    #[account(mut, token::mint = core.usdc_mint, token::authority = user)]
    pub user_usdc: Box<Account<'info, TokenAccount>>,
    /// CHECK: the whitelisted swap venue (Jupiter), which sells the leaver's part of the asset.
    #[account(address = core.config.swap_program @ VaultError::InvalidSwapAccounts)]
    pub swap_program: Option<UncheckedAccount<'info>>,
    /// CHECK: the vault authority's wrapped-SOL account, opened and closed around the SOL
    /// vault's swap.
    #[account(mut, address = get_associated_token_address(&core.authority.key(), &NATIVE_MINT))]
    pub vault_wsol: Option<UncheckedAccount<'info>>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Burns `shares` and pays out their part of the vault in USDC, after the leaver's own trade:
/// the short shrinks with the asset that leaves, and the payout is the NAV after that trade
/// less the part that stays with the other holders.
///
/// The leaver's part of the asset is sold for USDC through `route` (resized to the exact
/// amount, `swap::resize_route`); the rest of the payout is USDC from Phoenix. The leaver
/// carries the swap's cost, and `min_usdc_out` applies to the USDC they receive. A spot vault
/// sells the leaver's part of its tokens. The SOL vault takes SOL out first: its USDC is
/// Phoenix margin that only leaves once the short is gone. When Phoenix lets only part of the
/// payout leave (SOL that must stay to cover USDC the vault owes, or USDC still held as
/// margin), the leaver gets that part and burns shares for it alone; they keep the rest.
///
/// Withdrawals have no caps and stay open after hours: holders can always leave while Jupiter
/// can sell their asset. A withdrawal only ever closes short, its share of it
/// (`VaultCore::close_part`), and does not depend on Phoenix's market map.
pub fn withdraw<'info>(
    ctx: Context<'info, Withdraw<'info>>,
    gti: u8,
    atb: u8,
    shares: u64,
    min_usdc_out: u64,
    route: Option<SwapRoute>,
) -> Result<()> {
    let a = &ctx.accounts;
    let core = &a.core;
    let vault = &core.basis_vault;
    require!(shares > 0, VaultError::ZeroAmount);
    require!(shares <= a.user_shares.amount, VaultError::NotEnoughShares);

    let vault_key = vault.key();
    let seeds = vault.authority_seeds(&vault_key);
    let signer: &[&[&[u8]]] = &[&seeds];
    let token_program = a.token_program.to_account_info();
    let system_program = a.system_program.to_account_info();
    let user = a.user.to_account_info();
    let share_mint = core.share_mint.to_account_info();
    let user_shares = a.user_shares.to_account_info();
    let authority = core.authority.to_account_info();

    let trader = core.trader(
        ctx.remaining_accounts,
        gti,
        atb,
        &token_program,
        &system_program,
        &[&user],
    )?;
    let market = core.market()?;
    let supply = core.share_supply()?;
    let before = core.snapshot(&trader, &market, false)?;
    let nav_before = before.nav(&market)?;

    // The asset that leaves, estimated before the trade so the short can shrink with it.
    let part_usd = mul_div(nav_before, shares as u128, supply as u128, false)?;
    let part_units = to_u64(
        before
            .price
            .tokens_for_usd(part_usd, market.decimals, false)?,
    )?;
    let units = before.asset_units;
    let leaving = match &core.vault_asset {
        None => part_units.min(before.native_withdrawable(&market, true)?),
        Some(_) => to_u64(mul_div(
            units as u128,
            shares as u128,
            supply as u128,
            false,
        )?)?
        .min(part_units),
    };
    core.close_part(&trader, &before, leaving, signer)?;

    let after = core.snapshot(&trader, &market, false)?;
    let price = after.price;
    let nav_after = after.nav(&market)?;
    let payout = withdrawal_payout(nav_before, nav_after, shares, supply)?;
    // What the leaver's own trade took out of the vault: theirs alone, even when Phoenix lets
    // only part of the payout leave now.
    let cost = nav_before.saturating_sub(nav_after);
    let mut asset_out = leaving.min(to_u64(price.tokens_for_usd(
        payout,
        market.decimals,
        false,
    )?)?);
    if core.vault_asset.is_none() {
        // The trade's own costs came out of the margin too: check against what is left.
        asset_out = asset_out.min(after.native_withdrawable(&market, false)?);
    }
    let asset_usd = price.usd_value(asset_out as u128, market.decimals, true)?;
    let usdc_out = to_u64(payout.saturating_sub(asset_usd))?.min(after.margin.withdrawable);
    let paid = price.usd_value(asset_out as u128, market.decimals, false)? + usdc_out as u128;
    let burned = shares_burned(shares, paid, payout, cost)?;
    require!(burned > 0, VaultError::NothingToWithdraw);
    // The leaver's part of the asset is sold; the route is needed exactly then.
    let route = match (asset_out > 0, route) {
        (true, None) => return err!(VaultError::InvalidSwapAccounts),
        (false, Some(_)) => return err!(VaultError::SwapNotNeeded),
        (_, route) => route,
    };

    burn(&share_mint, &user_shares, &user, burned, &[])?;
    trader.withdraw_usdc(&core.ember(), usdc_out, signer)?;
    let mut received = 0;
    if let Some(route) = &route {
        let accounts = route_accounts(ctx.remaining_accounts, gti, atb, route)?;
        let venue = a.swap_program.as_ref();
        received = match &core.vault_asset {
            Some(vault_asset) => {
                core.swap(
                    venue,
                    accounts,
                    route,
                    Sell::Asset,
                    asset_out,
                    &price,
                    vault_asset,
                    signer,
                )?
                .1
            }
            None => {
                // The SOL leaves Phoenix to the authority and is wrapped to be sold.
                let wsol = wsol_account(&a.vault_wsol)?;
                create_ata_idempotent(
                    &user,
                    wsol,
                    &authority,
                    &core.asset_mint,
                    &system_program,
                    &token_program,
                    &[],
                )?;
                trader.withdraw_native_sol(&authority, asset_out, signer)?;
                system_transfer(&authority, wsol, asset_out, signer)?;
                sync_native(wsol)?;
                let (_, received) = core.swap(
                    venue,
                    accounts,
                    route,
                    Sell::Asset,
                    asset_out,
                    &price,
                    wsol,
                    signer,
                )?;
                // All of it was sold: closing the account pays the user their rent back.
                close_account(wsol, &user, &authority, signer)?;
                received
            }
        };
    }
    let usdc_paid = usdc_out + received;
    transfer(
        &core.vault_usdc,
        a.user_usdc.as_ref().as_ref(),
        &authority,
        usdc_paid,
        signer,
    )?;
    require!(usdc_paid >= min_usdc_out, VaultError::PayoutTooLow);

    emit!(Withdrawn {
        basis_vault: vault_key,
        user: a.user.key(),
        shares: burned,
        payout_usd: to_u64(paid)?,
        asset_sold: asset_out,
        usdc_out: usdc_paid,
        position_lots: after.position_lots,
    });
    Ok(())
}
