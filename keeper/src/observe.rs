//! Reads the vault the way the program values it. Every number the keeper decides on comes
//! from the program's own code (`nav::Snapshot`, `Market`, `market_state`, the multiplier
//! reads) fed with the same accounts and Hawkeye views, so the keeper's NAV, reserve and margin
//! are the ones the program will check its moves against.

use anchor_lang::prelude::{AccountInfo, Pubkey};
use anchor_lang::AccountDeserialize;
use anyhow::{anyhow, Context};
use phoenix_rise_ix::hawkeye::{
    create_hawkeye_view_margin_for_asset_ix, create_hawkeye_view_margin_ix, decode_hawkeye_return,
    HawkeyeTraderViewAccounts, ViewAssetReturn, ViewMarginReturn,
};
use std::collections::HashMap;
use vanna_phoenix_multi_vault::constants::*;
use vanna_phoenix_multi_vault::instructions::common::{multiplier_changing, multiplier_range};
use vanna_phoenix_multi_vault::math::Price;
use vanna_phoenix_multi_vault::nav::{LegState, Margin, Market, Snapshot};
use vanna_phoenix_multi_vault::phoenix::{market_state, PHOENIX_GLOBAL_CONFIG, PHOENIX_PROGRAM_ID};
use vanna_phoenix_multi_vault::state::*;

use crate::backend::{Backend, RawAccount};

/// The program's errors as the keeper's.
pub fn anchor<T>(result: anchor_lang::Result<T>) -> anyhow::Result<T> {
    result.map_err(|e| anyhow!("{e}"))
}

pub fn config_address() -> Pubkey {
    Pubkey::find_program_address(&[CONFIG_SEED], &vanna_phoenix_multi_vault::ID).0
}

/// One leg and its market.
#[derive(Clone)]
pub struct LegObs {
    pub index: u8,
    pub address: Pubkey,
    pub leg: Leg,
    /// Phoenix's name for the market, for its funding.
    pub symbol: String,
    pub open_interest_lots: u64,
    /// A stock market outside its session (or reopening after one).
    pub after_hours: bool,
    /// A stock token in the middle of a large multiplier change: no deploys or re-hedges, and
    /// its sales must fetch the higher of its two multipliers' value.
    pub multiplier_changing: bool,
    /// That higher multiplier (the current one outside a window).
    pub high_multiplier: u128,
}

/// A live withdrawal request, and whether the program can pay it: its user's USDC account must
/// be their own, open and not frozen.
#[derive(Clone)]
pub struct RequestObs {
    pub address: Pubkey,
    pub request: WithdrawRequest,
    pub payable: bool,
}

impl LegObs {
    pub fn live(&self) -> bool {
        !self.leg.retired
    }
}

/// Everything the keeper decides on, read at one moment.
#[derive(Clone)]
pub struct Observation {
    pub now: i64,
    pub config_address: Pubkey,
    pub config: Config,
    pub vault_address: Pubkey,
    pub vault: Vault,
    /// Every leg, in index order (retired ones too).
    pub legs: Vec<LegObs>,
    /// The program's valuation: `snapshot.legs[i]` is leg `i`.
    pub snapshot: Snapshot,
    /// Live withdrawal requests (not returned), oldest first.
    pub requests: Vec<RequestObs>,
    pub gti: Vec<Pubkey>,
    pub atb: Vec<Pubkey>,
    /// The keeper's SOL, for fees and rent.
    pub keeper_lamports: u64,
}

impl Observation {
    pub fn state(&self, index: usize) -> &LegState {
        &self.snapshot.legs[index]
    }

    pub fn nav(&self) -> u128 {
        self.snapshot.nav().unwrap_or(0)
    }

    /// Idle USDC the keeper may not spend: the vault's buffer and the pending requests' value.
    pub fn reserved(&self) -> u128 {
        self.snapshot
            .reserved_usdc(self.vault.buffer_bps)
            .unwrap_or(u128::MAX)
    }

    /// Idle USDC above the reserve.
    pub fn free_usdc(&self) -> u64 {
        (self.snapshot.idle_usdc as u128).saturating_sub(self.reserved()) as u64
    }

    pub fn total_notional(&self) -> u128 {
        self.snapshot.total_notional().unwrap_or(0)
    }

    pub fn value(&self, index: usize) -> u128 {
        self.state(index).value().unwrap_or(0)
    }

    pub fn notional(&self, index: usize) -> u128 {
        self.state(index).notional().unwrap_or(0)
    }

    /// What deploys and unwinds may still move in the current turnover window (USD, 6 dp).
    pub fn turnover_left(&self) -> u64 {
        let v = &self.vault;
        if self.now >= v.turnover_window_start.saturating_add(TURNOVER_WINDOW_SECONDS) {
            v.max_turnover_usd
        } else {
            v.max_turnover_usd.saturating_sub(v.turnover_window_usd)
        }
    }

    /// Phoenix effective collateral per dollar of every short, in bps (None when flat).
    pub fn margin_bps(&self) -> Option<i128> {
        let total = self.total_notional();
        (total > 0).then(|| self.snapshot.margin.effective * BPS_SCALE as i128 / total as i128)
    }

    /// One line per leg and one for the vault, for the log.
    pub fn summary(&self) -> String {
        let usd = |v: u128| v as f64 / 1e6;
        let mut out = format!(
            "NAV ${:.2} | idle ${:.2} (reserved ${:.2}) | Phoenix equity ${:.2} | margin {} | supply {:.6} | pending {} request(s) | deposits {}",
            usd(self.nav()),
            self.snapshot.idle_usdc as f64 / 1e6,
            usd(self.reserved().min(u64::MAX as u128)),
            self.snapshot.margin.equity as f64 / 1e6,
            self.margin_bps()
                .map(|b| format!("{:.1}%", b as f64 / 100.0))
                .unwrap_or_else(|| "flat".into()),
            self.snapshot.supply as f64 / 1e6,
            self.requests.len(),
            if self.vault.deposits_open { "open" } else { "closed" },
        );
        for (i, l) in self.legs.iter().enumerate().filter(|(_, l)| l.live()) {
            let s = self.state(i);
            out.push_str(&format!(
                "\n  leg {} {:<6} held ${:>12.2} short ${:>12.2} ({} lots) OI {} lots{}{}",
                l.index,
                l.symbol,
                usd(self.value(i)),
                usd(self.notional(i)),
                s.position_lots,
                l.open_interest_lots,
                if l.after_hours { " after-hours" } else { "" },
                if l.leg.active { "" } else { " inactive" },
            ));
        }
        out
    }
}

fn deserialize<T: AccountDeserialize>(account: Option<&RawAccount>, what: &str) -> anyhow::Result<T> {
    let account = account.with_context(|| format!("{what}: no account"))?;
    anyhow::ensure!(
        account.owner == vanna_phoenix_multi_vault::ID,
        "{what}: not the vault program's"
    );
    T::try_deserialize(&mut &account.data[..]).map_err(|e| anyhow!("{what}: {e}"))
}

/// Runs `f` with the account as the program would see it.
fn with_info<T>(key: &Pubkey, account: &RawAccount, f: impl FnOnce(&AccountInfo) -> T) -> T {
    let mut lamports = account.lamports;
    let mut data = account.data.clone();
    let info = AccountInfo::new(key, false, false, &mut lamports, &mut data, &account.owner, false);
    f(&info)
}

fn token_amount(account: &RawAccount) -> anyhow::Result<u64> {
    anyhow::ensure!(account.data.len() >= 72, "not a token account");
    Ok(u64::from_le_bytes(account.data[64..72].try_into().unwrap()))
}

fn view<B: Backend + ?Sized, T: bytemuck::Pod + Copy>(
    backend: &mut B,
    ix: phoenix_rise_ix::types::Instruction,
    kind: &'static str,
) -> anyhow::Result<T> {
    let mut data = backend.view(ix.into())?;
    data.resize(std::mem::size_of::<T>(), 0);
    decode_hawkeye_return::<T>(&data, kind).map_err(|e| anyhow!("{kind}: {e:?}"))
}

// Phoenix's perp asset map: a 48-byte header, then 1,584-byte slots, one per market (the
// program reads the same layout in `phoenix::market_state`). A slot starts with the market's
// name.
const ASSET_MAP_HEADER: usize = 48;
const ASSET_SLOT: usize = 1_584;
const SLOT_SYMBOL: usize = 16;

/// The market's name in Phoenix's perp asset map.
fn market_symbol(data: &[u8], asset_id: u32) -> Option<String> {
    let slots = u32::from_le_bytes(data.get(32..36)?.try_into().ok()?) as usize;
    let read_u16 = |at: usize| Some(u16::from_le_bytes([*data.get(at)?, *data.get(at + 1)?]));
    let at = (0..slots.min(1_024))
        .map(|slot| ASSET_MAP_HEADER + slot * ASSET_SLOT)
        .find(|&at| {
            let id = read_u16(at + 960).unwrap_or(0) as u32 | (read_u16(at + 964).unwrap_or(0) as u32) << 16;
            id == asset_id && data.get(at + 1_466) == Some(&0) && data.get(at + 1_583) == Some(&0)
        })?;
    let name = data.get(at..at + SLOT_SYMBOL)?;
    let end = name.iter().position(|b| *b == 0).unwrap_or(SLOT_SYMBOL);
    let symbol = std::str::from_utf8(&name[..end]).ok()?.trim().to_string();
    (!symbol.is_empty() && symbol.chars().all(|c| c.is_ascii_graphic())).then_some(symbol)
}

fn view_accounts(gti: &[Pubkey], atb: &[Pubkey], perp_asset_map: Pubkey, trader: Pubkey) -> HawkeyeTraderViewAccounts {
    HawkeyeTraderViewAccounts {
        phoenix_program_id: PHOENIX_PROGRAM_ID,
        global_config: PHOENIX_GLOBAL_CONFIG,
        global_trader_index: gti.to_vec(),
        active_trader_buffer: atb.to_vec(),
        perp_asset_map,
        trader,
    }
}

/// Reads vault `vault_id`: its config, vault and legs, every holding, Phoenix's margin and
/// position views, the perp asset map and the live withdrawal requests. `symbols` names
/// markets whose perp-map name is not the API's.
pub fn observe<B: Backend + ?Sized>(
    backend: &mut B,
    vault_id: u16,
    symbols: &HashMap<u32, String>,
) -> anyhow::Result<Observation> {
    let config_key = config_address();
    let vault_key = vault_address(vault_id).0;
    let found = backend.accounts(&[config_key, vault_key])?;
    let config: Config = deserialize(found[0].as_ref(), "config")?;
    let vault: Vault = deserialize(found[1].as_ref(), "vault")?;

    let leg_keys: Vec<Pubkey> = (0..vault.leg_count)
        .map(|i| leg_address(&vault_key, i).0)
        .collect();
    let leg_accounts = backend.accounts(&leg_keys)?;
    let legs: Vec<Leg> = leg_accounts
        .iter()
        .enumerate()
        .map(|(i, a)| deserialize(a.as_ref(), &format!("leg {i}")))
        .collect::<anyhow::Result<_>>()?;

    // The buffer, the share mint, the trader account, then each live spot leg's holding and,
    // for Token-2022 legs, its mint.
    let mut keys = vec![vault.usdc_account, vault.share_mint, vault.trader_account];
    let mut at = Vec::with_capacity(legs.len());
    for leg in &legs {
        let holding = (!leg.retired && leg.kind == LegKind::Spot).then(|| {
            keys.push(leg.asset_account);
            keys.len() - 1
        });
        let mint = (!leg.retired
            && leg.kind == LegKind::Spot
            && leg.asset_token_program == anchor_spl::token_2022::ID)
            .then(|| {
                keys.push(leg.asset_mint);
                keys.len() - 1
            });
        at.push((holding, mint));
    }
    let accounts = backend.accounts(&keys)?;
    let get = |i: usize, what: &str| {
        accounts[i]
            .as_ref()
            .with_context(|| format!("{what} {} missing", keys[i]))
    };
    let idle_usdc = token_amount(get(0, "vault USDC")?)?;
    let share_mint = get(1, "share mint")?;
    anyhow::ensure!(share_mint.data.len() >= 44, "share mint too short");
    let supply = u64::from_le_bytes(share_mint.data[36..44].try_into().unwrap());
    let trader = get(2, "trader account")?;
    let perp_map = &backend.perp_asset_map(&config.perp_asset_map)?;

    let now = backend.now()?;
    let sol_floor = backend.rent_exempt_minimum(trader.data.len())?;
    let sol_units = trader.lamports.saturating_sub(sol_floor);
    let (gti, atb) = backend.phoenix_index()?;
    let accounts_for_views = || view_accounts(&gti, &atb, config.perp_asset_map, vault.trader_account);

    let m: ViewMarginReturn = view(
        backend,
        create_hawkeye_view_margin_ix(accounts_for_views()),
        "view_margin",
    )?;
    let collateral = m.collateral_quote_lots as i128 + m.unsettled_funding_quote_lots as i128;
    let margin = Margin {
        equity: collateral + m.unrealized_pnl_quote_lots as i128,
        effective: m.effective_collateral_quote_lots as i128,
        initial: m.initial_margin_quote_lots,
        withdrawable: m.withdrawable_collateral_quote_lots,
    };

    let mut states = Vec::with_capacity(legs.len());
    let mut observed = Vec::with_capacity(legs.len());
    for (i, leg) in legs.iter().enumerate() {
        let (holding, mint) = at[i];
        let (multiplier, high_multiplier, changing) = match mint {
            Some(m) => {
                let account = get(m, "asset mint")?;
                with_info(&keys[m], account, |info| -> anyhow::Result<(u128, u128, bool)> {
                    let (current, _, high) = anchor(multiplier_range(info, now))?;
                    Ok((current, high, anchor(multiplier_changing(info, now))?))
                })?
            }
            None => (MULTIPLIER_SCALE, MULTIPLIER_SCALE, false),
        };
        let market = Market {
            decimals: leg.asset_decimals,
            base_lot_decimals: leg.base_lot_decimals,
            tick_size: leg.tick_size,
            multiplier,
        };
        let state = if leg.retired {
            LegState {
                market,
                mark_ticks: 0,
                price: Price {
                    value: 0,
                    expo: -(USD_DECIMALS as i32),
                },
                position_lots: 0,
                units: 0,
            }
        } else {
            let asset: ViewAssetReturn = view(
                backend,
                create_hawkeye_view_margin_for_asset_ix(accounts_for_views(), leg.asset_id),
                "view_margin_for_asset",
            )?;
            anyhow::ensure!(asset.asset_id == leg.asset_id, "leg {i}: Hawkeye viewed another market");
            let units = match holding {
                Some(h) => token_amount(get(h, "asset account")?)?,
                None => sol_units,
            };
            LegState {
                market,
                mark_ticks: asset.mark_price_ticks,
                price: anchor(market.price(asset.mark_price_ticks))
                    .with_context(|| format!("leg {i} mark"))?,
                position_lots: asset.base_lots,
                units,
            }
        };
        let (open_interest_lots, after_hours) = if leg.retired {
            (0, false)
        } else {
            let s = with_info(&config.perp_asset_map, perp_map, |info| {
                anchor(market_state(info, leg.asset_id))
            })?;
            (s.open_interest_lots, s.after_hours)
        };
        let symbol = symbols
            .get(&leg.asset_id)
            .cloned()
            .or_else(|| market_symbol(&perp_map.data, leg.asset_id))
            .unwrap_or_else(|| format!("asset-{}", leg.asset_id));
        states.push(state);
        observed.push(LegObs {
            index: i as u8,
            address: leg_keys[i],
            leg: leg.clone(),
            symbol,
            open_interest_lots,
            after_hours,
            multiplier_changing: changing,
            high_multiplier,
        });
    }

    let mut live = backend.withdraw_requests(&vault_key, vault.next_request_id)?;
    live.retain(|(_, r)| r.vault == vault_key && !r.returned);
    live.sort_by_key(|(_, r)| r.id);
    // Each user's USDC account and the keeper's own SOL.
    let mut keys: Vec<Pubkey> = live
        .iter()
        .map(|(_, r)| anchor_spl::associated_token::get_associated_token_address(&r.user, &config.usdc_mint))
        .collect();
    keys.push(backend.keeper());
    let found = backend.accounts(&keys)?;
    let keeper_lamports = found.last().and_then(|a| a.as_ref()).map_or(0, |a| a.lamports);
    let requests = live
        .into_iter()
        .zip(found)
        .map(|((address, request), usdc)| RequestObs {
            payable: usdc.is_some_and(|a| {
                // A classic token account: the mint, its owner, and Initialized (not Frozen).
                a.owner == anchor_spl::token::ID
                    && a.data.len() >= 165
                    && a.data[..32] == config.usdc_mint.to_bytes()
                    && a.data[32..64] == request.user.to_bytes()
                    && a.data[108] == 1
            }),
            address,
            request,
        })
        .collect();

    Ok(Observation {
        now,
        config_address: config_key,
        snapshot: Snapshot {
            legs: states,
            idle_usdc,
            margin,
            supply,
            pending_shares: vault.pending_shares,
        },
        config,
        vault_address: vault_key,
        vault,
        legs: observed,
        requests,
        gti,
        atb,
        keeper_lamports,
    })
}
