use crate::constants::*;
use crate::errors::VaultError;
use crate::events::Deposited;
use crate::instructions::common::*;
use crate::instructions::vault_core::*;
use crate::math::*;
use crate::nav::*;
use crate::swap::*;
use anchor_lang::prelude::*;
use anchor_spl::associated_token::{get_associated_token_address, AssociatedToken};
use anchor_spl::token::{Token, TokenAccount};

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    pub core: VaultCore<'info>,
    /// CHECK: the user's share account (their associated token account), created if missing.
    #[account(
        mut,
        address = get_associated_token_address(user.key, &core.share_mint.key()),
    )]
    pub user_shares: UncheckedAccount<'info>,
    /// The user's USDC: deposits are USDC only.
    #[account(mut, token::mint = core.usdc_mint, token::authority = user)]
    pub user_usdc: Box<Account<'info, TokenAccount>>,
    /// CHECK: the whitelisted swap venue (Jupiter), which buys the asset.
    #[account(address = core.config.swap_program @ VaultError::InvalidSwapAccounts)]
    pub swap_program: UncheckedAccount<'info>,
    /// CHECK: the vault authority's wrapped-SOL account, opened and closed around the SOL
    /// vault's swap.
    #[account(mut, address = get_associated_token_address(&core.authority.key(), &NATIVE_MINT))]
    pub vault_wsol: Option<UncheckedAccount<'info>>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Deposits `usdc_amount` of USDC, buys the asset with it through `route`, trades the short to
/// match the asset the vault then holds and mints shares for the value the deposit added after
/// its own swap and trade.
///
/// How much USDC buys the asset is the vault's target mix (`swap::deposit_swap_amount`):
/// - ETH, BTC or stock vault: half buys the asset, half stays as the short's margin (1x).
/// - SOL vault: all of it buys SOL, which is Phoenix collateral itself.
///
/// Deposits are refused while the vault owes more than it holds, and while a stock token's
/// multiplier is in the middle of a large change (`multiplier_changing`).
///
/// A deposit may add at most `max_deposit_usd` and keep the vault within its caps
/// (`VaultCore::check_caps`). A stock vault takes deposits only while its market is in session
/// unless `after_hours_deposits` is set.
pub fn deposit<'info>(
    ctx: Context<'info, Deposit<'info>>,
    gti: u8,
    atb: u8,
    usdc_amount: u64,
    min_shares: u64,
    route: SwapRoute,
) -> Result<()> {
    let a = &ctx.accounts;
    let core = &a.core;
    let vault = &core.basis_vault;
    core.config.require_live()?;
    require!(vault.deposits_open, VaultError::DepositsClosed);
    let planned = deposit_swap_amount(usdc_amount, vault.target_margin_bps)?;
    require!(planned > 0, VaultError::ZeroAmount);

    let vault_key = vault.key();
    let seeds = vault.authority_seeds(&vault_key);
    let signer: &[&[&[u8]]] = &[&seeds];
    let token_program = a.token_program.to_account_info();
    let system_program = a.system_program.to_account_info();
    let user = a.user.to_account_info();
    let trader = core.trader(
        ctx.remaining_accounts,
        gti,
        atb,
        &token_program,
        &system_program,
        &[&user],
    )?;
    let market = core.market()?;

    let before = core.snapshot(&trader, &market, true)?;
    require!(
        vault.after_hours_deposits || !before.market.after_hours,
        VaultError::MarketAfterHours
    );
    require!(
        !multiplier_changing(&core.asset_mint, Clock::get()?.unix_timestamp)?,
        VaultError::MultiplierChanging
    );
    let nav_before = before.nav(&market)?;
    let notional_before = before.notional(&market)?;
    let supply = core.share_supply()?;
    // A vault that owes more than it holds would spend the deposit on its debt first.
    let equity = before.equity(&market)?;
    require!(
        equity > 0 || (supply == 0 && equity == 0),
        VaultError::VaultInsolvent
    );

    // The user's USDC comes in and buys the asset at spot.
    transfer(
        a.user_usdc.as_ref().as_ref(),
        &core.vault_usdc,
        &user,
        usdc_amount,
        &[],
    )?;
    let accounts = route_accounts(ctx.remaining_accounts, gti, atb, &route)?;
    let venue = Some(&a.swap_program);
    let (spent, bought) = match &core.vault_asset {
        Some(vault_asset) => core.swap(
            venue,
            accounts,
            &route,
            Sell::Usdc,
            planned,
            &before.price,
            vault_asset,
            signer,
        )?,
        None => {
            // The SOL vault buys wrapped SOL, unwraps it and posts it to Phoenix.
            let wsol = wsol_account(&a.vault_wsol)?;
            let authority = core.authority.to_account_info();
            let lamports_before = authority.lamports();
            create_ata_idempotent(
                &user,
                wsol,
                &authority,
                &core.asset_mint,
                &system_program,
                &token_program,
                &[],
            )?;
            let fill = core.swap(
                venue,
                accounts,
                &route,
                Sell::Usdc,
                planned,
                &before.price,
                wsol,
                signer,
            )?;
            close_account(wsol, &authority, &authority, signer)?;
            trader.deposit_native_sol(&user, &authority, fill.1, signer)?;
            // The wrapped-SOL account's rent goes back to the user, who paid it.
            let rent = authority.lamports() - lamports_before;
            system_transfer(&authority, &user, rent, signer)?;
            fill
        }
    };

    // The USDC left becomes the short's margin on Phoenix.
    trader.deposit_usdc(&core.ember(), usdc_amount - spent, signer)?;

    core.hedge(&trader, &market, &before, core.asset_units()?, signer)?;
    let after = core.snapshot(&trader, &market, true)?;
    let nav_after = after.nav(&market)?;
    let notional_after = after.notional(&market)?;
    core.check_caps(&market, &before, &after)?;
    require!(
        margin_ok(
            (before.margin.effective, notional_before),
            (after.margin.effective, notional_after),
            vault.margin_bps,
            true,
        ),
        VaultError::MarginTooThin
    );

    let value = nav_after.saturating_sub(nav_before);
    require!(value > 0, VaultError::NothingDeposited);
    require!(
        value <= vault.max_deposit_usd as u128,
        VaultError::DepositTooLarge
    );
    let shares = to_u64(shares_for_value(value, nav_before, supply)?)?;
    require!(shares > 0 && shares >= min_shares, VaultError::SharesTooLow);

    let share_mint = core.share_mint.to_account_info();
    let user_shares = a.user_shares.to_account_info();
    let authority = core.authority.to_account_info();
    create_ata_idempotent(
        &user,
        &user_shares,
        &user,
        &share_mint,
        &system_program,
        &token_program,
        &[],
    )?;
    mint_to(&share_mint, &user_shares, &authority, shares, signer)?;

    emit!(Deposited {
        basis_vault: vault_key,
        user: a.user.key(),
        usdc_amount,
        asset_bought: bought,
        shares,
        value_usd: to_u64(value)?,
        nav_before_usd: to_u64(nav_before)?,
        position_lots: after.position_lots,
    });
    Ok(())
}
