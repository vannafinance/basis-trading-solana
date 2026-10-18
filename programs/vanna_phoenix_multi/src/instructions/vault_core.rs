//! The accounts and steps every vault operation shares: reading every leg and the vault's
//! value from Phoenix, trading a leg's short, and swapping a leg's asset through Jupiter.

use crate::constants::*;
use crate::errors::VaultError;
use crate::events::Swapped;
use crate::instructions::common::{multiplier_changing, multiplier_range, token_amount};
use crate::math::Price;
use crate::nav::*;
use crate::phoenix::*;
use crate::state::*;
use crate::swap::*;
use anchor_lang::prelude::*;
use solana_sha256_hasher::hash;

#[derive(Accounts)]
pub struct VaultCore<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    /// A vault of this program: only `create_vault` makes one, at its id's PDA, so the owner
    /// and type checks are enough. Writable for the keeper's turnover count and request ids.
    #[account(mut)]
    pub vault: Box<Account<'info, Vault>>,
    /// CHECK: the vault's data-less authority PDA, the Phoenix trader.
    #[account(mut, address = vault.authority)]
    pub authority: UncheckedAccount<'info>,
    /// CHECK: the vault's share mint, created by `create_vault`.
    #[account(mut, address = vault.share_mint)]
    pub share_mint: UncheckedAccount<'info>,
    /// CHECK: USDC.
    #[account(address = config.usdc_mint)]
    pub usdc_mint: UncheckedAccount<'info>,
    /// CHECK: the authority's USDC account: the buffer.
    #[account(mut, address = vault.usdc_account)]
    pub vault_usdc: UncheckedAccount<'info>,
    /// CHECK: the authority's Phoenix-token account.
    #[account(mut, address = vault.phoenix_account)]
    pub vault_phoenix: UncheckedAccount<'info>,
    /// CHECK: the authority's share account, holding pending requests' shares.
    #[account(mut, address = vault.escrow_shares)]
    pub escrow_shares: UncheckedAccount<'info>,
    /// CHECK: the vault's Phoenix trader account.
    #[account(mut, address = vault.trader_account)]
    pub trader_account: UncheckedAccount<'info>,
    /// CHECK: Phoenix's perp asset map.
    #[account(mut, address = config.perp_asset_map)]
    pub perp_asset_map: UncheckedAccount<'info>,
    pub phoenix: PhoenixPrograms<'info>,
}

/// Phoenix's and Hawkeye's programs and fixed accounts. A struct of its own: Anchor checks it
/// in its own stack frame.
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
}

/// Ember, Phoenix's USDC gateway: for moving USDC margin in and out.
#[derive(Accounts)]
pub struct EmberPrograms<'info> {
    /// CHECK: Ember.
    #[account(address = EMBER_PROGRAM_ID)]
    pub ember_program: UncheckedAccount<'info>,
    /// CHECK: Ember's state; Ember's instruction builder names the canonical address.
    pub ember_state: UncheckedAccount<'info>,
    /// CHECK: Ember's USDC vault; Ember's instruction builder names the canonical address.
    #[account(mut)]
    pub ember_vault: UncheckedAccount<'info>,
}

/// Account lists for building instructions off-chain: Anchor's `accounts` module only lists
/// the structs instructions name directly.
pub use __client_accounts_ember_programs::EmberPrograms as EmberProgramsAccounts;
pub use __client_accounts_phoenix_programs::PhoenixPrograms as PhoenixProgramsAccounts;

/// Which multiplier a stock token is valued at while a large change is in its window
/// (`multiplier_range`): the current one for the keeper's own checks, the lower for
/// withdrawals, the higher for deposits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pricing {
    Mark,
    Low,
    High,
}

/// A leg read from the instruction's leg list.
pub struct LegRef<'a, 'info> {
    pub leg: Leg,
    pub key: Pubkey,
    /// The leg's asset account (spot legs).
    pub holding: Option<&'a AccountInfo<'info>>,
    /// The leg's mint (Token-2022 legs, for the multiplier).
    pub mint: Option<&'a AccountInfo<'info>>,
}

/// The vault's legs from the instruction's leg list (after Phoenix's index accounts): every
/// leg, in order, each followed by its holding and Token-2022 mint (`Leg::account_count`); a
/// retired leg alone. Returns them and how many accounts they took. Valuing the vault without
/// one of its legs would misprice every share, so all must be there.
pub fn load_legs<'a, 'info>(
    vault_key: &Pubkey,
    vault: &Vault,
    accounts: &'a [AccountInfo<'info>],
) -> Result<(Vec<LegRef<'a, 'info>>, usize)> {
    let mut legs = Vec::with_capacity(vault.leg_count as usize);
    let mut at = 0;
    for index in 0..vault.leg_count {
        let info = accounts.get(at).ok_or(VaultError::InvalidLegAccounts)?;
        require_keys_eq!(*info.owner, crate::ID, VaultError::InvalidLegAccounts);
        let leg = Leg::try_deserialize(&mut &info.try_borrow_data()?[..])
            .map_err(|_| VaultError::InvalidLegAccounts)?;
        // `add_leg` makes the leg of each index once, at its PDA: this is that leg.
        require!(
            leg.vault == *vault_key && leg.index == index,
            VaultError::InvalidLegAccounts
        );
        let n = leg.account_count();
        let accounts = accounts
            .get(at..at + n)
            .ok_or(VaultError::InvalidLegAccounts)?;
        let holding = match leg.kind {
            _ if leg.retired => None,
            LegKind::NativeSol => None,
            LegKind::Spot => {
                require_keys_eq!(
                    accounts[1].key(),
                    leg.asset_account,
                    VaultError::InvalidLegAccounts
                );
                Some(&accounts[1])
            }
        };
        let mint = if n == 3 {
            require_keys_eq!(
                accounts[2].key(),
                leg.asset_mint,
                VaultError::InvalidLegAccounts
            );
            Some(&accounts[2])
        } else {
            None
        };
        legs.push(LegRef {
            leg,
            key: info.key(),
            holding,
            mint,
        });
        at += n;
    }
    Ok((legs, at))
}

impl<'info> VaultCore<'info> {
    /// Shares in circulation: the share mint's supply.
    pub fn share_supply(&self) -> Result<u64> {
        let data = self.share_mint.try_borrow_data()?;
        require!(data.len() >= 44, VaultError::InvalidMint);
        Ok(u64::from_le_bytes(data[36..44].try_into().unwrap()))
    }

    /// The vault's Phoenix trader. `extra` adds the accounts the caller's own steps hand to
    /// Phoenix or Ember (a leg's market, the token and system programs, Ember, the payer).
    #[allow(clippy::too_many_arguments)]
    pub fn trader<'a>(
        &'a self,
        remaining: &'a [AccountInfo<'info>],
        gti: u8,
        atb: u8,
        extra: &[&AccountInfo<'info>],
        withdraw_queue: Option<&'a AccountInfo<'info>>,
        global_vault: Option<&'a AccountInfo<'info>>,
    ) -> Result<PhoenixTrader<'a, 'info>> {
        let index = TraderIndex::split(remaining, gti, atb)?;
        let own: [&AccountInfo<'info>; 11] = [
            &self.phoenix.phoenix_program,
            &self.phoenix.log_authority,
            &self.phoenix.global_config,
            &self.phoenix.hawkeye_program,
            &self.authority,
            &self.trader_account,
            &self.perp_asset_map,
            &self.usdc_mint,
            &self.vault_usdc,
            &self.vault_phoenix,
            &self.share_mint,
        ];
        let mut accounts = Vec::with_capacity(
            own.len()
                + extra.len()
                + 2
                + index.global_trader_index.len()
                + index.active_trader_buffer.len(),
        );
        accounts.extend(own.into_iter().chain(extra.iter().copied()).cloned());
        accounts.extend(withdraw_queue.into_iter().chain(global_vault).cloned());
        accounts.extend(index.global_trader_index.iter().cloned());
        accounts.extend(index.active_trader_buffer.iter().cloned());
        Ok(PhoenixTrader {
            global_config: &self.phoenix.global_config,
            authority: &self.authority,
            trader_account: &self.trader_account,
            perp_asset_map: &self.perp_asset_map,
            withdraw_queue,
            global_vault,
            index,
            accounts,
        })
    }

    /// The asset a leg holds now: for the SOL leg, every lamport in the trader account above
    /// its rent floor (Phoenix counts them as collateral up to its per-trader cap); for a spot
    /// leg, its asset tokens.
    pub fn units(&self, leg: &LegRef<'_, 'info>) -> Result<u64> {
        match leg.holding {
            Some(holding) => token_amount(holding),
            None => {
                let info = self.trader_account.to_account_info();
                let floor = Rent::get()?.minimum_balance(info.data_len());
                Ok(info.lamports().saturating_sub(floor))
            }
        }
    }

    pub fn market(&self, leg: &LegRef<'_, 'info>, now: i64, pricing: Pricing) -> Result<Market> {
        Ok(Market {
            decimals: leg.leg.asset_decimals,
            base_lot_decimals: leg.leg.base_lot_decimals,
            tick_size: leg.leg.tick_size,
            multiplier: match leg.mint {
                Some(mint) => {
                    let (current, low, high) = multiplier_range(mint, now)?;
                    match pricing {
                        Pricing::Mark => current,
                        Pricing::Low => low,
                        Pricing::High => high,
                    }
                }
                None => MULTIPLIER_SCALE,
            },
        })
    }

    /// Whether a leg's token is in the middle of a large multiplier change.
    pub fn changing(&self, leg: &LegRef<'_, 'info>) -> Result<bool> {
        match leg.mint {
            Some(mint) => multiplier_changing(mint, Clock::get()?.unix_timestamp),
            None => Ok(false),
        }
    }

    pub fn margin(&self, trader: &PhoenixTrader<'_, 'info>) -> Result<Margin> {
        let m = trader.view_margin()?;
        let collateral = m.collateral_quote_lots as i128 + m.unsettled_funding_quote_lots as i128;
        Ok(Margin {
            equity: collateral + m.unrealized_pnl_quote_lots as i128,
            effective: m.effective_collateral_quote_lots as i128,
            initial: m.initial_margin_quote_lots,
            withdrawable: m.withdrawable_collateral_quote_lots,
        })
    }

    fn leg_state(
        &self,
        trader: &PhoenixTrader<'_, 'info>,
        leg: &LegRef<'_, 'info>,
        now: i64,
        pricing: Pricing,
    ) -> Result<LegState> {
        if leg.leg.retired {
            // Flat when it was retired, and nothing of it is read again.
            return Ok(LegState {
                market: self.market(leg, now, Pricing::Mark)?,
                mark_ticks: 0,
                price: Price {
                    value: 0,
                    expo: -(USD_DECIMALS as i32),
                },
                position_lots: 0,
                units: 0,
            });
        }
        let view = trader.view_asset(leg.leg.asset_id)?;
        let market = self.market(leg, now, pricing)?;
        Ok(LegState {
            market,
            mark_ticks: view.mark_price_ticks,
            price: market.price(view.mark_price_ticks)?,
            position_lots: view.base_lots,
            units: self.units(leg)?,
        })
    }

    /// What the vault owns now, every leg valued at its mark (and, for a stock token in a
    /// multiplier change, at the multiplier `pricing` picks).
    pub fn snapshot(
        &self,
        trader: &PhoenixTrader<'_, 'info>,
        legs: &[LegRef<'_, 'info>],
        pricing: Pricing,
    ) -> Result<Snapshot> {
        // A Phoenix instruction first: Phoenix checks the trader index accounts the caller
        // passed, which Hawkeye's views below only read. (It also counts lamports someone sent
        // to the trader account.)
        trader.sync_native()?;
        let now = Clock::get()?.unix_timestamp;
        let mut states = Vec::with_capacity(legs.len());
        for leg in legs {
            states.push(self.leg_state(trader, leg, now, pricing)?);
        }
        Ok(Snapshot {
            legs: states,
            idle_usdc: token_amount(&self.vault_usdc)?,
            margin: self.margin(trader)?,
            supply: self.share_supply()?,
            pending_shares: self.vault.pending_shares,
        })
    }

    /// `before` after a keeper operation on leg `index`: the other legs' assets and marks are
    /// untouched (a swap may not move them), so only this leg, the margin and the balances are
    /// read again.
    pub fn refresh(
        &self,
        trader: &PhoenixTrader<'_, 'info>,
        legs: &[LegRef<'_, 'info>],
        before: &Snapshot,
        index: usize,
    ) -> Result<Snapshot> {
        let mut after = before.clone();
        after.legs[index] = self.leg_state(
            trader,
            &legs[index],
            Clock::get()?.unix_timestamp,
            Pricing::Mark,
        )?;
        after.idle_usdc = token_amount(&self.vault_usdc)?;
        after.margin = self.margin(trader)?;
        after.supply = self.share_supply()?;
        Ok(after)
    }

    /// Trades a leg's short to the size that hedges `units` of its asset, at no worse than
    /// `slippage_bps` from the mark.
    pub fn hedge(
        &self,
        trader: &PhoenixTrader<'_, 'info>,
        market: &MarketAccounts<'_, 'info>,
        state: &LegState,
        units: u64,
        seeds: &[&[&[u8]]],
    ) -> Result<()> {
        let target = state.market.hedge_lots(units)? as i128;
        self.trade(trader, market, state, target + state.position_lots as i128, seeds)
    }

    /// Grows a leg's short by `delta` lots (shrinks it when negative) with a market order no
    /// worse than `slippage_bps` from the mark.
    pub fn trade(
        &self,
        trader: &PhoenixTrader<'_, 'info>,
        market: &MarketAccounts<'_, 'info>,
        state: &LegState,
        delta: i128,
        seeds: &[&[&[u8]]],
    ) -> Result<()> {
        if delta != 0 {
            let sell = delta > 0;
            let lots = u64::try_from(delta.unsigned_abs()).map_err(|_| VaultError::MathOverflow)?;
            let limit = limit_ticks(state.mark_ticks, self.vault.slippage_bps, sell)?;
            trader.market_order(market, sell, lots, limit, seeds)?;
        }
        Ok(())
    }

    /// Everything a swap may and may not move, to check it against: the vault's USDC, the
    /// leg's side (its asset account, or wrapped SOL for the SOL leg), then what must not move
    /// at all: its Phoenix tokens, the escrowed shares, the authority's lamports, the native SOL
    /// in its trader account, the share supply and every other leg's asset.
    fn holdings(&self, asset_account: &AccountInfo<'info>, others: &[&AccountInfo<'info>]) -> Result<Vec<u64>> {
        let mut balances = Vec::with_capacity(7 + others.len());
        balances.push(token_amount(&self.vault_usdc)?);
        balances.push(token_amount(asset_account)?);
        balances.push(token_amount(&self.vault_phoenix)?);
        balances.push(token_amount(&self.escrow_shares)?);
        balances.push(self.authority.lamports());
        balances.push(self.trader_account.lamports());
        balances.push(self.share_supply()?);
        for other in others {
            balances.push(token_amount(other)?);
        }
        Ok(balances)
    }

    /// Sells exactly `planned` of one side of leg `index` for the other through Jupiter, the
    /// route resized to that amount and signed by the vault authority. Returns what it spent
    /// and what it received. With `Config.swap_program` turned off (the default key), nothing
    /// swaps.
    #[allow(clippy::too_many_arguments)]
    pub fn swap(
        &self,
        venue: &AccountInfo<'info>,
        route_accounts: &[AccountInfo<'info>],
        route: &SwapRoute,
        sell: Sell,
        planned: u64,
        legs: &[LegRef<'_, 'info>],
        index: usize,
        price: &Price,
        asset_account: &AccountInfo<'info>,
        seeds: &[&[&[u8]]],
    ) -> Result<(u64, u64)> {
        require_keys_eq!(
            *venue.key,
            JUPITER_PROGRAM_ID,
            VaultError::InvalidSwapAccounts
        );
        let data = resize_route(&route.data, planned)?;
        let others: Vec<&AccountInfo<'info>> = legs
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .filter_map(|(_, leg)| leg.holding)
            .collect();
        let before = self.holdings(asset_account, &others)?;
        let trader_before = hash(&self.trader_account.try_borrow_data()?);
        invoke_route(venue, route_accounts, &self.authority.key(), &data, seeds)?;
        let after = self.holdings(asset_account, &others)?;
        // Nor may it reach the vault's Phoenix account through Phoenix itself.
        require!(
            hash(&self.trader_account.try_borrow_data()?) == trader_before,
            VaultError::SwapMovedVaultFunds
        );
        let authority = self.authority.key();
        // A route signed by the authority must not reassign it or give it data: every later
        // lamport transfer from it would fail.
        require!(
            *self.authority.owner == anchor_lang::system_program::ID && self.authority.data_is_empty(),
            VaultError::SwapMovedVaultFunds
        );
        for info in [&*self.vault_usdc, asset_account, &*self.vault_phoenix, &*self.escrow_shares]
            .into_iter()
            .chain(others.iter().copied())
        {
            require_plain_token_account(info, &authority)?;
        }
        require!(
            share_mint_authority(&self.share_mint)? == Some(authority),
            VaultError::SwapMovedVaultFunds
        );
        // Only the two sides may move: the route runs with the authority's signature, which
        // could otherwise reach everything else the vault owns.
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
        check_fill(
            sell,
            planned,
            spent,
            received,
            price,
            legs[index].leg.asset_decimals,
            self.vault.slippage_bps,
        )?;
        emit!(Swapped {
            vault: self.vault.key(),
            leg_index: index as u8,
            sold_usdc: sell == Sell::Usdc,
            spent,
            received,
        });
        Ok((spent, received))
    }
}

/// The route's accounts: the `route.accounts` remaining accounts after the leg list.
pub fn route_accounts<'a, 'info>(
    remaining: &'a [AccountInfo<'info>],
    start: usize,
    route: &SwapRoute,
) -> Result<&'a [AccountInfo<'info>]> {
    remaining
        .get(start..start + route.accounts as usize)
        .ok_or_else(|| VaultError::InvalidSwapAccounts.into())
}
