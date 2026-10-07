use anchor_lang::prelude::*;

pub const CONFIG_SEED: &[u8] = b"config";
pub const BASIS_VAULT_SEED: &[u8] = b"basis_vault";
/// Data-less PDA of a vault: the Phoenix trader authority, owner of the vault's token
/// accounts and the share mint's mint authority.
pub const AUTHORITY_SEED: &[u8] = b"authority";
pub const SHARE_MINT_SEED: &[u8] = b"share_mint";

/// Shares have 6 decimals, so the first deposit mints one share per dollar.
pub const SHARE_DECIMALS: u8 = 6;

/// Values are in USD with 6 decimals: the same unit as Phoenix quote lots and USDC atoms.
pub const USD_DECIMALS: u32 = 6;

pub const BPS_SCALE: u128 = 10_000;

/// Virtual shares and value added to the share price, so the first depositor gets one share
/// per dollar and nobody can inflate the price to round a later depositor down to zero.
pub const VIRTUAL_SHARES: u128 = 1_000_000;
pub const VIRTUAL_USD: u128 = 1_000_000;

/// Position slots reserved in the vault's Phoenix trader account. A vault trades one market.
pub const MAX_PHOENIX_POSITIONS: u64 = 2;

pub const NATIVE_SOL_DECIMALS: u8 = 9;

pub const NATIVE_MINT: Pubkey = pubkey!("So11111111111111111111111111111111111111112");

/// Fixed-point scale of a token's UI multiplier: tokenized stocks (xStocks) reinvest
/// dividends by raising it, so one token is worth `multiplier` shares.
pub const MULTIPLIER_SCALE: u128 = 1_000_000_000_000;

/// A change of a token's multiplier above this (a special dividend, a split) closes deposits
/// from a day before it takes effect until three days after: until Phoenix's market and the
/// vault's short have caught up, a deposit could be valued at the wrong number of shares.
pub const MULTIPLIER_JUMP_BPS: u128 = 25;
pub const MULTIPLIER_WINDOW_BEFORE: i64 = 86_400;
pub const MULTIPLIER_WINDOW_AFTER: i64 = 3 * 86_400;

/// The only swap venue the vault authority will sign for: Jupiter v6.
pub const JUPITER_PROGRAM_ID: Pubkey = pubkey!("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4");

/// The tightest price band a vault may have: at zero, no order could fill and nobody could
/// leave.
pub const MIN_SLIPPAGE_BPS: u16 = 10;

/// The keeper's rebalancing volume is limited per this many seconds.
pub const REBALANCE_WINDOW_SECONDS: i64 = 86_400;
