//! The accounts and steps every vault operation shares: reading the vault's value from
//! Phoenix, moving USDC in and out of Phoenix, and keeping the short equal to the asset held.

use crate::constants::*;
use crate::errors::VaultError;
use crate::events::Swapped;
use crate::instructions::common::{token_amount, transfer_checked, ui_multiplier};
use crate::math::Price;
use crate::nav::*;
use crate::phoenix::*;
use crate::state::*;
use crate::swap::*;
use anchor_lang::prelude::*;

#[derive(Accounts)]
pub struct VaultCore<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    /// A vault of this program: only `create_vault` makes one, at its asset's PDA, so the
    /// owner and type checks are enough (re-deriving the PDA here would overflow the stack).
    /// Writable for the keeper's daily rebalancing count.
    #[account(mut)]
    pub basis_vault: Box<Account<'info, BasisVault>>,
    /// CHECK: the vault's data-less authority PDA, the Phoenix trader.
    #[account(mut, address = basis_vault.authority)]
    pub authority: UncheckedAccount<'info>,
    /// CHECK: the vault's share mint, created by `create_vault`.
    #[account(mut, address = basis_vault.share_mint)]
    pub share_mint: UncheckedAccount<'info>,
    /// CHECK: USDC.
    #[account(address = config.usdc_mint)]
    pub usdc_mint: UncheckedAccount<'info>,
    /// CHECK: Phoenix's collateral token, minted and burned by Ember.
    #[account(mut, address = config.phoenix_mint)]
    pub phoenix_mint: UncheckedAccount<'info>,
    /// CHECK: the vault authority's USDC account.
    #[account(mut, address = basis_vault.usdc_account)]
    pub vault_usdc: UncheckedAccount<'info>,
    /// CHECK: the vault authority's Phoenix-token account.
    #[account(mut, address = basis_vault.phoenix_account)]
    pub vault_phoenix: UncheckedAccount<'info>,
    /// CHECK: the vault authority's asset account; spot vaults (ETH, BTC, stocks) only.
    #[account(mut, address = basis_vault.asset_account)]
    pub vault_asset: Option<UncheckedAccount<'info>>,
    /// CHECK: the asset's mint, the native mint for the SOL vault. A stock token's multiplier
    /// is read from it.
    #[account(address = basis_vault.asset_mint)]
    pub asset_mint: UncheckedAccount<'info>,
    /// CHECK: the asset's token program.
    #[account(address = basis_vault.asset_token_program)]
    pub asset_token_program: UncheckedAccount<'info>,
    /// CHECK: the vault's Phoenix trader account.
    #[account(mut, address = basis_vault.trader_account)]
    pub trader_account: UncheckedAccount<'info>,
    pub phoenix: PhoenixPrograms<'info>,
    /// CHECK: Phoenix's perp asset map.
    #[account(mut, address = config.perp_asset_map)]
    pub perp_asset_map: UncheckedAccount<'info>,
    /// CHECK: the vault market's orderbook.
    #[account(mut, address = basis_vault.orderbook)]
    pub orderbook: UncheckedAccount<'info>,
    /// CHECK: the vault market's spline collection.
    #[account(mut, address = basis_vault.spline_collection)]
    pub spline_collection: UncheckedAccount<'info>,
    /// CHECK: Phoenix's withdraw queue.
    #[account(mut, address = config.withdraw_queue)]
    pub withdraw_queue: UncheckedAccount<'info>,
    /// CHECK: Phoenix's vault for its collateral token.
    #[account(mut, address = config.global_vault)]
    pub global_vault: UncheckedAccount<'info>,
}

/// Phoenix's, Hawkeye's and Ember's programs and fixed accounts. A struct of its own: Anchor
/// checks it in its own stack frame, which `VaultCore` needs to stay within Solana's 4 KB.
#[derive(Accounts)]
pub struct PhoenixPrograms<'info> {
    /// CHECK: Phoenix.
    #[account(address = PHOENIX_PROGRAM_ID)]
    pub phoenix_program: UncheckedAccount<'info>,
    /// CHECK: Phoenix's log authority.
    #[account(address = PHOENIX_LOG_AUTHORITY)]
    pub log_authority: UncheckedAccount<'info>,
    /// CHECK: Phoenix's global configuration.
    #[account(mut, address = PHOENIX_GLOBAL_CONFIG)]
    pub global_config: UncheckedAccount<'info>,
    /// CHECK: Phoenix's read-only view program.
    #[account(address = HAWKEYE_ID)]
    pub hawkeye_program: UncheckedAccount<'info>,
    /// CHECK: Ember, Phoenix's USDC gateway.
    #[account(address = EMBER_PROGRAM_ID)]
    pub ember_program: UncheckedAccount<'info>,
    /// CHECK: Ember's state; Ember's instruction builder names the canonical address.
    pub ember_state: UncheckedAccount<'info>,
    /// CHECK: Ember's USDC vault; Ember's instruction builder names the canonical address.
    #[account(mut)]
    pub ember_vault: UncheckedAccount<'info>,
}

/// `PhoenixPrograms`' account list for building instructions off-chain: Anchor's `accounts`
/// module only lists the structs instructions name directly.
pub use __client_accounts_phoenix_programs::PhoenixPrograms as PhoenixProgramsAccounts;

impl<'info> VaultCore<'info> {
    /// The vault's market, with what one asset token is worth in it now.
    pub fn market(&self) -> Result<Market> {
        Ok(Market {
            decimals: self.basis_vault.asset_decimals,
            base_lot_decimals: self.basis_vault.base_lot_decimals,
            tick_size: self.basis_vault.tick_size,
            multiplier: ui_multiplier(&self.asset_mint, Clock::get()?.unix_timestamp)?,
        })
    }

    /// Shares in circulation: the share mint's supply.
    pub fn share_supply(&self) -> Result<u64> {
        let data = self.share_mint.try_borrow_data()?;
        require!(data.len() >= 44, VaultError::InvalidMint);
        Ok(u64::from_le_bytes(data[36..44].try_into().unwrap()))
    }

    /// Moves the spot vault's asset tokens.
    pub fn transfer_asset(
        &self,
        from: &AccountInfo<'info>,
        to: &AccountInfo<'info>,
        authority: &AccountInfo<'info>,
        amount: u64,
        seeds: &[&[&[u8]]],
    ) -> Result<()> {
        transfer_checked(
            &self.asset_token_program,
            from,
            &self.asset_mint,
            to,
            authority,
            amount,
            self.basis_vault.asset_decimals,
            seeds,
        )
    }

    /// Checks the vault's caps after a change that grew its holding or its short: the short's
    /// notional, the asset held, and the short's share of the market's open interest.
    pub fn check_caps(&self, market: &Market, before: &Snapshot, after: &Snapshot) -> Result<()> {
        if after.short_lots() <= before.short_lots() && after.asset_units <= before.asset_units {
            return Ok(());
        }
        let vault = &self.basis_vault;
        require!(
            after.notional(market)? <= vault.max_position_usd as u128
                && after.asset_units <= vault.max_asset_units,
            VaultError::MaxPositionReached
        );
        require!(
            after.short_lots() as u128 * BPS_SCALE
                <= after.market.open_interest_lots as u128 * vault.max_oi_share_bps as u128,
            VaultError::OpenInterestShareReached
        );
        Ok(())
    }

    /// The vault's Phoenix trader. `extra` adds the accounts the caller's own steps hand to
    /// Phoenix or Ember (the user paying in or being paid, the wrapped-SOL account).
    pub fn trader<'a>(
        &'a self,
        remaining: &'a [AccountInfo<'info>],
        gti: u8,
        atb: u8,
        token_program: &'a AccountInfo<'info>,
        system_program: &'a AccountInfo<'info>,
        extra: &[&AccountInfo<'info>],
    ) -> Result<PhoenixTrader<'a, 'info>> {
        // A spot vault keeps its asset in its own token account; the SOL vault keeps it in
        // Phoenix.
        require!(
            self.vault_asset.is_some() == (self.basis_vault.kind == VaultKind::Spot),
            VaultError::WrongVaultKind
        );
        let index = TraderIndex::split(remaining, gti, atb)?;
        let own: [&AccountInfo<'info>; 20] = [
            &self.phoenix.phoenix_program,
            &self.phoenix.log_authority,
            &self.phoenix.global_config,
            &self.authority,
            &self.trader_account,
            &self.perp_asset_map,
            &self.orderbook,
            &self.spline_collection,
            &self.withdraw_queue,
            &self.global_vault,
            &self.phoenix.hawkeye_program,
            token_program,
            system_program,
            &self.phoenix.ember_program,
            &self.phoenix.ember_state,
            &self.phoenix.ember_vault,
            &self.usdc_mint,
            &self.phoenix_mint,
            &self.vault_usdc,
            &self.vault_phoenix,
        ];
        let mut accounts = Vec::with_capacity(
            own.len()
                + extra.len()
                + index.global_trader_index.len()
                + index.active_trader_buffer.len(),
        );
        accounts.extend(own.into_iter().chain(extra.iter().copied()).cloned());
        accounts.extend(index.global_trader_index.iter().cloned());
        accounts.extend(index.active_trader_buffer.iter().cloned());
        Ok(PhoenixTrader {
            phoenix_program: &self.phoenix.phoenix_program,
            log_authority: &self.phoenix.log_authority,
            global_config: &self.phoenix.global_config,
            authority: &self.authority,
            trader_account: &self.trader_account,
            perp_asset_map: &self.perp_asset_map,
            orderbook: &self.orderbook,
            spline_collection: &self.spline_collection,
            withdraw_queue: &self.withdraw_queue,
            global_vault: &self.global_vault,
            hawkeye_program: &self.phoenix.hawkeye_program,
            token_program,
            system_program,
            index,
            asset_id: self.basis_vault.asset_id,
            accounts,
        })
    }

    pub fn ember<'a>(&'a self) -> Ember<'a, 'info> {
        Ember {
            program: &self.phoenix.ember_program,
            state: &self.phoenix.ember_state,
            vault: &self.phoenix.ember_vault,
            usdc_mint: &self.usdc_mint,
            phoenix_mint: &self.phoenix_mint,
            usdc_account: &self.vault_usdc,
            phoenix_account: &self.vault_phoenix,
        }
    }

    /// Asset the vault holds now: for the SOL vault, every lamport in its trader account
    /// above the rent floor (Phoenix counts them as collateral up to its per-trader cap, and
    /// the vault can withdraw all of them); for a spot vault, its asset tokens.
    pub fn asset_units(&self) -> Result<u64> {
        match &self.vault_asset {
            Some(vault_asset) => token_amount(vault_asset),
            None => {
                let info = self.trader_account.to_account_info();
                let floor = Rent::get()?.minimum_balance(info.data_len());
                Ok(info.lamports().saturating_sub(floor))
            }
        }
    }

    pub fn margin(&self, trader: &PhoenixTrader<'_, 'info>) -> Result<Margin> {
        let m = trader.view_margin()?;
        let collateral = m.collateral_quote_lots as i128 + m.unsettled_funding_quote_lots as i128;
        let effective = m.effective_collateral_quote_lots as i128;
        Ok(Margin {
            equity: collateral + m.unrealized_pnl_quote_lots as i128,
            effective,
            sol_effective: effective - collateral - m.discounted_unrealized_pnl_quote_lots as i128,
            initial: m.initial_margin_quote_lots,
            withdrawable: m.withdrawable_collateral_quote_lots,
        })
    }

    /// What the vault owns now. `with_market` also reads the market's open interest and
    /// session from Phoenix's perp asset map, which deposits and the keeper need; withdrawals
    /// leave it out, so a change to that account cannot stop anyone leaving.
    pub fn snapshot(
        &self,
        trader: &PhoenixTrader<'_, 'info>,
        market: &Market,
        with_market: bool,
    ) -> Result<Snapshot> {
        // A Phoenix instruction first, for every vault: Phoenix checks the trader index
        // accounts the caller passed, which Hawkeye's views below only read. (For the SOL vault
        // it also counts lamports someone sent to the trader account.)
        trader.sync_native()?;
        let asset = trader.view_asset()?;
        require!(
            asset.asset_id == self.basis_vault.asset_id,
            VaultError::InvalidPhoenixAccount
        );
        Ok(Snapshot {
            mark_ticks: asset.mark_price_ticks,
            price: market.price(asset.mark_price_ticks)?,
            position_lots: asset.base_lots,
            asset_units: self.asset_units()?,
            margin: self.margin(trader)?,
            market: if with_market {
                market_state(&self.perp_asset_map, self.basis_vault.asset_id)?
            } else {
                MarketState::default()
            },
        })
    }

    /// Trades the short to the size that hedges `units` of the asset, at no worse than
    /// `slippage_bps` from the mark price.
    ///
    /// Phoenix's mark price can move with the trade, so callers value the vault after it
    /// from a fresh `snapshot`: the asset and the short must be priced at the same mark.
    pub fn hedge(
        &self,
        trader: &PhoenixTrader<'_, 'info>,
        market: &Market,
        before: &Snapshot,
        units: u64,
        seeds: &[&[&[u8]]],
    ) -> Result<()> {
        let target = market.hedge_lots(units)? as i128;
        self.trade(trader, before, target + before.position_lots as i128, seeds)
    }

    /// Closes a leaver's part of the short: the same share of it as `leaving` is of the asset
    /// held, so the vault stays hedged as it was. A withdrawal never opens short: asset someone
    /// donated, or a short Phoenix liquidated or deleveraged, is the keeper's to re-hedge, not
    /// the leaver's (who could be stopped by an order book too thin for it).
    pub fn close_part(
        &self,
        trader: &PhoenixTrader<'_, 'info>,
        before: &Snapshot,
        leaving: u64,
        seeds: &[&[&[u8]]],
    ) -> Result<()> {
        let (short, units) = (before.short_lots() as u128, before.asset_units as u128);
        if units == 0 {
            return Ok(());
        }
        let lots = ((short * leaving as u128 + units / 2) / units).min(short);
        self.trade(trader, before, -(lots as i128), seeds)
    }

    /// Grows the short by `delta` lots (shrinks it when negative) with a market order no worse
    /// than `slippage_bps` from the mark.
    fn trade(
        &self,
        trader: &PhoenixTrader<'_, 'info>,
        before: &Snapshot,
        delta: i128,
        seeds: &[&[&[u8]]],
    ) -> Result<()> {
        if delta != 0 {
            let sell = delta > 0;
            let lots = u64::try_from(delta.unsigned_abs()).map_err(|_| VaultError::MathOverflow)?;
            let limit = limit_ticks(before.mark_ticks, self.basis_vault.slippage_bps, sell)?;
            trader.market_order(sell, lots, limit, seeds)?;
        }
        Ok(())
    }

    /// What a swap may and may not move, to check it against: the vault's USDC, the asset side
    /// (`asset_account`: the vault's asset tokens, or wrapped SOL for the SOL vault), its
    /// Phoenix tokens, the authority's lamports, the native SOL in its trader account and the
    /// share supply.
    fn holdings(&self, asset_account: &AccountInfo<'info>) -> Result<[u64; 6]> {
        Ok([
            token_amount(&self.vault_usdc)?,
            token_amount(asset_account)?,
            token_amount(&self.vault_phoenix)?,
            self.authority.lamports(),
            self.trader_account.lamports(),
            self.share_supply()?,
        ])
    }

    /// Sells about `planned` of one side for the other through the venue's route, signed by
    /// the vault authority. Returns what it spent and what it received.
    #[allow(clippy::too_many_arguments)]
    pub fn swap(
        &self,
        venue: Option<&UncheckedAccount<'info>>,
        route_accounts: &[AccountInfo<'info>],
        route: &SwapRoute,
        sell: Sell,
        planned: u64,
        price: &Price,
        asset_account: &AccountInfo<'info>,
        seeds: &[&[&[u8]]],
    ) -> Result<(u64, u64)> {
        let venue = venue.ok_or(VaultError::InvalidSwapAccounts)?;
        let before = self.holdings(asset_account)?;
        invoke_route(
            venue,
            route_accounts,
            &self.authority.key(),
            &route.data,
            seeds,
        )?;
        let after = self.holdings(asset_account)?;
        for info in [&*self.vault_usdc, asset_account, &*self.vault_phoenix] {
            require_plain_token_account(info, &self.authority.key())?;
        }
        require!(
            share_mint_authority(&self.share_mint)? == Some(self.authority.key()),
            VaultError::SwapMovedVaultFunds
        );
        // The Phoenix-token balance, the authority's lamports, the native SOL in the trader
        // account and the share supply must not move: the route runs with the authority's
        // signature, which could otherwise reach them.
        require!(after[2..] == before[2..], VaultError::SwapMovedVaultFunds);
        let (sold, bought) = match sell {
            Sell::Usdc => (0, 1),
            Sell::Asset => (1, 0),
        };
        require!(
            after[sold] <= before[sold] && after[bought] >= before[bought],
            VaultError::SwapMovedVaultFunds
        );
        let spent = before[sold] - after[sold];
        let received = after[bought] - before[bought];
        let vault = &self.basis_vault;
        check_fill(
            sell,
            planned,
            spent,
            received,
            price,
            vault.asset_decimals,
            vault.slippage_bps,
        )?;
        emit!(Swapped {
            basis_vault: vault.key(),
            sold_usdc: sell == Sell::Usdc,
            spent,
            received,
        });
        Ok((spent, received))
    }
}

/// The route's accounts: the `route.accounts` remaining accounts after Phoenix's index ones.
pub fn route_accounts<'a, 'info>(
    remaining: &'a [AccountInfo<'info>],
    gti: u8,
    atb: u8,
    route: &SwapRoute,
) -> Result<&'a [AccountInfo<'info>]> {
    let start = gti as usize + atb as usize;
    remaining
        .get(start..start + route.accounts as usize)
        .ok_or_else(|| VaultError::InvalidSwapAccounts.into())
}

/// The SOL vault's swap account: the authority's wrapped-SOL account.
pub fn wsol_account<'a, 'info>(
    wsol: &'a Option<UncheckedAccount<'info>>,
) -> Result<&'a AccountInfo<'info>> {
    match wsol {
        Some(wsol) => Ok(wsol),
        None => err!(VaultError::InvalidSwapAccounts),
    }
}
