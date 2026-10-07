//! CPI into Phoenix perpetuals (Ellipsis Labs), built with the official `phoenix-rise-ix`
//! instruction builders. The vault's authority PDA is the Phoenix trader and signs every
//! trader action through `invoke_signed`.
//!
//! Phoenix keeps two dynamic account groups (the global trader index and the active trader
//! buffer). Their addresses change as Phoenix grows, so callers pass them in
//! `remaining_accounts` and say how many of each there are.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::{get_return_data, invoke_signed};
use bytemuck::Pod;
use phoenix_rise_ix::deposit_funds::{create_deposit_funds_ix, DepositFundsParams};
use phoenix_rise_ix::ember_deposit::{create_ember_deposit_ix, EmberDepositParams};
use phoenix_rise_ix::ember_withdraw::{create_ember_withdraw_ix, EmberWithdrawParams};
use phoenix_rise_ix::hawkeye::{
    create_hawkeye_view_margin_for_asset_ix, create_hawkeye_view_margin_ix, decode_hawkeye_return,
    HawkeyeTraderViewAccounts, ViewAssetReturn, ViewMarginReturn, HAWKEYE_PROGRAM_ID,
};
use phoenix_rise_ix::market_order::{create_place_market_order_ix, MarketOrderParams};
use phoenix_rise_ix::native_sol::{
    create_realloc_trader_ix, create_sync_native_ix, create_withdraw_native_sol_ix,
    SyncNativeParams, WithdrawNativeSolAction, WithdrawNativeSolParams,
};
use phoenix_rise_ix::register_trader::{create_register_trader_ix, RegisterTraderParams};
use phoenix_rise_ix::types::Side;
use phoenix_rise_ix::withdraw_funds::{create_withdraw_funds_ix, WithdrawFundsParams};

use crate::errors::VaultError;
use crate::nav::MarketState;

pub const PHOENIX_PROGRAM_ID: Pubkey = phoenix_rise_ix::constants::PROD_PHOENIX_PROGRAM_ID;
pub const PHOENIX_LOG_AUTHORITY: Pubkey = phoenix_rise_ix::constants::PROD_PHOENIX_LOG_AUTHORITY;
pub const PHOENIX_GLOBAL_CONFIG: Pubkey =
    phoenix_rise_ix::constants::PROD_PHOENIX_GLOBAL_CONFIGURATION;
pub const EMBER_PROGRAM_ID: Pubkey = phoenix_rise_ix::constants::EMBER_PROGRAM_ID;
pub const HAWKEYE_ID: Pubkey = HAWKEYE_PROGRAM_ID;
const TRADER_SEED: &[u8] = b"trader";

/// The Phoenix trader account of `authority` (the default cross-margin account).
pub fn trader_account_address(authority: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[TRADER_SEED, authority.as_ref(), &[0, 0]],
        &PHOENIX_PROGRAM_ID,
    )
    .0
}

// Phoenix's perp asset map (`phoenix-rise-accounts`, `perp_asset_map`): a 48-byte header that
// starts with the account's discriminant, then 1,024 slots of 1,584 bytes, one per market. Offsets
// are into a slot. A Phoenix upgrade that changes the layout changes the size, and the reads
// fail instead of returning garbage.
const ASSET_MAP_DISCRIMINANT: u64 = 2_768_851_190_529_865_358; // sha256("account:perp_asset_map")
const ASSET_MAP_HEADER: usize = 48;
const ASSET_MAP_SLOTS_USED: usize = 32;
const ASSET_SLOT: usize = 1_584;
const ASSET_MAP_LEN: usize = ASSET_MAP_HEADER + 1_024 * ASSET_SLOT;
const SLOT_MARKET_ACCOUNT: usize = 920;
const SLOT_TICK_SIZE: usize = 952;
const SLOT_ASSET_ID_LOW: usize = 960;
const SLOT_BASE_LOT_DECIMALS: usize = 962;
const SLOT_ASSET_ID_HIGH: usize = 964;
const SLOT_OPEN_INTEREST: usize = 1_440;
const SLOT_TOMBSTONED: usize = 1_466;
const SLOT_FLAGS: usize = 1_472;
const SLOT_VARIANT: usize = 1_583;
/// Stock and commodity markets: outside the session, and reopening after it.
const FLAG_REOPEN: u8 = 1 << 1;
const FLAG_AFTER_HOURS: u8 = 1 << 2;

/// The live slot of market `asset_id` in Phoenix's perp asset map.
fn market_slot(data: &[u8], asset_id: u32) -> Result<usize> {
    require!(
        data.len() == ASSET_MAP_LEN
            && u64::from_le_bytes(data[..8].try_into().unwrap()) == ASSET_MAP_DISCRIMINANT,
        VaultError::InvalidPhoenixAccount
    );
    let read_u16 = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
    let slots = u32::from_le_bytes(
        data[ASSET_MAP_SLOTS_USED..ASSET_MAP_SLOTS_USED + 4]
            .try_into()
            .unwrap(),
    ) as usize;
    require!(slots <= 1_024, VaultError::InvalidPhoenixAccount);
    (0..slots)
        .map(|slot| ASSET_MAP_HEADER + slot * ASSET_SLOT)
        .find(|&at| {
            let id = read_u16(at + SLOT_ASSET_ID_LOW) as u32
                | (read_u16(at + SLOT_ASSET_ID_HIGH) as u32) << 16;
            id == asset_id && data[at + SLOT_TOMBSTONED] == 0 && data[at + SLOT_VARIANT] == 0
        })
        .ok_or_else(|| VaultError::InvalidPhoenixAccount.into())
}

/// The open interest and session state of market `asset_id`, read from Phoenix's perp asset
/// map (the account the vault's config names, owned by Phoenix).
pub fn market_state(perp_asset_map: &AccountInfo, asset_id: u32) -> Result<MarketState> {
    let data = perp_asset_map.try_borrow_data()?;
    let at = market_slot(&data, asset_id)?;
    Ok(MarketState {
        open_interest_lots: u64::from_le_bytes(
            data[at + SLOT_OPEN_INTEREST..at + SLOT_OPEN_INTEREST + 8]
                .try_into()
                .unwrap(),
        ),
        after_hours: data[at + SLOT_FLAGS] & (FLAG_REOPEN | FLAG_AFTER_HOURS) != 0,
    })
}

/// How Phoenix lists market `asset_id`: its order book, tick size (quote lots per base lot
/// per tick) and base lot decimals.
pub fn market_listing(perp_asset_map: &AccountInfo, asset_id: u32) -> Result<(Pubkey, u64, u8)> {
    let data = perp_asset_map.try_borrow_data()?;
    let at = market_slot(&data, asset_id)?;
    let orderbook =
        Pubkey::try_from(&data[at + SLOT_MARKET_ACCOUNT..at + SLOT_MARKET_ACCOUNT + 32])
            .map_err(|_| VaultError::InvalidPhoenixAccount)?;
    let tick_size = u64::from_le_bytes(
        data[at + SLOT_TICK_SIZE..at + SLOT_TICK_SIZE + 8]
            .try_into()
            .unwrap(),
    );
    Ok((orderbook, tick_size, data[at + SLOT_BASE_LOT_DECIMALS]))
}

/// The spline collection of a Phoenix market.
pub fn spline_collection_address(orderbook: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"spline", orderbook.as_ref()], &PHOENIX_PROGRAM_ID).0
}

/// Phoenix's dynamic account groups, taken from the front of `remaining_accounts`.
pub struct TraderIndex<'a, 'info> {
    pub global_trader_index: &'a [AccountInfo<'info>],
    pub active_trader_buffer: &'a [AccountInfo<'info>],
}

impl<'a, 'info> TraderIndex<'a, 'info> {
    /// Phoenix checks these accounts itself in every trader instruction; Hawkeye only reads
    /// them, so the vault makes sure they are at least Phoenix's own accounts.
    pub fn split(remaining: &'a [AccountInfo<'info>], gti: u8, atb: u8) -> Result<Self> {
        let (gti, atb) = (gti as usize, atb as usize);
        require!(
            gti > 0 && atb > 0 && remaining.len() >= gti + atb,
            VaultError::MissingPhoenixAccounts
        );
        let accounts = &remaining[..gti + atb];
        require!(
            accounts.iter().all(|a| *a.owner == PHOENIX_PROGRAM_ID),
            VaultError::InvalidPhoenixAccount
        );
        Ok(Self {
            global_trader_index: &accounts[..gti],
            active_trader_buffer: &accounts[gti..],
        })
    }

    fn keys(&self) -> (Vec<Pubkey>, Vec<Pubkey>) {
        (
            self.global_trader_index.iter().map(|a| a.key()).collect(),
            self.active_trader_buffer.iter().map(|a| a.key()).collect(),
        )
    }
}

fn ix_err<E>(_: E) -> Error {
    VaultError::PhoenixInstruction.into()
}

/// Invokes a Phoenix-built instruction, handing over every account the caller holds; the
/// runtime picks the ones the instruction references.
fn invoke(
    ix: phoenix_rise_ix::types::Instruction,
    accounts: &[AccountInfo],
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let ix: anchor_lang::solana_program::instruction::Instruction = ix.into();
    invoke_signed(&ix, accounts, signer_seeds)?;
    Ok(())
}

/// Registers `authority`'s default cross-margin trader account. The authority does not sign:
/// Phoenix only needs its key. Phoenix's onboarding then grants the trader its capabilities.
#[allow(clippy::too_many_arguments)]
pub fn register_trader<'info>(
    phoenix_program: &AccountInfo<'info>,
    log_authority: &AccountInfo<'info>,
    global_config: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    trader_account: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    max_positions: u64,
) -> Result<()> {
    let params = RegisterTraderParams::builder()
        .payer(payer.key())
        .trader(authority.key())
        .trader_account(trader_account.key())
        .max_positions(max_positions)
        .trader_pda_index(0)
        .subaccount_index(0)
        .build()
        .map_err(ix_err)?;
    let ix = create_register_trader_ix(params).map_err(ix_err)?;
    let infos = [
        phoenix_program.clone(),
        log_authority.clone(),
        global_config.clone(),
        payer.clone(),
        authority.clone(),
        trader_account.clone(),
        system_program.clone(),
    ];
    invoke(ix, &infos, &[])
}

/// Ember swaps USDC and Phoenix's collateral token 1:1.
pub struct Ember<'a, 'info> {
    pub program: &'a AccountInfo<'info>,
    pub state: &'a AccountInfo<'info>,
    pub vault: &'a AccountInfo<'info>,
    pub usdc_mint: &'a AccountInfo<'info>,
    pub phoenix_mint: &'a AccountInfo<'info>,
    /// The vault authority's USDC and Phoenix-token accounts.
    pub usdc_account: &'a AccountInfo<'info>,
    pub phoenix_account: &'a AccountInfo<'info>,
}

/// The vault's Phoenix trader and every account its actions touch.
pub struct PhoenixTrader<'a, 'info> {
    pub phoenix_program: &'a AccountInfo<'info>,
    pub log_authority: &'a AccountInfo<'info>,
    pub global_config: &'a AccountInfo<'info>,
    /// The vault authority PDA: the trader.
    pub authority: &'a AccountInfo<'info>,
    pub trader_account: &'a AccountInfo<'info>,
    pub perp_asset_map: &'a AccountInfo<'info>,
    pub orderbook: &'a AccountInfo<'info>,
    pub spline_collection: &'a AccountInfo<'info>,
    pub withdraw_queue: &'a AccountInfo<'info>,
    pub global_vault: &'a AccountInfo<'info>,
    pub hawkeye_program: &'a AccountInfo<'info>,
    pub token_program: &'a AccountInfo<'info>,
    pub system_program: &'a AccountInfo<'info>,
    pub index: TraderIndex<'a, 'info>,
    pub asset_id: u32,
    /// Every account any of the trader's calls may need, gathered once: Solana's allocator
    /// never frees, and a vault operation makes a dozen calls.
    pub accounts: Vec<AccountInfo<'info>>,
}

impl<'info> PhoenixTrader<'_, 'info> {
    /// Posts `lamports` of native SOL from `from` as collateral: Phoenix keeps the lamports
    /// in the trader account itself and `sync_native` counts them. `payer` funds any rent the
    /// trader account needs to grow; `from_seeds` signs for `from` when it is a PDA.
    pub fn deposit_native_sol(
        &self,
        payer: &AccountInfo<'info>,
        from: &AccountInfo<'info>,
        lamports: u64,
        from_seeds: &[&[&[u8]]],
    ) -> Result<()> {
        let realloc =
            create_realloc_trader_ix(payer.key(), self.authority.key(), self.trader_account.key());
        invoke(realloc, &self.accounts, &[])?;
        let transfer = anchor_lang::system_program::Transfer {
            from: from.clone(),
            to: self.trader_account.clone(),
        };
        anchor_lang::system_program::transfer(
            CpiContext::new_with_signer(anchor_lang::system_program::ID, transfer, from_seeds),
            lamports,
        )?;
        self.sync_native()
    }

    /// Counts every lamport above the trader account's rent floor as collateral.
    pub fn sync_native(&self) -> Result<()> {
        let (gti, atb) = self.index.keys();
        let sync = SyncNativeParams::builder()
            .trader_account(self.trader_account.key())
            .global_trader_index(gti)
            .active_trader_buffer(atb)
            .build()
            .map_err(ix_err)?;
        invoke(
            create_sync_native_ix(sync).map_err(ix_err)?,
            &self.accounts,
            &[],
        )
    }

    /// Withdraws `lamports` of native SOL straight to `destination`: first any lamports
    /// Phoenix does not count as collateral (above its per-trader cap), then collateral.
    pub fn withdraw_native_sol(
        &self,
        destination: &AccountInfo<'info>,
        lamports: u64,
        signer_seeds: &[&[&[u8]]],
    ) -> Result<()> {
        if lamports == 0 {
            return Ok(());
        }
        let (gti, atb) = self.index.keys();
        let params = WithdrawNativeSolParams::builder()
            .trader(self.authority.key())
            .trader_account(self.trader_account.key())
            .perp_asset_map(self.perp_asset_map.key())
            .destination(destination.key())
            .withdraw_queue(self.withdraw_queue.key())
            .global_trader_index(gti)
            .active_trader_buffer(atb)
            .action(WithdrawNativeSolAction::WithExcess { amount: lamports })
            .build()
            .map_err(ix_err)?;
        let ix = create_withdraw_native_sol_ix(params).map_err(ix_err)?;
        invoke(ix, &self.accounts, signer_seeds)
    }

    /// Turns `amount` USDC in the vault's USDC account into Phoenix collateral.
    pub fn deposit_usdc(
        &self,
        ember: &Ember<'_, 'info>,
        amount: u64,
        seeds: &[&[&[u8]]],
    ) -> Result<()> {
        if amount == 0 {
            return Ok(());
        }
        let params = EmberDepositParams::builder()
            .trader(self.authority.key())
            .usdc_mint(ember.usdc_mint.key())
            .canonical_mint(ember.phoenix_mint.key())
            .trader_usdc_account(ember.usdc_account.key())
            .trader_phoenix_account(ember.phoenix_account.key())
            .amount(amount)
            .build()
            .map_err(ix_err)?;
        invoke(
            create_ember_deposit_ix(params).map_err(ix_err)?,
            &self.accounts,
            seeds,
        )?;

        let (gti, atb) = self.index.keys();
        let params = DepositFundsParams::builder()
            .trader(self.authority.key())
            .trader_account(self.trader_account.key())
            .canonical_mint(ember.phoenix_mint.key())
            .global_vault(self.global_vault.key())
            .trader_token_account(ember.phoenix_account.key())
            .global_trader_index(gti)
            .active_trader_buffer(atb)
            .amount(amount)
            .build()
            .map_err(ix_err)?;
        invoke(
            create_deposit_funds_ix(params).map_err(ix_err)?,
            &self.accounts,
            seeds,
        )
    }

    /// Takes `amount` of Phoenix collateral out as USDC into the vault's USDC account.
    pub fn withdraw_usdc(
        &self,
        ember: &Ember<'_, 'info>,
        amount: u64,
        seeds: &[&[&[u8]]],
    ) -> Result<()> {
        if amount == 0 {
            return Ok(());
        }
        let (gti, atb) = self.index.keys();
        let params = WithdrawFundsParams::builder()
            .trader(self.authority.key())
            .trader_account(self.trader_account.key())
            .perp_asset_map(self.perp_asset_map.key())
            .global_vault(self.global_vault.key())
            .trader_token_account(ember.phoenix_account.key())
            .global_trader_index(gti)
            .active_trader_buffer(atb)
            .withdraw_queue(self.withdraw_queue.key())
            .amount(amount)
            .build()
            .map_err(ix_err)?;
        invoke(
            create_withdraw_funds_ix(params).map_err(ix_err)?,
            &self.accounts,
            seeds,
        )?;

        let params = EmberWithdrawParams::builder()
            .trader(self.authority.key())
            .usdc_mint(ember.usdc_mint.key())
            .canonical_mint(ember.phoenix_mint.key())
            .trader_usdc_account(ember.usdc_account.key())
            .trader_phoenix_account(ember.phoenix_account.key())
            .amount(Some(amount))
            .build()
            .map_err(ix_err)?;
        invoke(
            create_ember_withdraw_ix(params).map_err(ix_err)?,
            &self.accounts,
            seeds,
        )
    }

    /// An immediate-or-cancel market order on the vault's market that must fill in full at
    /// `limit_price_ticks` or better.
    pub fn market_order(
        &self,
        sell: bool,
        num_base_lots: u64,
        limit_price_ticks: u64,
        signer_seeds: &[&[&[u8]]],
    ) -> Result<()> {
        let (gti, atb) = self.index.keys();
        let params = MarketOrderParams::builder()
            .trader(self.authority.key())
            .trader_account(self.trader_account.key())
            .perp_asset_map(self.perp_asset_map.key())
            .orderbook(self.orderbook.key())
            .spline_collection(self.spline_collection.key())
            .global_trader_index(gti)
            .active_trader_buffer(atb)
            .side(if sell { Side::Ask } else { Side::Bid })
            .price_in_ticks(limit_price_ticks)
            .num_base_lots(num_base_lots)
            .min_base_lots_to_fill(num_base_lots)
            .build()
            .map_err(ix_err)?;
        let ix = create_place_market_order_ix(params).map_err(ix_err)?;
        invoke(ix, &self.accounts, signer_seeds)
    }

    fn view_accounts(&self) -> HawkeyeTraderViewAccounts {
        let (gti, atb) = self.index.keys();
        HawkeyeTraderViewAccounts {
            phoenix_program_id: PHOENIX_PROGRAM_ID,
            global_config: self.global_config.key(),
            global_trader_index: gti,
            active_trader_buffer: atb,
            perp_asset_map: self.perp_asset_map.key(),
            trader: self.trader_account.key(),
        }
    }

    fn view<T: Pod + Copy>(
        &self,
        ix: phoenix_rise_ix::types::Instruction,
        kind: &'static str,
    ) -> Result<T> {
        invoke(ix, &self.accounts, &[])?;
        let (program, data) = get_return_data().ok_or(VaultError::PhoenixInstruction)?;
        require_keys_eq!(program, HAWKEYE_ID, VaultError::PhoenixInstruction);
        decode_hawkeye_return::<T>(&data, kind).map_err(ix_err)
    }

    /// Phoenix's own margin view of the trader (Hawkeye): collateral, unrealized PnL and
    /// unsettled funding, in quote lots.
    pub fn view_margin(&self) -> Result<ViewMarginReturn> {
        self.view(
            create_hawkeye_view_margin_ix(self.view_accounts()),
            "view_margin",
        )
    }

    /// Phoenix's view of the vault's market: its mark price and the vault's position.
    pub fn view_asset(&self) -> Result<ViewAssetReturn> {
        let ix = create_hawkeye_view_margin_for_asset_ix(self.view_accounts(), self.asset_id);
        self.view(ix, "view_margin_for_asset")
    }
}
