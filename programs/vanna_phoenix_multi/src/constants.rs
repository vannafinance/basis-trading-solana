use anchor_lang::prelude::*;

pub const CONFIG_SEED: &[u8] = b"config";
pub const VAULT_SEED: &[u8] = b"vault";
/// Data-less PDA of the vault: the Phoenix trader authority, owner of the vault's token
/// accounts and the share mint's mint authority.
pub const AUTHORITY_SEED: &[u8] = b"authority";
pub const SHARE_MINT_SEED: &[u8] = b"share_mint";
pub const LEG_SEED: &[u8] = b"leg";
pub const REQUEST_SEED: &[u8] = b"withdraw_request";

/// Shares have 6 decimals, so the first deposit mints one share per dollar.
pub const SHARE_DECIMALS: u8 = 6;

/// Values are in USD with 6 decimals: the same unit as Phoenix quote lots and USDC atoms.
pub const USD_DECIMALS: u32 = 6;

pub const BPS_SCALE: u128 = 10_000;

/// Virtual shares and value added to the share price, so the first depositor gets one share
/// per dollar and nobody can inflate the price to round a later depositor down to zero.
pub const VIRTUAL_SHARES: u128 = 1_000_000;
pub const VIRTUAL_USD: u128 = 1_000_000;

/// The most legs one vault may ever add, retired ones included (a retired leg takes one account
/// and no Phoenix call).
pub const MAX_LEGS: u8 = 16;

/// The most live (not retired) legs. Every deposit and withdrawal reads every live leg, so this
/// bounds their accounts and compute: with eight a keeper deploy names 47 accounts with the
/// tests' 10-account mock route (a direct Jupiter route is about 15 more: 62 of Solana's 64;
/// the keeper asks Jupiter for routes with `maxAccounts` to fit) and uses about 650k compute
/// units.
pub const MAX_LIVE_LEGS: usize = 8;

/// The most a leg may hold, at its mark, and still be retired (USD, 6 dp): dust someone sends
/// cannot keep a leg alive (a griefer would have to give the vault this much each time). It is
/// left out of the NAV from then on.
pub const RETIRE_DUST_USD: u128 = 100_000_000;

/// Position slots reserved in the vault's Phoenix trader account: one per live leg, with room.
pub const MAX_PHOENIX_POSITIONS: u64 = 16;

pub const NATIVE_SOL_DECIMALS: u8 = 9;

pub const NATIVE_MINT: Pubkey = pubkey!("So11111111111111111111111111111111111111112");

/// Fixed-point scale of a token's UI multiplier: tokenized stocks (xStocks) reinvest
/// dividends by raising it, so one token is worth `multiplier` shares.
pub const MULTIPLIER_SCALE: u128 = 1_000_000_000_000;

/// A change of a token's multiplier above this (a special dividend, a split), from a day before
/// it takes effect until three days after: until Phoenix's market and the token have caught
/// up, the vault prices the token for those who stay. Deposits value it at the higher of the
/// two multipliers, withdrawals at the lower, and the keeper may not deploy into it.
pub const MULTIPLIER_JUMP_BPS: u128 = 25;
pub const MULTIPLIER_WINDOW_BEFORE: i64 = 86_400;
pub const MULTIPLIER_WINDOW_AFTER: i64 = 3 * 86_400;

/// The only swap venue the vault authority will sign for: Jupiter v6.
pub const JUPITER_PROGRAM_ID: Pubkey = pubkey!("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4");

/// The tightest price band the vault may have: at zero, no order could fill.
pub const MIN_SLIPPAGE_BPS: u16 = 10;

/// The widest: every swap and order is the keeper's, and what a stolen keeper key could cost is
/// about twice the band (a swap and an order per dollar) on its daily turnover.
pub const MAX_SLIPPAGE_BPS: u16 = 200;

/// The most a deposit or withdrawal fee may be. Both stay in the vault: they cover the trades
/// the keeper makes for the money moving in or out, which every holder otherwise pays.
pub const MAX_FEE_BPS: u16 = 100;

/// What the keeper may post to Phoenix out of the reserve beyond what the vault owes there, per
/// dollar of short (bps): the fees of closing the shorts, which Phoenix settles in USDC before
/// it lets native SOL go.
pub const PHOENIX_COST_CUSHION_BPS: u128 = 50;

/// The keeper's trading volume is limited per this many seconds.
pub const TURNOVER_WINDOW_SECONDS: i64 = 86_400;
