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
use anchor_spl::token_interface::TokenAccount as AnyTokenAccount;

#[derive(Accounts)]
pub struct Withdraw<'info> {
    /// Receives the SOL vault's payout as lamports.
    #[account(mut)]
    pub user: Signer<'info>,
    pub core: VaultCore<'info>,
    #[account(mut, associated_token::mint = core.share_mint, associated_token::authority = user)]
    pub user_shares: Box<Account<'info, TokenAccount>>,
    /// Receives a spot vault's asset.
    #[account(mut, token::mint = core.basis_vault.asset_mint, token::authority = user)]
    pub user_asset: Option<Box<InterfaceAccount<'info, AnyTokenAccount>>>,
    /// Receives the USDC part of the payout.
    #[account(mut, token::mint = core.usdc_mint, token::authority = user)]
    pub user_usdc: Option<Box<Account<'info, TokenAccount>>>,
    /// CHECK: the whitelisted swap venue, for single-asset payouts.
    #[account(address = core.config.swap_program @ VaultError::InvalidSwapAccounts)]
    pub swap_program: Option<UncheckedAccount<'info>>,
    /// CHECK: the vault authority's wrapped-SOL account, opened and closed around a SOL swap.
    #[account(mut, address = get_associated_token_address(&core.authority.key(), &NATIVE_MINT))]
    pub vault_wsol: Option<UncheckedAccount<'info>>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Burns `shares` and pays out their part of the vault after the leaver's own trade: the
/// short shrinks with the asset that leaves, and the payout is the NAV after that trade less
/// the part that stays with the other holders.
///
/// A spot vault pays the leaver's part of its asset tokens and the rest in USDC. The SOL
/// vault pays in SOL first: its depositors brought SOL, and its USDC is Phoenix margin that
/// only leaves once the short is gone. When Phoenix lets only part of the payout leave (SOL
/// that must stay to cover USDC the vault owes, or USDC still held as margin), the leaver
/// gets that part and burns shares for it alone; they keep the rest.
///
/// `receive` picks a single asset instead: the other part is swapped through `route` and the
/// leaver carries the swap's cost. `min_asset_out` and `min_usdc_out` apply to what is paid.
///
/// Withdrawals have no caps and stay open after hours: holders can always leave. A withdrawal
/// only ever closes short, its share of it (`VaultCore::close_part`), and does not depend on
/// Phoenix's market map.
#[allow(clippy::too_many_arguments)]
pub fn withdraw<'info>(
    ctx: Context<'info, Withdraw<'info>>,
    gti: u8,
    atb: u8,
    shares: u64,
    min_asset_out: u64,
    min_usdc_out: u64,
    receive: Receive,
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
    // A single-asset payout swaps the other part through the route.
    let sell = match receive {
        Receive::Usdc if asset_out > 0 => Some((Sell::Asset, asset_out)),
        Receive::Asset if usdc_out > 0 => Some((Sell::Usdc, usdc_out)),
        _ => None,
    };
    match (&sell, &route) {
        (Some(_), None) => return err!(VaultError::InvalidSwapAccounts),
        (None, Some(_)) => return err!(VaultError::SwapNotNeeded),
        _ => {}
    }

    burn(&share_mint, &user_shares, &user, burned, &[])?;
    if usdc_out > 0 {
        trader.withdraw_usdc(&core.ember(), usdc_out, signer)?;
    }
    let (mut asset_paid, mut usdc_paid) = (asset_out, usdc_out);
    let swapped = |(spent, received): (u64, u64), sell: Sell| match sell {
        Sell::Asset => (asset_out - spent, usdc_out + received),
        Sell::Usdc => (asset_out + received, usdc_out - spent),
    };
    match &core.vault_asset {
        Some(vault_asset) => {
            if let (Some((sell, planned)), Some(route)) = (sell, &route) {
                let accounts = route_accounts(ctx.remaining_accounts, gti, atb, route)?;
                let fill = core.swap(
                    a.swap_program.as_ref(),
                    accounts,
                    route,
                    sell,
                    planned,
                    &price,
                    vault_asset,
                    signer,
                )?;
                (asset_paid, usdc_paid) = swapped(fill, sell);
            }
            if asset_paid > 0 {
                let receiver = a
                    .user_asset
                    .as_ref()
                    .ok_or(VaultError::InvalidTokenAccount)?;
                core.transfer_asset(
                    vault_asset,
                    receiver.as_ref().as_ref(),
                    &authority,
                    asset_paid,
                    signer,
                )?;
            }
        }
        None => match (sell, &route) {
            (Some((sell, planned)), Some(route)) => {
                let accounts = route_accounts(ctx.remaining_accounts, gti, atb, route)?;
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
                if sell == Sell::Asset {
                    // The SOL leaves Phoenix to the authority and is wrapped to be sold.
                    trader.withdraw_native_sol(&authority, asset_out, signer)?;
                    system_transfer(&authority, wsol, asset_out, signer)?;
                    sync_native(wsol)?;
                } else {
                    trader.withdraw_native_sol(&user, asset_out, signer)?;
                }
                let fill = core.swap(
                    a.swap_program.as_ref(),
                    accounts,
                    route,
                    sell,
                    planned,
                    &price,
                    wsol,
                    signer,
                )?;
                (asset_paid, usdc_paid) = swapped(fill, sell);
                // Closing the wrapped-SOL account pays the user its SOL and their rent back.
                close_account(wsol, &user, &authority, signer)?;
            }
            _ => trader.withdraw_native_sol(&user, asset_out, signer)?,
        },
    }
    if usdc_paid > 0 {
        let receiver = a
            .user_usdc
            .as_ref()
            .ok_or(VaultError::InvalidTokenAccount)?;
        transfer(
            &core.vault_usdc,
            receiver.as_ref().as_ref(),
            &authority,
            usdc_paid,
            signer,
        )?;
    }
    require!(
        asset_paid >= min_asset_out && usdc_paid >= min_usdc_out,
        VaultError::PayoutTooLow
    );

    emit!(Withdrawn {
        basis_vault: vault_key,
        user: a.user.key(),
        shares: burned,
        payout_usd: to_u64(paid)?,
        asset_out: asset_paid,
        usdc_out: usdc_paid,
        position_lots: after.position_lots,
    });
    Ok(())
}
