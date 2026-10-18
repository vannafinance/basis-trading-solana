//! Vanna's multi-strategy basis vault on Solana, hedged on Phoenix perpetuals.
//!
//! One vault, many markets. Users deposit and withdraw USDC and hold the vault's shares. The
//! keeper watches the funding and open interest of Phoenix's markets and spreads the USDC over
//! the vault's legs: each leg holds an asset and shorts the same amount on Phoenix, so the
//! vault's value stays flat in USD while it earns the funding longs pay shorts, wherever it is
//! best. The SOL leg holds native SOL, which Phoenix counts as collateral; every other leg
//! holds its asset as tokens, with USDC margin on Phoenix for its short. All shorts sit in one
//! Phoenix cross-margin account.
//!
//! The share price is the NAV over the supply, priced exactly as in the single-asset vault:
//! `NAV = idle USDC + Σ legs (asset held × mark × multiplier) + Phoenix equity`, with the same
//! virtual offset on deposits and withdrawals. Deposits wait in a USDC buffer for the keeper
//! to deploy; withdrawals leave from it at once, or through a request the keeper raises USDC
//! for. The keeper's moves are held to on-chain limits: a buffer and margin floor, per-leg
//! caps, price bands on every order and swap, and a daily turnover limit.

pub mod constants;
pub mod errors;
pub mod events;
pub mod instructions;
pub mod math;
pub mod nav;
pub mod phoenix;
pub mod state;
pub mod swap;

use anchor_lang::prelude::*;
use instructions::*;
use state::LegKind;

declare_id!("ppTyvPjN6xCEvCX7zsjPBip393fVVm8RMvkiU3Lz9WA");

#[program]
pub mod vanna_phoenix_multi_vault {
    use super::*;

    // ---------------------------------------------------------------- admin

    pub fn initialize(
        ctx: Context<Initialize>,
        keeper: Pubkey,
        swap_program: Pubkey,
    ) -> Result<()> {
        instructions::initialize(ctx, keeper, swap_program)
    }

    pub fn set_config(
        ctx: Context<AdminOnly>,
        admin: Pubkey,
        keeper: Pubkey,
        swap_program: Pubkey,
        paused: bool,
    ) -> Result<()> {
        instructions::set_config(ctx, admin, keeper, swap_program, paused)
    }

    pub fn create_vault(
        ctx: Context<CreateVault>,
        id: u16,
        params: VaultParams,
        authority_nonce: u64,
    ) -> Result<()> {
        instructions::create_vault(ctx, id, params, authority_nonce)
    }

    pub fn update_vault(ctx: Context<UpdateVault>, params: VaultParams) -> Result<()> {
        instructions::update_vault(ctx, params)
    }

    pub fn add_leg<'info>(
        ctx: Context<'info, AddLeg<'info>>,
        kind: LegKind,
        market: MarketParams,
        params: LegParams,
    ) -> Result<()> {
        instructions::add_leg(ctx, kind, market, params)
    }

    pub fn update_leg(ctx: Context<UpdateLeg>, params: LegParams) -> Result<()> {
        instructions::update_leg(ctx, params)
    }

    pub fn retire_leg<'info>(
        ctx: Context<'info, RetireLeg<'info>>,
        gti: u8,
        atb: u8,
        max_units: u64,
    ) -> Result<()> {
        instructions::retire_leg(ctx, gti, atb, max_units)
    }

    /// The keeper's funding guard (or the admin): open or close the vault's deposits.
    pub fn set_deposits_open(ctx: Context<SetDeposits>, open: bool) -> Result<()> {
        instructions::set_deposits_open(ctx, open)
    }

    // ---------------------------------------------------------------- users
    //
    // `gti` and `atb` are how many of Phoenix's global trader index and active trader buffer
    // accounts lead `remaining_accounts`; every leg follows them (`load_legs`).

    pub fn deposit<'info>(
        ctx: Context<'info, Deposit<'info>>,
        gti: u8,
        atb: u8,
        usdc_amount: u64,
        min_shares: u64,
    ) -> Result<()> {
        instructions::deposit(ctx, gti, atb, usdc_amount, min_shares)
    }

    pub fn withdraw<'info>(
        ctx: Context<'info, Withdraw<'info>>,
        gti: u8,
        atb: u8,
        shares: u64,
        min_usdc_out: u64,
    ) -> Result<()> {
        instructions::withdraw(ctx, gti, atb, shares, min_usdc_out)
    }

    pub fn request_withdraw(ctx: Context<RequestWithdraw>, shares: u64) -> Result<()> {
        instructions::request_withdraw(ctx, shares)
    }

    pub fn cancel_withdraw(ctx: Context<CancelWithdraw>) -> Result<()> {
        instructions::cancel_withdraw(ctx)
    }

    /// The keeper (or the admin) gives up on a request it cannot pay; its user takes the
    /// shares back with `cancel_withdraw`.
    pub fn return_withdraw(ctx: Context<ReturnWithdraw>) -> Result<()> {
        instructions::return_withdraw(ctx)
    }

    pub fn fulfill_withdraw<'info>(
        ctx: Context<'info, FulfillWithdraw<'info>>,
        gti: u8,
        atb: u8,
    ) -> Result<()> {
        instructions::fulfill_withdraw(ctx, gti, atb)
    }

    // ---------------------------------------------------------------- keeper

    pub fn deploy<'info>(
        ctx: Context<'info, Deploy<'info>>,
        gti: u8,
        atb: u8,
        leg_index: u8,
        usdc_amount: u64,
        route: swap::SwapRoute,
    ) -> Result<()> {
        instructions::deploy(ctx, gti, atb, leg_index, usdc_amount, route)
    }

    pub fn unwind<'info>(
        ctx: Context<'info, Unwind<'info>>,
        gti: u8,
        atb: u8,
        leg_index: u8,
        asset_amount: u64,
        route: swap::SwapRoute,
    ) -> Result<()> {
        instructions::unwind(ctx, gti, atb, leg_index, asset_amount, route)
    }

    pub fn rehedge<'info>(
        ctx: Context<'info, Rehedge<'info>>,
        gti: u8,
        atb: u8,
        leg_index: u8,
    ) -> Result<()> {
        instructions::rehedge(ctx, gti, atb, leg_index)
    }

    pub fn move_margin<'info>(
        ctx: Context<'info, MoveMargin<'info>>,
        gti: u8,
        atb: u8,
        to_phoenix: bool,
        amount: u64,
    ) -> Result<()> {
        instructions::move_margin(ctx, gti, atb, to_phoenix, amount)
    }
}
