use anchor_lang::prelude::*;

#[event]
pub struct VaultCreated {
    pub basis_vault: Pubkey,
    pub asset_mint: Pubkey,
    pub share_mint: Pubkey,
    pub trader_account: Pubkey,
    pub asset_id: u32,
}

#[event]
pub struct Deposited {
    pub basis_vault: Pubkey,
    pub user: Pubkey,
    pub asset_amount: u64,
    pub usdc_amount: u64,
    pub shares: u64,
    /// Value the deposit added after its own trade (USD, 6 dp).
    pub value_usd: u64,
    pub nav_before_usd: u64,
    /// The vault's position after the deposit, in base lots; negative is short.
    pub position_lots: i64,
}

#[event]
pub struct Withdrawn {
    pub basis_vault: Pubkey,
    pub user: Pubkey,
    pub shares: u64,
    pub payout_usd: u64,
    pub asset_out: u64,
    pub usdc_out: u64,
    pub position_lots: i64,
}

#[event]
pub struct Rebalanced {
    pub basis_vault: Pubkey,
    /// True when funding USDC was reinvested into the asset.
    pub buy_asset: bool,
    pub asset_amount: u64,
    pub usdc_amount: u64,
    pub nav_before_usd: u64,
    pub nav_after_usd: u64,
    pub position_lots: i64,
}

#[event]
pub struct DepositsSet {
    pub basis_vault: Pubkey,
    pub open: bool,
    /// The keeper or the admin.
    pub by: Pubkey,
}

#[event]
pub struct Swapped {
    pub basis_vault: Pubkey,
    /// True when the vault sold USDC for the asset.
    pub sold_usdc: bool,
    pub spent: u64,
    pub received: u64,
}
