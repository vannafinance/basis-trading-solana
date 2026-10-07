//! Vanna basis vaults on Solana, hedged on Phoenix perpetuals.
//!
//! One vault per asset (SOL, ETH, BTC, and tokenized stocks such as TSLA and META). A
//! depositor gets the vault's shares; the vault holds the asset and a short of the same size
//! on Phoenix, so its value stays flat in USD while it earns the funding that longs pay
//! shorts. The share price is the vault's NAV over its supply. There are no fees and no
//! stablecoin; nothing is shared between vaults.
//!
//! Phoenix takes native SOL as collateral, so the SOL vault posts the deposit itself and
//! shorts the same amount: 10 SOL in, 10 SOL short. ETH, BTC and stocks are not Phoenix
//! collateral, so those vaults hold the asset as tokens (Token-2022 xStocks for stocks) and
//! post USDC as the short's margin.
//!
//! Each vault has caps: its short's notional, the asset it holds, its share of the market's
//! open interest and the size of one deposit. Stock vaults close deposits while their market
//! trades after hours.

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
use state::VaultKind;

declare_id!("GtuQoYxzytGKtLqQGkYrVbsaPHTZN5jboP25DrSmKvGr");

#[program]
pub mod vanna_phoenix_vault {
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
        kind: VaultKind,
        market: MarketParams,
        params: VaultParams,
        authority_nonce: u64,
    ) -> Result<()> {
        instructions::create_vault(ctx, kind, market, params, authority_nonce)
    }

    pub fn update_vault(ctx: Context<UpdateVault>, params: VaultParams) -> Result<()> {
        instructions::update_vault(ctx, params)
    }

    /// The keeper's funding guard (or the admin): open or close a vault's deposits.
    pub fn set_deposits_open(ctx: Context<SetDeposits>, open: bool) -> Result<()> {
        instructions::set_deposits_open(ctx, open)
    }

    // ---------------------------------------------------------------- users
    //
    // `gti` and `atb` are how many of Phoenix's global trader index and active trader buffer
    // accounts lead `remaining_accounts`.

    pub fn deposit<'info>(
        ctx: Context<'info, Deposit<'info>>,
        gti: u8,
        atb: u8,
        asset_amount: u64,
        usdc_amount: u64,
        min_shares: u64,
        route: Option<swap::SwapRoute>,
    ) -> Result<()> {
        instructions::deposit(ctx, gti, atb, asset_amount, usdc_amount, min_shares, route)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn withdraw<'info>(
        ctx: Context<'info, Withdraw<'info>>,
        gti: u8,
        atb: u8,
        shares: u64,
        min_asset_out: u64,
        min_usdc_out: u64,
        receive: swap::Receive,
        route: Option<swap::SwapRoute>,
    ) -> Result<()> {
        instructions::withdraw(
            ctx,
            gti,
            atb,
            shares,
            min_asset_out,
            min_usdc_out,
            receive,
            route,
        )
    }

    // ---------------------------------------------------------------- keeper

    pub fn rebalance<'info>(
        ctx: Context<'info, Rebalance<'info>>,
        gti: u8,
        atb: u8,
        buy_asset: bool,
        asset_amount: u64,
        usdc_amount: u64,
    ) -> Result<()> {
        instructions::rebalance(ctx, gti, atb, buy_asset, asset_amount, usdc_amount)
    }
}
