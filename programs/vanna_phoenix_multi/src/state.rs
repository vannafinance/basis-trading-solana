use crate::constants::*;
use crate::errors::VaultError;
use anchor_lang::prelude::*;

/// Protocol-wide roles and the Phoenix accounts the vault uses.
#[account]
#[derive(InitSpace)]
pub struct Config {
    pub admin: Pubkey,
    /// The bot that allocates the vault over its legs: it deploys, unwinds, moves margin and
    /// re-hedges, within the limits the program enforces.
    pub keeper: Pubkey,
    pub usdc_mint: Pubkey,
    /// Phoenix's collateral token (PhUSD), minted 1:1 for USDC by Ember.
    pub phoenix_mint: Pubkey,
    pub perp_asset_map: Pubkey,
    pub withdraw_queue: Pubkey,
    /// Phoenix's token vault for `phoenix_mint`.
    pub global_vault: Pubkey,
    /// The swap venue the keeper buys and sells the legs' assets through: Jupiter v6, or the
    /// default key (swaps off: no deploys or unwinds; deposits and withdrawals still work).
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

/// The multi-strategy vault. Users deposit and withdraw USDC and hold its shares; the keeper
/// spreads the USDC over the vault's legs, each an asset held with an equal short on Phoenix,
/// so the value stays flat in USD while it earns the funding longs pay shorts. Every leg's
/// short sits in one Phoenix cross-margin trader account.
///
/// The share price is the NAV over the supply, priced exactly as in the single-asset vault:
/// `NAV = idle USDC + Σ legs (asset held × mark × multiplier) + Phoenix equity`.
#[account]
#[derive(InitSpace)]
pub struct Vault {
    /// Picked by the admin at `create_vault`; part of the vault's address.
    pub id: u16,
    pub share_mint: Pubkey,
    pub authority: Pubkey,
    /// The authority's USDC account: deposits land here, withdrawals leave from here, and the
    /// keeper deploys from it (the buffer).
    pub usdc_account: Pubkey,
    pub phoenix_account: Pubkey,
    /// The authority's share account, holding the shares of pending withdrawal requests.
    pub escrow_shares: Pubkey,
    pub trader_account: Pubkey,
    /// Legs are numbered 0..leg_count; every deposit and withdrawal passes them all.
    pub leg_count: u8,
    pub next_request_id: u64,
    /// Shares in live withdrawal requests. Only these are reserved from the buffer: shares
    /// someone sends straight to the escrow are not a request.
    pub pending_shares: u64,
    /// Max NAV (USD, 6 dp) deposits may take the vault to.
    pub max_tvl_usd: u64,
    /// Max USDC one deposit may bring.
    pub max_deposit_usd: u64,
    /// Fees on deposits and withdrawals, in bps. They stay in the vault: they pay for the
    /// keeper's trades that money moving in or out causes.
    pub entry_fee_bps: u16,
    pub exit_fee_bps: u16,
    /// Idle USDC the keeper must leave per dollar of NAV, on top of what pending withdrawal
    /// requests are worth.
    pub buffer_bps: u16,
    /// Phoenix effective collateral the vault keeps per dollar of total short, in bps.
    pub margin_bps: u16,
    /// Price band around Phoenix's mark for the vault's orders and swaps.
    pub slippage_bps: u16,
    /// Max value the keeper may deploy and unwind per `TURNOVER_WINDOW_SECONDS` (USD, 6 dp):
    /// bounds what churn could cost the vault.
    pub max_turnover_usd: u64,
    pub turnover_window_start: i64,
    pub turnover_window_usd: u64,
    pub deposits_open: bool,
    pub bump: u8,
    /// Part of the authority's address, picked by the admin at `create_vault` (little-endian
    /// u64), so nobody can claim the vault's Phoenix trader before the vault exists.
    pub authority_nonce: [u8; 8],
    pub authority_bump: u8,
}

impl Vault {
    pub fn authority_seeds<'a>(&'a self, vault: &'a Pubkey) -> [&'a [u8]; 4] {
        [
            AUTHORITY_SEED,
            vault.as_ref(),
            &self.authority_nonce,
            std::slice::from_ref(&self.authority_bump),
        ]
    }
}

pub fn vault_address(id: u16) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[VAULT_SEED, &id.to_le_bytes()], &crate::ID)
}

pub fn authority_address(vault: &Pubkey, nonce: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[AUTHORITY_SEED, vault.as_ref(), &nonce.to_le_bytes()],
        &crate::ID,
    )
}

pub fn leg_address(vault: &Pubkey, index: u8) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[LEG_SEED, vault.as_ref(), &[index]], &crate::ID)
}

pub fn request_address(vault: &Pubkey, id: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[REQUEST_SEED, vault.as_ref(), &id.to_le_bytes()], &crate::ID)
}

/// How a leg holds the asset it hedges.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum LegKind {
    /// SOL: Phoenix takes native SOL as collateral, so the SOL sits in the vault's trader
    /// account and counts as margin for every leg (80% of it, up to Phoenix's per-trader cap).
    NativeSol,
    /// Any other asset (ETH, BTC, tokenized stocks): held as tokens in the authority's token
    /// account; its short's margin is the USDC the keeper posts on Phoenix.
    Spot,
}

/// One market the vault can hold: an asset and its Phoenix perp.
#[account]
#[derive(InitSpace)]
pub struct Leg {
    pub vault: Pubkey,
    pub index: u8,
    pub kind: LegKind,
    pub asset_mint: Pubkey,
    pub asset_decimals: u8,
    /// SPL Token, or Token-2022 for tokenized stocks.
    pub asset_token_program: Pubkey,
    /// The authority's asset account; unset for the SOL leg (its SOL is in the trader account).
    pub asset_account: Pubkey,
    /// The Phoenix market.
    pub asset_id: u32,
    pub orderbook: Pubkey,
    pub spline_collection: Pubkey,
    pub base_lot_decimals: u8,
    pub tick_size: u64,
    /// Whether the keeper may deploy into the leg; it may always unwind and re-hedge it.
    pub active: bool,
    /// A flat leg the admin took out of the vault (`retire_leg`): it holds nothing, takes one
    /// account in the leg list and no Phoenix call, so its market can no longer stop the vault.
    pub retired: bool,
    /// Max short notional (USD, 6 dp).
    pub max_notional_usd: u64,
    /// Max asset held, in its smallest units (the SOL leg stays under Phoenix's native SOL
    /// cap per trader: 500 SOL on mainnet).
    pub max_asset_units: u64,
    /// Max share of the market's open interest the leg's short may be, in bps.
    pub max_oi_share_bps: u16,
    /// Max share of the vault's NAV the leg's asset may be worth, in bps; 10,000 is no cap.
    pub max_nav_share_bps: u16,
    /// Whether the keeper may deploy while Phoenix trades the market after hours (stocks).
    pub after_hours_trading: bool,
    pub bump: u8,
}

impl Leg {
    /// The accounts a leg takes in an instruction's leg list: the leg, then its holding (the
    /// asset account; none for the SOL leg, whose SOL is in the trader account), then its mint
    /// when it is a Token-2022 token (its multiplier).
    pub fn account_count(&self) -> usize {
        if self.retired {
            return 1;
        }
        match self.kind {
            LegKind::NativeSol => 1,
            LegKind::Spot if self.asset_token_program == anchor_spl::token_2022::ID => 3,
            LegKind::Spot => 2,
        }
    }
}

/// A withdrawal waiting for the keeper to raise its USDC. Its shares sit in the vault's escrow
/// and keep earning (or losing) with everyone's until it is fulfilled at the NAV then; the
/// user can cancel it until then.
#[account]
#[derive(InitSpace)]
pub struct WithdrawRequest {
    pub vault: Pubkey,
    pub user: Pubkey,
    pub id: u64,
    pub shares: u64,
    pub created_at: i64,
    pub bump: u8,
    /// The keeper gave up on paying it (`return_withdraw`): its shares no longer reserve the
    /// buffer, and its user can only take them back (`cancel_withdraw`).
    pub returned: bool,
}
