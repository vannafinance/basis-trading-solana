use crate::constants::*;
use crate::errors::VaultError;
use anchor_lang::prelude::*;

/// Protocol-wide roles and the Phoenix accounts every vault shares.
#[account]
#[derive(InitSpace)]
pub struct Config {
    pub admin: Pubkey,
    /// The bot that reinvests funding and keeps each vault's margin near its target.
    pub keeper: Pubkey,
    pub usdc_mint: Pubkey,
    /// Phoenix's collateral token (PhUSD), minted 1:1 for USDC by Ember.
    pub phoenix_mint: Pubkey,
    pub perp_asset_map: Pubkey,
    pub withdraw_queue: Pubkey,
    /// Phoenix's token vault for `phoenix_mint`.
    pub global_vault: Pubkey,
    /// The swap venue the vault may route single-asset deposits and withdrawals through
    /// (Jupiter on mainnet).
    pub swap_program: Pubkey,
    pub paused: bool,
    pub bump: u8,
}

impl Config {
    pub fn require_admin(&self, signer: &Pubkey) -> Result<()> {
        require_keys_eq!(*signer, self.admin, VaultError::Unauthorized);
        Ok(())
    }

    pub fn require_keeper(&self, signer: &Pubkey) -> Result<()> {
        require!(
            *signer == self.keeper || *signer == self.admin,
            VaultError::Unauthorized
        );
        Ok(())
    }

    pub fn require_live(&self) -> Result<()> {
        require!(!self.paused, VaultError::Paused);
        Ok(())
    }
}

/// How a vault holds the asset it hedges.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum VaultKind {
    /// SOL: the deposit is Phoenix collateral itself (native SOL in the trader account) and
    /// backs a short of the same size. 10 SOL in, 10 SOL short.
    NativeSol,
    /// ETH, BTC, tokenized stocks: Phoenix only takes USDC and SOL as collateral, so the
    /// vault holds the asset as spot tokens and posts USDC as the short's margin. At 1x
    /// margin a deposit is half asset and half USDC: 5 ETH held, 5 ETH short.
    Spot,
}

/// One vault per asset. Depositors get the vault's shares; the vault holds the asset and an
/// equal short on Phoenix, so its value is flat in USD and it earns the funding longs pay
/// shorts. The share price is the vault's NAV over its supply. Phoenix fills market orders
/// in the same instruction, so every deposit and withdrawal settles at once and is priced
/// from the NAV before and after its own trade: each user carries their own trading cost.
#[account]
#[derive(InitSpace)]
pub struct BasisVault {
    pub kind: VaultKind,
    /// The asset; the native mint for the SOL vault.
    pub asset_mint: Pubkey,
    pub asset_decimals: u8,
    /// The asset's token program: SPL Token, or Token-2022 for tokenized stocks.
    pub asset_token_program: Pubkey,
    pub share_mint: Pubkey,
    pub authority: Pubkey,
    /// The authority's token accounts; `asset_account` is unset for the SOL vault.
    pub usdc_account: Pubkey,
    pub phoenix_account: Pubkey,
    pub asset_account: Pubkey,
    pub trader_account: Pubkey,
    /// The Phoenix market the vault shorts.
    pub asset_id: u32,
    pub orderbook: Pubkey,
    pub spline_collection: Pubkey,
    /// Base lots per whole asset are `10^base_lot_decimals`.
    pub base_lot_decimals: u8,
    /// Quote lots per base lot per price tick.
    pub tick_size: u64,
    /// Max short notional (USD, 6 dp).
    pub max_position_usd: u64,
    /// Max asset the vault holds, in its smallest units. For the SOL vault this stays under
    /// the native SOL Phoenix counts as collateral per trader (500 SOL on mainnet).
    pub max_asset_units: u64,
    /// Max share of the Phoenix market's open interest the vault's short may be, in bps.
    /// Shorts earn funding only while longs outweigh them; a vault that is most of the short
    /// side pushes funding against itself.
    pub max_oi_share_bps: u16,
    /// Max value a single deposit may add (USD, 6 dp): large deposits come in parts the
    /// order book can take.
    pub max_deposit_usd: u64,
    /// Price band around Phoenix's mark price for the vault's orders and users' swaps.
    pub slippage_bps: u16,
    /// The keeper's own, tighter band for `rebalance`, where the keeper is the vault's
    /// counterparty.
    pub keeper_band_bps: u16,
    /// Max value the keeper may rebalance per `REBALANCE_WINDOW_SECONDS` (USD, 6 dp): what a
    /// stolen keeper key could cost the vault is at most the band on this.
    pub max_rebalance_usd: u64,
    pub rebalance_window_start: i64,
    pub rebalance_window_usd: u64,
    /// Phoenix effective collateral the vault keeps per dollar of short, in bps. Deposits
    /// and reinvesting may not take the vault below it.
    pub margin_bps: u16,
    /// USDC margin a single-asset deposit sets aside per dollar of asset, in bps: 10,000
    /// (1x) splits a deposit half asset, half USDC. 0 for the SOL vault, whose SOL is its own
    /// margin.
    pub target_margin_bps: u16,
    pub deposits_open: bool,
    /// Whether deposits stay open while Phoenix trades the market after hours (stock markets
    /// on weekends and holidays, when the book is thin and the price is held near the last
    /// close) or reopens it. Withdrawals are always open.
    pub after_hours_deposits: bool,
    pub bump: u8,
    /// Part of the authority's address, picked by the admin at `create_vault` (little-endian
    /// u64), so nobody can claim the vault's Phoenix trader before the vault exists.
    pub authority_nonce: [u8; 8],
    pub authority_bump: u8,
}

impl BasisVault {
    pub fn authority_seeds<'a>(&'a self, basis_vault: &'a Pubkey) -> [&'a [u8]; 4] {
        [
            AUTHORITY_SEED,
            basis_vault.as_ref(),
            &self.authority_nonce,
            std::slice::from_ref(&self.authority_bump),
        ]
    }
}

pub fn authority_address(basis_vault: &Pubkey, nonce: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[AUTHORITY_SEED, basis_vault.as_ref(), &nonce.to_le_bytes()],
        &crate::ID,
    )
}
