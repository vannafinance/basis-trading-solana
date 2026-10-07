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
use anchor_spl::token_interface::TokenAccount as AnyTokenAccount;

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
    /// The user's asset tokens (spot vaults). The SOL vault takes lamports from `user`.
    #[account(mut, token::mint = core.basis_vault.asset_mint, token::authority = user)]
    pub user_asset: Option<Box<InterfaceAccount<'info, AnyTokenAccount>>>,
    /// The user's USDC, when the deposit brings margin.
    #[account(mut, token::mint = core.usdc_mint, token::authority = user)]
    pub user_usdc: Option<Box<Account<'info, TokenAccount>>>,
    /// CHECK: the whitelisted swap venue, for single-asset deposits.
    #[account(address = core.config.swap_program @ VaultError::InvalidSwapAccounts)]
    pub swap_program: Option<UncheckedAccount<'info>>,
    /// CHECK: the vault authority's wrapped-SOL account, opened and closed around a SOL swap.
    #[account(mut, address = get_associated_token_address(&core.authority.key(), &NATIVE_MINT))]
    pub vault_wsol: Option<UncheckedAccount<'info>>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Deposits `asset_amount` of the asset and `usdc_amount` of USDC, trades the short to match
/// the asset the vault then holds and mints shares for the value the deposit added after its
/// own swap and trade.
///
/// A user can bring a single asset. With a `route`, the vault swaps the deposit to its target
/// mix first (`swap::plan_deposit_swap`):
/// - ETH or BTC vault, USDC alone: half buys the asset, half stays as the short's margin (1x).
/// - ETH or BTC vault, the asset alone: half stays, half is sold for margin.
/// - SOL vault, USDC alone: all of it buys SOL, which is Phoenix collateral itself.
/// - SOL vault, SOL alone: nothing to swap; 10 SOL in becomes a 10 SOL short.
///
/// Without a route the deposit must already be at the vault's mix (`require_on_mix`) and still
/// leave the vault at least `margin_bps` of margin (or no thinner than before).
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
    asset_amount: u64,
    usdc_amount: u64,
    min_shares: u64,
    route: Option<SwapRoute>,
) -> Result<()> {
    let a = &ctx.accounts;
    let core = &a.core;
    let vault = &core.basis_vault;
    core.config.require_live()?;
    require!(vault.deposits_open, VaultError::DepositsClosed);
    require!(asset_amount > 0 || usdc_amount > 0, VaultError::ZeroAmount);

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
    if route.is_none() {
        require_on_mix(
            &before.price,
            &market,
            vault.target_margin_bps,
            asset_amount,
            usdc_amount,
        )?;
    }

    // The user's funds come in. SOL goes straight into Phoenix as collateral.
    if asset_amount > 0 {
        match &core.vault_asset {
            None => trader.deposit_native_sol(&user, &user, asset_amount, &[])?,
            Some(vault_asset) => {
                let source = a
                    .user_asset
                    .as_ref()
                    .ok_or(VaultError::InvalidTokenAccount)?;
                core.transfer_asset(
                    source.as_ref().as_ref(),
                    vault_asset,
                    &user,
                    asset_amount,
                    &[],
                )?;
            }
        }
    }
    if usdc_amount > 0 {
        let source = a
            .user_usdc
            .as_ref()
            .ok_or(VaultError::InvalidTokenAccount)?;
        transfer(
            source.as_ref().as_ref(),
            &core.vault_usdc,
            &user,
            usdc_amount,
            &[],
        )?;
    }

    // A single-asset deposit swaps to the vault's target mix.
    let mut margin_in = usdc_amount;
    if let Some(route) = &route {
        let (sell, planned) = plan_deposit_swap(
            &before.price,
            market.decimals,
            vault.target_margin_bps,
            asset_amount,
            usdc_amount,
        )?
        .ok_or(VaultError::SwapNotNeeded)?;
        let accounts = route_accounts(ctx.remaining_accounts, gti, atb, route)?;
        let venue = a.swap_program.as_ref();
        match &core.vault_asset {
            Some(vault_asset) => {
                let (spent, received) = core.swap(
                    venue,
                    accounts,
                    route,
                    sell,
                    planned,
                    &before.price,
                    vault_asset,
                    signer,
                )?;
                margin_in = match sell {
                    Sell::Usdc => margin_in - spent,
                    Sell::Asset => margin_in + received,
                };
            }
            None => {
                // The SOL vault buys wrapped SOL, unwraps it and posts it to Phoenix.
                require!(sell == Sell::Usdc, VaultError::SwapNotNeeded);
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
                let (spent, received) = core.swap(
                    venue,
                    accounts,
                    route,
                    sell,
                    planned,
                    &before.price,
                    wsol,
                    signer,
                )?;
                close_account(wsol, &authority, &authority, signer)?;
                trader.deposit_native_sol(&user, &authority, received, signer)?;
                // The wrapped-SOL account's rent goes back to the user, who paid it.
                let rent = authority.lamports() - lamports_before;
                system_transfer(&authority, &user, rent, signer)?;
                margin_in -= spent;
            }
        }
    }

    // The USDC left becomes the short's margin on Phoenix.
    trader.deposit_usdc(&core.ember(), margin_in, signer)?;

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
        asset_amount,
        usdc_amount,
        shares,
        value_usd: to_u64(value)?,
        nav_before_usd: to_u64(nav_before)?,
        position_lots: after.position_lots,
    });
    Ok(())
}

/// A deposit without a swap must already be at the vault's mix (`target_margin_bps` of USDC
/// per dollar of asset), within `MIX_TOLERANCE_BPS` of its value. Off the mix, the vault would
/// take or give its hedged asset at Phoenix's mark price, which can sit away from the spot
/// price; a single-asset deposit swaps at spot instead.
fn require_on_mix(
    price: &Price,
    market: &Market,
    target_margin_bps: u16,
    asset_amount: u64,
    usdc_amount: u64,
) -> Result<()> {
    let Some((sell, amount)) = plan_deposit_swap(
        price,
        market.decimals,
        target_margin_bps,
        asset_amount,
        usdc_amount,
    )?
    else {
        return Ok(());
    };
    let asset_usd = price.usd_value(asset_amount as u128, market.decimals, false)?;
    let off_usd = match sell {
        Sell::Usdc => amount as u128,
        Sell::Asset => price.usd_value(amount as u128, market.decimals, true)?,
    };
    require!(
        off_usd * BPS_SCALE <= (asset_usd + usdc_amount as u128) * MIX_TOLERANCE_BPS,
        VaultError::DepositOffMix
    );
    Ok(())
}
