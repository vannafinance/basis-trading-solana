//! The keeper's strategy instructions. Off-chain, the keeper watches every Phoenix market's
//! funding and open interest and decides where the vault's USDC should be; on-chain, each of
//! its moves is held to the vault's limits:
//!
//! - `deploy` buys a leg's asset with buffer USDC through Jupiter and shorts the same amount;
//!   it may not take the buffer below `buffer_bps` of NAV plus what pending requests are
//!   worth, the margin below `margin_bps`, or the leg past its caps.
//! - `unwind` closes part of a leg's short and sells that much of its asset back to USDC.
//! - `move_margin` moves USDC between the buffer and Phoenix collateral.
//! - `rehedge` brings a leg's short back to its asset after a liquidation or a donation.
//!
//! Every swap is checked against Phoenix's mark (`check_fill`) and may move nothing but its
//! two sides, every order fills within `slippage_bps` of the mark, and deploys and unwinds
//! together are limited to `max_turnover_usd` a day: the keeper can neither take value nor
//! churn the vault without bound.

use crate::constants::*;
use crate::errors::VaultError;
use crate::events::{Deployed, MarginMoved, Rehedged, Unwound};
use crate::instructions::common::*;
use crate::instructions::vault_core::*;
use crate::math::*;
use crate::nav::*;
use crate::phoenix::*;
use crate::state::*;
use crate::swap::*;
use anchor_lang::prelude::*;
use anchor_spl::associated_token::{get_associated_token_address, AssociatedToken};
use anchor_spl::token::Token;

/// The keeper's turnover window after `usd` more: (window start, volume), refused past the
/// daily limit.
fn turnover(vault: &Vault, usd: u128) -> Result<(i64, u64)> {
    let now = Clock::get()?.unix_timestamp;
    let (start, total) = if now
        >= vault
            .turnover_window_start
            .saturating_add(TURNOVER_WINDOW_SECONDS)
    {
        (now, usd)
    } else {
        (
            vault.turnover_window_start,
            vault.turnover_window_usd as u128 + usd,
        )
    };
    require!(
        total <= vault.max_turnover_usd as u128,
        VaultError::TurnoverLimitReached
    );
    Ok((start, to_u64(total)?))
}

/// Leg `index`, which must not be retired.
fn leg_at<'b, 'a, 'info>(legs: &'b [LegRef<'a, 'info>], index: u8) -> Result<&'b LegRef<'a, 'info>> {
    let leg = legs
        .get(index as usize)
        .ok_or(VaultError::InvalidLegAccounts)?;
    require!(!leg.leg.retired, VaultError::LegRetired);
    Ok(leg)
}

/// The leg's market accounts, as the leg names them.
fn market_accounts<'a, 'info>(
    leg: &LegRef<'_, 'info>,
    orderbook: &'a AccountInfo<'info>,
    spline_collection: &'a AccountInfo<'info>,
) -> Result<MarketAccounts<'a, 'info>> {
    require!(
        orderbook.key() == leg.leg.orderbook && spline_collection.key() == leg.leg.spline_collection,
        VaultError::InvalidPhoenixAccount
    );
    Ok(MarketAccounts {
        orderbook,
        spline_collection,
    })
}

#[derive(Accounts)]
pub struct Deploy<'info> {
    #[account(mut)]
    pub keeper: Signer<'info>,
    pub core: VaultCore<'info>,
    /// CHECK: the leg's order book, checked against the leg.
    #[account(mut)]
    pub orderbook: UncheckedAccount<'info>,
    /// CHECK: the leg's spline collection, checked against the leg.
    #[account(mut)]
    pub spline_collection: UncheckedAccount<'info>,
    /// CHECK: the whitelisted swap venue (Jupiter).
    #[account(address = core.config.swap_program @ VaultError::InvalidSwapAccounts)]
    pub swap_program: UncheckedAccount<'info>,
    /// CHECK: the authority's wrapped-SOL account, opened and closed around the SOL leg's swap.
    #[account(mut, address = get_associated_token_address(&core.authority.key(), &NATIVE_MINT))]
    pub vault_wsol: Option<UncheckedAccount<'info>>,
    /// CHECK: the native mint, for the SOL leg's wrapped-SOL account.
    #[account(address = NATIVE_MINT)]
    pub native_mint: Option<UncheckedAccount<'info>>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Spends `usdc_amount` of buffer USDC on leg `leg_index`'s asset through `route` and shorts
/// the asset it then holds. For the SOL leg the SOL goes into Phoenix as collateral; for a spot
/// leg the tokens stay in its asset account and its short draws on the USDC margin already on
/// Phoenix (`move_margin`). `remaining_accounts`: Phoenix's index accounts, every leg
/// (`load_legs`), then the route's accounts.
pub fn deploy<'info>(
    ctx: Context<'info, Deploy<'info>>,
    gti: u8,
    atb: u8,
    leg_index: u8,
    usdc_amount: u64,
    route: SwapRoute,
) -> Result<()> {
    let a = &ctx.accounts;
    let core = &a.core;
    let vault = &core.vault;
    core.config.require_live()?;
    core.config.require_keeper(a.keeper.key)?;
    require!(usdc_amount > 0, VaultError::ZeroAmount);

    let vault_key = vault.key();
    let start = gti as usize + atb as usize;
    let (legs, used) = load_legs(
        &vault_key,
        vault,
        ctx.remaining_accounts.get(start..).unwrap_or_default(),
    )?;
    let index = leg_index as usize;
    let leg = leg_at(&legs, leg_index)?;
    require!(leg.leg.active, VaultError::LegInactive);
    let market = market_accounts(leg, &a.orderbook, &a.spline_collection)?;
    let accounts = route_accounts(ctx.remaining_accounts, start + used, &route)?;

    let keeper = a.keeper.to_account_info();
    let token_program = a.token_program.to_account_info();
    let system_program = a.system_program.to_account_info();
    let trader = core.trader(
        ctx.remaining_accounts,
        gti,
        atb,
        &[&keeper, &a.orderbook, &a.spline_collection, &token_program, &system_program],
        None,
        None,
    )?;
    let before = core.snapshot(&trader, &legs, Pricing::Mark)?;
    let state = before.legs[index];
    require!(!core.changing(leg)?, VaultError::MultiplierChanging);
    require!(
        leg.leg.after_hours_trading
            || !market_state(&core.perp_asset_map, leg.leg.asset_id)?.after_hours,
        VaultError::MarketAfterHours
    );
    let reserved = before.reserved_usdc(vault.buffer_bps)?;
    require!(
        usdc_amount <= before.idle_usdc
            && (before.idle_usdc - usdc_amount) as u128 >= reserved,
        VaultError::BufferTooThin
    );
    let window = turnover(vault, usdc_amount as u128)?;

    let seeds = vault.authority_seeds(&vault_key);
    let signer: &[&[&[u8]]] = &[&seeds];
    let venue = a.swap_program.to_account_info();
    let (spent, bought) = match leg.holding {
        Some(holding) => core.swap(
            &venue, accounts, &route, Sell::Usdc, usdc_amount, &legs, index, &state.price,
            holding, signer,
        )?,
        None => {
            // The SOL leg buys wrapped SOL, unwraps it and posts it to Phoenix.
            let wsol = a.vault_wsol.as_ref().ok_or(VaultError::InvalidSwapAccounts)?;
            let native_mint = a.native_mint.as_ref().ok_or(VaultError::InvalidSwapAccounts)?;
            let authority = core.authority.to_account_info();
            let lamports_before = authority.lamports();
            create_ata_idempotent(
                &keeper,
                wsol,
                &authority,
                native_mint,
                &system_program,
                &token_program,
                &[],
            )?;
            let fill = core.swap(
                &venue, accounts, &route, Sell::Usdc, usdc_amount, &legs, index, &state.price,
                wsol, signer,
            )?;
            close_account(wsol, &authority, &authority, signer)?;
            trader.deposit_native_sol(&keeper, &authority, fill.1, signer)?;
            // The wrapped-SOL account's rent goes back to the keeper, who paid it.
            let rent = authority.lamports() - lamports_before;
            system_transfer(&authority, &keeper, rent, signer)?;
            fill
        }
    };

    core.hedge(&trader, &market, &state, core.units(leg)?, signer)?;
    let after = core.refresh(&trader, &legs, &before, index)?;
    let leg_after = after.legs[index];
    require!(
        margin_ok(
            (before.margin.effective, before.total_notional()?),
            (after.margin.effective, after.total_notional()?),
            vault.margin_bps,
            false,
        ),
        VaultError::MarginTooThin
    );
    require!(
        leg_after.notional()? <= leg.leg.max_notional_usd as u128
            && leg_after.units <= leg.leg.max_asset_units,
        VaultError::MaxPositionReached
    );
    let open_interest = market_state(&core.perp_asset_map, leg.leg.asset_id)?.open_interest_lots;
    require!(
        leg_after.short_lots() as u128 * BPS_SCALE
            <= open_interest as u128 * leg.leg.max_oi_share_bps as u128,
        VaultError::OpenInterestShareReached
    );
    let nav_after = after.nav()?;
    // At 10,000 bps the leg may be the whole vault: its asset alone can be worth a little more
    // than the NAV, which its short's costs and fees come out of.
    require!(
        leg.leg.max_nav_share_bps as u128 >= BPS_SCALE
            || leg_after.value()? * BPS_SCALE <= nav_after * leg.leg.max_nav_share_bps as u128,
        VaultError::LegShareReached
    );

    emit!(Deployed {
        vault: vault_key,
        leg_index,
        usdc_spent: spent,
        asset_bought: bought,
        position_lots: leg_after.position_lots,
        nav_before_usd: to_u64(before.nav()?)?,
        nav_usd: to_u64(nav_after)?,
        supply: after.supply,
    });
    let vault = &mut ctx.accounts.core.vault;
    (vault.turnover_window_start, vault.turnover_window_usd) = window;
    Ok(())
}

#[derive(Accounts)]
pub struct Unwind<'info> {
    #[account(mut)]
    pub keeper: Signer<'info>,
    pub core: VaultCore<'info>,
    /// CHECK: the leg's order book, checked against the leg.
    #[account(mut)]
    pub orderbook: UncheckedAccount<'info>,
    /// CHECK: the leg's spline collection, checked against the leg.
    #[account(mut)]
    pub spline_collection: UncheckedAccount<'info>,
    /// CHECK: the whitelisted swap venue (Jupiter).
    #[account(address = core.config.swap_program @ VaultError::InvalidSwapAccounts)]
    pub swap_program: UncheckedAccount<'info>,
    /// CHECK: the authority's wrapped-SOL account, opened and closed around the SOL leg's swap.
    #[account(mut, address = get_associated_token_address(&core.authority.key(), &NATIVE_MINT))]
    pub vault_wsol: Option<UncheckedAccount<'info>>,
    /// CHECK: the native mint, for the SOL leg's wrapped-SOL account.
    #[account(address = NATIVE_MINT)]
    pub native_mint: Option<UncheckedAccount<'info>>,
    /// CHECK: Phoenix's withdraw queue, for taking the SOL leg's SOL out of Phoenix.
    #[account(mut, address = core.config.withdraw_queue)]
    pub withdraw_queue: Option<UncheckedAccount<'info>>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Sells `asset_amount` of leg `leg_index`'s asset back to buffer USDC through `route`, after
/// closing the same share of its short, so the rest of the leg stays hedged. The SOL leg's SOL
/// leaves Phoenix first; Phoenix only lets it go while the vault's margin stays covered. Open
/// while the protocol is paused: it only reduces what the vault holds. `remaining_accounts`:
/// Phoenix's index accounts, every leg (`load_legs`), then the route's accounts.
pub fn unwind<'info>(
    ctx: Context<'info, Unwind<'info>>,
    gti: u8,
    atb: u8,
    leg_index: u8,
    asset_amount: u64,
    route: SwapRoute,
) -> Result<()> {
    let a = &ctx.accounts;
    let core = &a.core;
    let vault = &core.vault;
    core.config.require_keeper(a.keeper.key)?;
    require!(asset_amount > 0, VaultError::ZeroAmount);

    let vault_key = vault.key();
    let start = gti as usize + atb as usize;
    let (legs, used) = load_legs(
        &vault_key,
        vault,
        ctx.remaining_accounts.get(start..).unwrap_or_default(),
    )?;
    let index = leg_index as usize;
    let leg = leg_at(&legs, leg_index)?;
    let market = market_accounts(leg, &a.orderbook, &a.spline_collection)?;
    let accounts = route_accounts(ctx.remaining_accounts, start + used, &route)?;

    let keeper = a.keeper.to_account_info();
    let token_program = a.token_program.to_account_info();
    let system_program = a.system_program.to_account_info();
    let withdraw_queue = a.withdraw_queue.as_ref().map(|w| w.as_ref());
    let trader = core.trader(
        ctx.remaining_accounts,
        gti,
        atb,
        &[&keeper, &a.orderbook, &a.spline_collection, &token_program, &system_program],
        withdraw_queue,
        None,
    )?;
    let before = core.snapshot(&trader, &legs, Pricing::Mark)?;
    let state = before.legs[index];
    require!(asset_amount <= state.units, VaultError::NotEnoughAsset);
    // In a multiplier window (a split, a special dividend) the token's multiplier and Phoenix's
    // mark may not have caught up with each other: its tokens may be worth up to the higher of
    // its two multipliers. The sale must fetch that, and counts that much against the turnover,
    // or a keeper could sell them at a fraction of their worth.
    let sale_price = if core.changing(leg)? {
        core.market(leg, Clock::get()?.unix_timestamp, Pricing::High)?
            .price(state.mark_ticks)?
    } else {
        state.price
    };
    let value = sale_price.usd_value(asset_amount as u128, state.market.decimals, false)?;
    let window = turnover(vault, value)?;

    let seeds = vault.authority_seeds(&vault_key);
    let signer: &[&[&[u8]]] = &[&seeds];
    let close = lots_to_close(state.short_lots(), state.units, asset_amount);
    core.trade(&trader, &market, &state, -(close as i128), signer)?;

    let venue = a.swap_program.to_account_info();
    let (sold, received) = match leg.holding {
        Some(holding) => core.swap(
            &venue, accounts, &route, Sell::Asset, asset_amount, &legs, index, &sale_price,
            holding, signer,
        )?,
        None => {
            // The SOL leaves Phoenix to the authority and is wrapped to be sold.
            let wsol = a.vault_wsol.as_ref().ok_or(VaultError::InvalidSwapAccounts)?;
            let native_mint = a.native_mint.as_ref().ok_or(VaultError::InvalidSwapAccounts)?;
            let authority = core.authority.to_account_info();
            create_ata_idempotent(
                &keeper,
                wsol,
                &authority,
                native_mint,
                &system_program,
                &token_program,
                &[],
            )?;
            trader.withdraw_native_sol(&authority, asset_amount, signer)?;
            system_transfer(&authority, wsol, asset_amount, signer)?;
            sync_native(wsol)?;
            let fill = core.swap(
                &venue, accounts, &route, Sell::Asset, asset_amount, &legs, index, &sale_price,
                wsol, signer,
            )?;
            // All of it was sold: closing the account pays the keeper its rent back.
            close_account(wsol, &keeper, &authority, signer)?;
            fill
        }
    };

    let after = core.refresh(&trader, &legs, &before, index)?;
    require!(
        margin_ok(
            (before.margin.effective, before.total_notional()?),
            (after.margin.effective, after.total_notional()?),
            vault.margin_bps,
            true,
        ),
        VaultError::MarginTooThin
    );
    emit!(Unwound {
        vault: vault_key,
        leg_index,
        asset_sold: sold,
        usdc_received: received,
        position_lots: after.legs[index].position_lots,
        nav_before_usd: to_u64(before.nav()?)?,
        nav_usd: to_u64(after.nav()?)?,
        supply: after.supply,
    });
    let vault = &mut ctx.accounts.core.vault;
    (vault.turnover_window_start, vault.turnover_window_usd) = window;
    Ok(())
}

#[derive(Accounts)]
pub struct Rehedge<'info> {
    #[account(mut)]
    pub keeper: Signer<'info>,
    pub core: VaultCore<'info>,
    /// CHECK: the leg's order book, checked against the leg.
    #[account(mut)]
    pub orderbook: UncheckedAccount<'info>,
    /// CHECK: the leg's spline collection, checked against the leg.
    #[account(mut)]
    pub spline_collection: UncheckedAccount<'info>,
}

/// Trades leg `leg_index`'s short back to the asset it holds: after Phoenix liquidated or
/// deleveraged part of it, or someone donated the asset. The margin may not end thinner than
/// before, or below target.
pub fn rehedge<'info>(ctx: Context<'info, Rehedge<'info>>, gti: u8, atb: u8, leg_index: u8) -> Result<()> {
    let a = &ctx.accounts;
    let core = &a.core;
    let vault = &core.vault;
    core.config.require_keeper(a.keeper.key)?;
    let vault_key = vault.key();
    let start = gti as usize + atb as usize;
    let (legs, _) = load_legs(
        &vault_key,
        vault,
        ctx.remaining_accounts.get(start..).unwrap_or_default(),
    )?;
    let index = leg_index as usize;
    let leg = leg_at(&legs, leg_index)?;
    // The hedge follows the token's multiplier: while a large change is in its window, the
    // multiplier and Phoenix's market may disagree on what the tokens are.
    require!(!core.changing(leg)?, VaultError::MultiplierChanging);
    let market = market_accounts(leg, &a.orderbook, &a.spline_collection)?;
    let keeper = a.keeper.to_account_info();
    let trader = core.trader(
        ctx.remaining_accounts,
        gti,
        atb,
        &[&keeper, &a.orderbook, &a.spline_collection],
        None,
        None,
    )?;
    let before = core.snapshot(&trader, &legs, Pricing::Mark)?;
    let state = before.legs[index];
    let seeds = vault.authority_seeds(&vault_key);
    core.hedge(&trader, &market, &state, state.units, &[&seeds])?;
    let after = core.refresh(&trader, &legs, &before, index)?;
    require!(
        margin_ok(
            (before.margin.effective, before.total_notional()?),
            (after.margin.effective, after.total_notional()?),
            vault.margin_bps,
            true,
        ),
        VaultError::MarginTooThin
    );
    emit!(Rehedged {
        vault: vault_key,
        leg_index,
        position_lots: after.legs[index].position_lots,
        nav_before_usd: to_u64(before.nav()?)?,
        nav_usd: to_u64(after.nav()?)?,
        supply: after.supply,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct MoveMargin<'info> {
    #[account(mut)]
    pub keeper: Signer<'info>,
    pub core: VaultCore<'info>,
    pub ember: EmberPrograms<'info>,
    /// CHECK: Phoenix's collateral token, minted and burned by Ember.
    #[account(mut, address = core.config.phoenix_mint)]
    pub phoenix_mint: UncheckedAccount<'info>,
    /// CHECK: Phoenix's withdraw queue.
    #[account(mut, address = core.config.withdraw_queue)]
    pub withdraw_queue: UncheckedAccount<'info>,
    /// CHECK: Phoenix's vault for its collateral token.
    #[account(mut, address = core.config.global_vault)]
    pub global_vault: UncheckedAccount<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

/// Moves `amount` of USDC from the buffer to Phoenix collateral (`to_phoenix`), as margin for
/// the spot legs' shorts, or back, as funding to reinvest or USDC for withdrawals. In, it may
/// not take the buffer below its reserve; out, the margin may not fall below target.
pub fn move_margin<'info>(
    ctx: Context<'info, MoveMargin<'info>>,
    gti: u8,
    atb: u8,
    to_phoenix: bool,
    amount: u64,
) -> Result<()> {
    let a = &ctx.accounts;
    let core = &a.core;
    let vault = &core.vault;
    core.config.require_keeper(a.keeper.key)?;
    require!(amount > 0, VaultError::ZeroAmount);
    let vault_key = vault.key();
    let start = gti as usize + atb as usize;
    let (legs, _) = load_legs(
        &vault_key,
        vault,
        ctx.remaining_accounts.get(start..).unwrap_or_default(),
    )?;
    let token_program = a.token_program.to_account_info();
    let system_program = a.system_program.to_account_info();
    let trader = core.trader(
        ctx.remaining_accounts,
        gti,
        atb,
        &[
            &a.keeper,
            &a.ember.ember_program,
            &a.ember.ember_state,
            &a.ember.ember_vault,
            &a.phoenix_mint,
            &token_program,
            &system_program,
        ],
        Some(&a.withdraw_queue),
        Some(&a.global_vault),
    )?;
    let ember = Ember {
        program: &a.ember.ember_program,
        state: &a.ember.ember_state,
        vault: &a.ember.ember_vault,
        usdc_mint: &core.usdc_mint,
        phoenix_mint: &a.phoenix_mint,
        usdc_account: &core.vault_usdc,
        phoenix_account: &core.vault_phoenix,
    };
    let before = core.snapshot(&trader, &legs, Pricing::Mark)?;
    let seeds = vault.authority_seeds(&vault_key);
    let signer: &[&[&[u8]]] = &[&seeds];
    if to_phoenix {
        core.config.require_live()?;
        let reserved = before.reserved_usdc(vault.buffer_bps)?;
        // USDC that covers what the vault owes on Phoenix (the SOL leg's short after a rally),
        // and the fees of closing its shorts, may come out of the reserve: the NAV does not
        // move, and until it is covered Phoenix will not let the SOL go that would pay the
        // requests the reserve is for.
        let owed = (-before.margin.equity).max(0) as u128
            + mul_div(before.total_notional()?, PHOENIX_COST_CUSHION_BPS, BPS_SCALE, false)?;
        require!(
            amount <= before.idle_usdc
                && ((before.idle_usdc - amount) as u128 >= reserved || amount as u128 <= owed),
            VaultError::BufferTooThin
        );
        trader.deposit_usdc(&ember, amount, signer)?;
    } else {
        trader.withdraw_usdc(&ember, amount, signer)?;
    }
    let mut after = before.clone();
    after.idle_usdc = token_amount(&core.vault_usdc)?;
    after.margin = core.margin(&trader)?;
    if !to_phoenix {
        require!(
            margin_ok(
                (before.margin.effective, before.total_notional()?),
                (after.margin.effective, after.total_notional()?),
                vault.margin_bps,
                false,
            ),
            VaultError::MarginTooThin
        );
    }
    emit!(MarginMoved {
        vault: vault_key,
        to_phoenix,
        amount,
        nav_usd: to_u64(after.nav()?)?,
        supply: after.supply,
    });
    Ok(())
}
