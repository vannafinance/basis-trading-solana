//! Every event that changes the vault's value or supply carries both after it, so the share
//! price history can be read from the logs alone.

use anchor_lang::prelude::*;

#[event]
pub struct VaultCreated {
    pub vault: Pubkey,
    pub share_mint: Pubkey,
    pub trader_account: Pubkey,
}

#[event]
pub struct LegRetired {
    pub vault: Pubkey,
    pub leg: Pubkey,
    pub index: u8,
}

#[event]
pub struct LegAdded {
    pub vault: Pubkey,
    pub leg: Pubkey,
    pub index: u8,
    pub asset_mint: Pubkey,
    pub asset_id: u32,
}

#[event]
pub struct Deposited {
    pub vault: Pubkey,
    pub user: Pubkey,
    pub usdc_amount: u64,
    pub shares: u64,
    /// The deposit's value to the vault after its fee (USD, 6 dp).
    pub value_usd: u64,
    pub nav_before_usd: u64,
    pub nav_usd: u64,
    pub supply: u64,
}

#[event]
pub struct Withdrawn {
    pub vault: Pubkey,
    pub user: Pubkey,
    pub shares: u64,
    /// The shares' value before the fee (USD, 6 dp).
    pub payout_usd: u64,
    pub fee_usd: u64,
    /// The USDC the user received.
    pub usdc_out: u64,
    /// The request this fulfilled; none for an instant withdrawal.
    pub request_id: Option<u64>,
    pub nav_usd: u64,
    pub supply: u64,
}

#[event]
pub struct WithdrawRequested {
    pub vault: Pubkey,
    pub user: Pubkey,
    pub request: Pubkey,
    pub request_id: u64,
    pub shares: u64,
}

#[event]
pub struct WithdrawCancelled {
    pub vault: Pubkey,
    pub user: Pubkey,
    pub request_id: u64,
    /// Shares back to the user.
    pub shares: u64,
    /// Shares burned as the exit fee (none for a request the keeper returned).
    pub fee_shares: u64,
}

#[event]
pub struct WithdrawReturned {
    pub vault: Pubkey,
    pub user: Pubkey,
    pub request_id: u64,
    pub shares: u64,
    pub by: Pubkey,
}

#[event]
pub struct Deployed {
    pub vault: Pubkey,
    pub leg_index: u8,
    pub usdc_spent: u64,
    pub asset_bought: u64,
    /// The leg's position after it, in base lots; negative is short.
    pub position_lots: i64,
    pub nav_before_usd: u64,
    pub nav_usd: u64,
    pub supply: u64,
}

#[event]
pub struct Unwound {
    pub vault: Pubkey,
    pub leg_index: u8,
    pub asset_sold: u64,
    pub usdc_received: u64,
    pub position_lots: i64,
    pub nav_before_usd: u64,
    pub nav_usd: u64,
    pub supply: u64,
}

#[event]
pub struct Rehedged {
    pub vault: Pubkey,
    pub leg_index: u8,
    pub position_lots: i64,
    pub nav_before_usd: u64,
    pub nav_usd: u64,
    pub supply: u64,
}

#[event]
pub struct MarginMoved {
    pub vault: Pubkey,
    /// True when USDC moved from the buffer into Phoenix.
    pub to_phoenix: bool,
    pub amount: u64,
    pub nav_usd: u64,
    pub supply: u64,
}

#[event]
pub struct DepositsSet {
    pub vault: Pubkey,
    pub open: bool,
    /// The keeper or the admin.
    pub by: Pubkey,
}

#[event]
pub struct Swapped {
    pub vault: Pubkey,
    pub leg_index: u8,
    /// True when the vault sold USDC for the asset.
    pub sold_usdc: bool,
    pub spent: u64,
    pub received: u64,
}
