//! Deposits and withdrawals. Users bring and take USDC only, and never trade: a deposit lands
//! in the vault's buffer and the keeper deploys it later; a withdrawal leaves from the buffer,
//! at once when it holds enough, or through a request the keeper raises USDC for. Shares are
//! priced exactly as in the single-asset vault, from the NAV of every leg.

use crate::constants::*;
use crate::errors::VaultError;
use crate::events::{Deposited, WithdrawCancelled, WithdrawRequested, WithdrawReturned, Withdrawn};
use crate::instructions::common::*;
use crate::instructions::vault_core::*;
use crate::math::*;
use crate::nav::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::associated_token::{get_associated_token_address, AssociatedToken};
use anchor_spl::token::{Token, TokenAccount};

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    pub core: VaultCore<'info>,
    #[account(mut, token::mint = core.usdc_mint, token::authority = user)]
    pub user_usdc: Box<Account<'info, TokenAccount>>,
    /// CHECK: the user's share account (their associated token account), created if missing.
    #[account(
        mut,
        address = get_associated_token_address(user.key, &core.share_mint.key()),
    )]
    pub user_shares: UncheckedAccount<'info>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Deposits `usdc_amount` of USDC into the vault's buffer and mints shares for its value at
/// the NAV before it, less `entry_fee_bps` (which stays in the vault, for the trades the
/// keeper will make to deploy it). `remaining_accounts`: Phoenix's index accounts (`gti`,
/// `atb`), then every leg (`load_legs`).
///
/// A stock token in the middle of a large multiplier change is valued at the higher of its two
/// multipliers (`Pricing::High`): a briefly wrong NAV cannot sell shares too cheap. Refused
/// while the vault owes more than it holds, past `max_deposit_usd` or past `max_tvl_usd`.
pub fn deposit<'info>(
    ctx: Context<'info, Deposit<'info>>,
    gti: u8,
    atb: u8,
    usdc_amount: u64,
    min_shares: u64,
) -> Result<()> {
    let a = &ctx.accounts;
    let core = &a.core;
    let vault = &core.vault;
    core.config.require_live()?;
    require!(vault.deposits_open, VaultError::DepositsClosed);
    require!(usdc_amount > 0, VaultError::ZeroAmount);
    require!(
        usdc_amount <= vault.max_deposit_usd,
        VaultError::DepositTooLarge
    );

    let vault_key = vault.key();
    let start = gti as usize + atb as usize;
    let (legs, _) = load_legs(
        &vault_key,
        vault,
        ctx.remaining_accounts.get(start..).unwrap_or_default(),
    )?;
    let token_program = a.token_program.to_account_info();
    let trader = core.trader(ctx.remaining_accounts, gti, atb, &[&token_program], None, None)?;
    let before = core.snapshot(&trader, &legs, Pricing::High)?;
    let supply = before.supply;
    // A vault that owes more than it holds would spend the deposit on its debt first.
    let equity = before.equity()?;
    require!(
        equity > 0 || (supply == 0 && equity == 0),
        VaultError::VaultInsolvent
    );
    let nav_before = before.nav()?;
    let nav_after = nav_before + usdc_amount as u128;
    require!(
        nav_after <= vault.max_tvl_usd as u128,
        VaultError::MaxTvlReached
    );

    let value = mul_div(
        usdc_amount as u128,
        BPS_SCALE - vault.entry_fee_bps as u128,
        BPS_SCALE,
        false,
    )?;
    require!(value > 0, VaultError::NothingDeposited);
    let shares = to_u64(shares_for_value(value, nav_before, supply)?)?;
    require!(shares > 0 && shares >= min_shares, VaultError::SharesTooLow);

    let user = a.user.to_account_info();
    transfer(
        a.user_usdc.as_ref().as_ref(),
        &core.vault_usdc,
        &user,
        usdc_amount,
        &[],
    )?;
    let seeds = vault.authority_seeds(&vault_key);
    let signer: &[&[&[u8]]] = &[&seeds];
    let share_mint = core.share_mint.to_account_info();
    let user_shares = a.user_shares.to_account_info();
    create_ata_idempotent(
        &user,
        &user_shares,
        &user,
        &share_mint,
        &a.system_program.to_account_info(),
        &token_program,
        &[],
    )?;
    mint_to(&share_mint, &user_shares, &core.authority, shares, signer)?;

    emit!(Deposited {
        vault: vault_key,
        user: a.user.key(),
        usdc_amount,
        shares,
        value_usd: to_u64(value)?,
        nav_before_usd: to_u64(nav_before)?,
        nav_usd: to_u64(nav_after)?,
        supply: supply + shares,
    });
    Ok(())
}

/// What leaving with `shares` pays, before and after the exit fee: their part of the NAV,
/// priced like a deposit so a deposit and a withdrawal at once move no value.
fn payout(snap: &Snapshot, shares: u64, exit_fee_bps: u16) -> Result<(u128, u128)> {
    let nav = snap.nav()?;
    let payout = withdrawal_payout(nav, nav, shares, snap.supply)?;
    let fee = mul_div(payout, exit_fee_bps as u128, BPS_SCALE, true)?;
    Ok((payout, payout - fee))
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    pub user: Signer<'info>,
    pub core: VaultCore<'info>,
    #[account(mut, associated_token::mint = core.share_mint, associated_token::authority = user)]
    pub user_shares: Box<Account<'info, TokenAccount>>,
    #[account(mut, token::mint = core.usdc_mint, token::authority = user)]
    pub user_usdc: Box<Account<'info, TokenAccount>>,
    pub token_program: Program<'info, Token>,
}

/// Burns `shares` and pays their part of the NAV in USDC from the buffer at once, less
/// `exit_fee_bps` (which stays in the vault). Only the buffer above what live requests are
/// worth can pay: requests come first. When it is not enough, `request_withdraw` instead.
/// Withdrawals stay open while the protocol is paused, and price a stock token in a multiplier
/// change at the lower of its two multipliers (`Pricing::Low`).
pub fn withdraw<'info>(
    ctx: Context<'info, Withdraw<'info>>,
    gti: u8,
    atb: u8,
    shares: u64,
    min_usdc_out: u64,
) -> Result<()> {
    let a = &ctx.accounts;
    let core = &a.core;
    let vault = &core.vault;
    require!(shares > 0, VaultError::ZeroAmount);
    require!(shares <= a.user_shares.amount, VaultError::NotEnoughShares);

    let vault_key = vault.key();
    let start = gti as usize + atb as usize;
    let (legs, _) = load_legs(
        &vault_key,
        vault,
        ctx.remaining_accounts.get(start..).unwrap_or_default(),
    )?;
    let trader = core.trader(ctx.remaining_accounts, gti, atb, &[], None, None)?;
    let snap = core.snapshot(&trader, &legs, Pricing::Low)?;
    let (gross, pay) = payout(&snap, shares, vault.exit_fee_bps)?;
    require!(pay > 0, VaultError::NothingToWithdraw);
    let available = (snap.idle_usdc as u128).saturating_sub(snap.pending_value()?);
    require!(pay <= available, VaultError::NotEnoughLiquidity);
    require!(pay >= min_usdc_out as u128, VaultError::PayoutTooLow);

    let seeds = vault.authority_seeds(&vault_key);
    let signer: &[&[&[u8]]] = &[&seeds];
    burn(
        &core.share_mint,
        &a.user_shares.to_account_info(),
        &a.user.to_account_info(),
        shares,
        &[],
    )?;
    let pay = to_u64(pay)?;
    transfer(
        &core.vault_usdc,
        a.user_usdc.as_ref().as_ref(),
        &core.authority,
        pay,
        signer,
    )?;
    emit!(Withdrawn {
        vault: vault_key,
        user: a.user.key(),
        shares,
        payout_usd: to_u64(gross)?,
        fee_usd: to_u64(gross)? - pay,
        usdc_out: pay,
        request_id: None,
        nav_usd: to_u64(snap.nav()? - pay as u128)?,
        supply: snap.supply - shares,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct RequestWithdraw<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [VAULT_SEED, &vault.id.to_le_bytes()], bump = vault.bump)]
    pub vault: Box<Account<'info, Vault>>,
    /// CHECK: the vault's escrow share account.
    #[account(mut, address = vault.escrow_shares)]
    pub escrow_shares: UncheckedAccount<'info>,
    #[account(mut, associated_token::mint = vault.share_mint, associated_token::authority = user)]
    pub user_shares: Box<Account<'info, TokenAccount>>,
    #[account(
        init,
        payer = user,
        space = 8 + WithdrawRequest::INIT_SPACE,
        seeds = [REQUEST_SEED, vault.key().as_ref(), &vault.next_request_id.to_le_bytes()],
        bump,
    )]
    pub request: Box<Account<'info, WithdrawRequest>>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

/// Puts `shares` in the vault's escrow for a withdrawal the buffer cannot pay now. The keeper
/// unwinds legs to raise the USDC and pays requests oldest first (`fulfill_withdraw`), at the
/// NAV then. The shares keep earning (or losing) with everyone's until then, and the user can
/// `cancel_withdraw` to take them back. The buffer is reserved for live requests before
/// instant withdrawals. A request has no minimum: it could otherwise ask for more than it can
/// ever be paid and hold the buffer forever.
pub fn request_withdraw(ctx: Context<RequestWithdraw>, shares: u64) -> Result<()> {
    let a = &ctx.accounts;
    require!(shares > 0, VaultError::ZeroAmount);
    require!(shares <= a.user_shares.amount, VaultError::NotEnoughShares);
    transfer(
        a.user_shares.as_ref().as_ref(),
        &a.escrow_shares,
        &a.user.to_account_info(),
        shares,
        &[],
    )?;
    let vault_key = a.vault.key();
    let id = a.vault.next_request_id;
    let request = WithdrawRequest {
        vault: vault_key,
        user: a.user.key(),
        id,
        shares,
        created_at: Clock::get()?.unix_timestamp,
        bump: ctx.bumps.request,
        returned: false,
    };
    emit!(WithdrawRequested {
        vault: vault_key,
        user: request.user,
        request: a.request.key(),
        request_id: id,
        shares,
    });
    ctx.accounts.request.set_inner(request);
    let vault = &mut ctx.accounts.vault;
    vault.next_request_id = id + 1;
    vault.pending_shares = vault
        .pending_shares
        .checked_add(shares)
        .ok_or(VaultError::MathOverflow)?;
    Ok(())
}

#[derive(Accounts)]
pub struct CancelWithdraw<'info> {
    /// The request's user.
    #[account(mut, address = request.user @ VaultError::Unauthorized)]
    pub user: Signer<'info>,
    #[account(mut, seeds = [VAULT_SEED, &vault.id.to_le_bytes()], bump = vault.bump)]
    pub vault: Box<Account<'info, Vault>>,
    /// CHECK: the vault's authority.
    #[account(address = vault.authority)]
    pub authority: UncheckedAccount<'info>,
    /// CHECK: the vault's share mint: the exit fee is burned from the escrow.
    #[account(mut, address = vault.share_mint)]
    pub share_mint: UncheckedAccount<'info>,
    /// CHECK: the vault's escrow share account.
    #[account(mut, address = vault.escrow_shares)]
    pub escrow_shares: UncheckedAccount<'info>,
    /// CHECK: the user's share account, created (at their cost) if they closed it.
    #[account(mut, address = get_associated_token_address(&request.user, &vault.share_mint))]
    pub user_shares: UncheckedAccount<'info>,
    #[account(mut, has_one = vault, close = user)]
    pub request: Box<Account<'info, WithdrawRequest>>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// The user takes a request's shares back and closes it. Leaving the queue costs what leaving
/// the vault costs, `exit_fee_bps` of the shares (burned, so their value stays with the
/// holders): the keeper may already have traded to raise the USDC, and a free cancel would let
/// anyone make it churn the vault at everyone's cost. A request the keeper returned
/// (`return_withdraw`) comes back whole.
pub fn cancel_withdraw(ctx: Context<CancelWithdraw>) -> Result<()> {
    let a = &ctx.accounts;
    let vault_key = a.vault.key();
    let seeds = a.vault.authority_seeds(&vault_key);
    let signer: &[&[&[u8]]] = &[&seeds];
    let user = a.user.to_account_info();
    let user_shares = a.user_shares.to_account_info();
    create_ata_idempotent(
        &user,
        &user_shares,
        &user,
        &a.share_mint.to_account_info(),
        &a.system_program.to_account_info(),
        &a.token_program.to_account_info(),
        &[],
    )?;
    let shares = a.request.shares;
    let returned = a.request.returned;
    let fee_shares = if returned {
        0
    } else {
        to_u64(mul_div(shares as u128, a.vault.exit_fee_bps as u128, BPS_SCALE, true)?)?
    };
    burn(&a.share_mint, &a.escrow_shares, &a.authority, fee_shares, signer)?;
    transfer(&a.escrow_shares, &user_shares, &a.authority, shares - fee_shares, signer)?;
    emit!(WithdrawCancelled {
        vault: vault_key,
        user: a.request.user,
        request_id: a.request.id,
        shares: shares - fee_shares,
        fee_shares,
    });
    if !returned {
        let vault = &mut ctx.accounts.vault;
        vault.pending_shares = vault.pending_shares.saturating_sub(shares);
    }
    Ok(())
}

#[derive(Accounts)]
pub struct ReturnWithdraw<'info> {
    /// The keeper or the admin.
    pub signer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [VAULT_SEED, &vault.id.to_le_bytes()], bump = vault.bump)]
    pub vault: Box<Account<'info, Vault>>,
    #[account(mut, has_one = vault)]
    pub request: Box<Account<'info, WithdrawRequest>>,
}

/// The keeper gives up on a request it cannot pay: its user closed, gave away or had frozen
/// their USDC account, or it comes to nothing. No token moves (a user could otherwise block
/// this too, through their own accounts): the shares stay in the escrow, earning as before,
/// no longer reserve the buffer, and only their user can take them back (`cancel_withdraw`,
/// at no fee). The queue goes on to the next request.
pub fn return_withdraw(ctx: Context<ReturnWithdraw>) -> Result<()> {
    let signer = ctx.accounts.signer.key();
    ctx.accounts.config.require_keeper(&signer)?;
    let request = &mut ctx.accounts.request;
    require!(!request.returned, VaultError::RequestReturned);
    request.returned = true;
    let shares = request.shares;
    emit!(WithdrawReturned {
        vault: request.vault,
        user: request.user,
        request_id: request.id,
        shares,
        by: signer,
    });
    let vault = &mut ctx.accounts.vault;
    vault.pending_shares = vault.pending_shares.saturating_sub(shares);
    Ok(())
}

#[derive(Accounts)]
pub struct FulfillWithdraw<'info> {
    /// The keeper, which pays requests oldest first; or anyone else (the user too) once the
    /// buffer holds this request and every other live one.
    #[account(mut)]
    pub cranker: Signer<'info>,
    pub core: VaultCore<'info>,
    #[account(mut, has_one = user, constraint = request.vault == core.vault.key() @ VaultError::InvalidParameter, close = user)]
    pub request: Box<Account<'info, WithdrawRequest>>,
    /// CHECK: the request's user, who gets its rent back.
    #[account(mut)]
    pub user: UncheckedAccount<'info>,
    /// The user's USDC account: their associated token account, still theirs. One they closed,
    /// gave away or had frozen is not paid into, or created at the cranker's cost: the keeper
    /// returns the request instead (`return_withdraw`).
    #[account(
        mut,
        address = get_associated_token_address(user.key, &core.usdc_mint.key()) @ VaultError::InvalidTokenAccount,
        token::mint = core.usdc_mint,
        token::authority = user,
    )]
    pub user_usdc: Box<Account<'info, TokenAccount>>,
    pub token_program: Program<'info, Token>,
}

/// Pays a pending request from the buffer at the NAV now (`Pricing::Low`), less
/// `exit_fee_bps`, burns its escrowed shares and closes it. The USDC always goes to the
/// request's user.
///
/// The keeper may pay any request (it pays them oldest first). Anyone else, the request's user
/// included, may pay one only from the buffer above what every other live request is worth: a
/// request cannot jump the queue and take USDC the keeper raised for an older one. And while a
/// stock token is in a multiplier window, where tokens are priced low for whoever leaves, only
/// the user or the keeper chooses that moment.
pub fn fulfill_withdraw<'info>(
    ctx: Context<'info, FulfillWithdraw<'info>>,
    gti: u8,
    atb: u8,
) -> Result<()> {
    let a = &ctx.accounts;
    let core = &a.core;
    let vault = &core.vault;
    let request = &a.request;
    require!(!request.returned, VaultError::RequestReturned);

    let vault_key = vault.key();
    let start = gti as usize + atb as usize;
    let (legs, _) = load_legs(
        &vault_key,
        vault,
        ctx.remaining_accounts.get(start..).unwrap_or_default(),
    )?;
    let trader = core.trader(ctx.remaining_accounts, gti, atb, &[], None, None)?;
    let snap = core.snapshot(&trader, &legs, Pricing::Low)?;
    let (gross, pay) = payout(&snap, request.shares, vault.exit_fee_bps)?;
    require!(pay > 0, VaultError::NothingToWithdraw);

    let cranker = a.cranker.key();
    let keeper = cranker == core.config.keeper || cranker == core.config.admin;
    let others = match vault.pending_shares.saturating_sub(request.shares) {
        _ if keeper => 0,
        0 => 0,
        // What the other requests are worth once this one has left.
        shares => {
            let nav = snap.nav()?.saturating_sub(pay);
            withdrawal_payout(nav, nav, shares, snap.supply - request.shares)?
        }
    };
    require!(
        pay + others <= snap.idle_usdc as u128,
        VaultError::NotEnoughLiquidity
    );
    if !keeper && cranker != request.user {
        for leg in &legs {
            require!(!core.changing(leg)?, VaultError::MultiplierChanging);
        }
    }

    let seeds = vault.authority_seeds(&vault_key);
    let signer: &[&[&[u8]]] = &[&seeds];
    burn(
        &core.share_mint,
        &core.escrow_shares,
        &core.authority,
        request.shares,
        signer,
    )?;
    let pay = to_u64(pay)?;
    transfer(
        &core.vault_usdc,
        a.user_usdc.as_ref().as_ref(),
        &core.authority,
        pay,
        signer,
    )?;
    emit!(Withdrawn {
        vault: vault_key,
        user: request.user,
        shares: request.shares,
        payout_usd: to_u64(gross)?,
        fee_usd: to_u64(gross)? - pay,
        usdc_out: pay,
        request_id: Some(request.id),
        nav_usd: to_u64(snap.nav()? - pay as u128)?,
        supply: snap.supply - request.shares,
    });
    let shares = request.shares;
    let vault = &mut ctx.accounts.core.vault;
    vault.pending_shares = vault.pending_shares.saturating_sub(shares);
    Ok(())
}
