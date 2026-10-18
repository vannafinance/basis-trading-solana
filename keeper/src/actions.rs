//! The program's instructions for each decision, with the accounts the program expects: the
//! vault's core accounts, Phoenix's index accounts, every leg (`load_legs`), then the swap
//! route's accounts for deploys and unwinds.

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::{InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address;
use anyhow::{anyhow, Context};
use vanna_phoenix_multi_vault::constants::NATIVE_MINT;
use vanna_phoenix_multi_vault::instructions::{EmberProgramsAccounts, PhoenixProgramsAccounts};
use vanna_phoenix_multi_vault::phoenix::{
    EMBER_PROGRAM_ID, HAWKEYE_ID, PHOENIX_GLOBAL_CONFIG, PHOENIX_LOG_AUTHORITY, PHOENIX_PROGRAM_ID,
};
use vanna_phoenix_multi_vault::state::LegKind;
use vanna_phoenix_multi_vault::{accounts, instruction};

use crate::backend::{Backend, Route};
use crate::observe::Observation;
use crate::policy::Action;

const TOKEN_PROGRAM: Pubkey = anchor_spl::token::ID;
const TOKEN_2022: Pubkey = anchor_spl::token_2022::ID;
const ATA_PROGRAM: Pubkey = anchor_spl::associated_token::ID;
const SYSTEM_PROGRAM: Pubkey = anchor_lang::system_program::ID;

/// A decision as instructions, and the lookup tables its route needs.
pub struct Built {
    pub ixs: Vec<Instruction>,
    pub lookup_tables: Vec<Pubkey>,
}

fn ix(accounts: impl ToAccountMetas, data: impl InstructionData) -> Instruction {
    Instruction {
        program_id: vanna_phoenix_multi_vault::ID,
        accounts: accounts.to_account_metas(None),
        data: data.data(),
    }
}

fn core(obs: &Observation) -> accounts::VaultCore {
    let v = &obs.vault;
    accounts::VaultCore {
        config: obs.config_address,
        vault: obs.vault_address,
        authority: v.authority,
        share_mint: v.share_mint,
        usdc_mint: obs.config.usdc_mint,
        vault_usdc: v.usdc_account,
        vault_phoenix: v.phoenix_account,
        escrow_shares: v.escrow_shares,
        trader_account: v.trader_account,
        perp_asset_map: obs.config.perp_asset_map,
        phoenix: PhoenixProgramsAccounts {
            phoenix_program: PHOENIX_PROGRAM_ID,
            log_authority: PHOENIX_LOG_AUTHORITY,
            global_config: PHOENIX_GLOBAL_CONFIG,
            hawkeye_program: HAWKEYE_ID,
        },
    }
}

/// Phoenix's index accounts, then every leg: the leg, its holding (spot legs) and its mint
/// (Token-2022 legs); a retired leg alone.
fn with_legs(obs: &Observation, mut ix: Instruction) -> Instruction {
    ix.accounts
        .extend(obs.gti.iter().chain(&obs.atb).map(|k| AccountMeta::new(*k, false)));
    for l in &obs.legs {
        ix.accounts.push(AccountMeta::new_readonly(l.address, false));
        if l.leg.retired {
            continue;
        }
        if l.leg.kind == LegKind::Spot {
            ix.accounts.push(AccountMeta::new(l.leg.asset_account, false));
            if l.leg.asset_token_program == TOKEN_2022 {
                ix.accounts.push(AccountMeta::new_readonly(l.leg.asset_mint, false));
            }
        }
    }
    ix
}

fn index_counts(obs: &Observation) -> (u8, u8) {
    (obs.gti.len() as u8, obs.atb.len() as u8)
}

/// The mint a leg's swaps trade: wrapped SOL for the SOL leg.
fn swap_mint(obs: &Observation, leg: usize) -> Pubkey {
    match obs.legs[leg].leg.kind {
        LegKind::NativeSol => NATIVE_MINT,
        LegKind::Spot => obs.legs[leg].leg.asset_mint,
    }
}

/// Jupiter's quote must clear the program's own check (`check_fill`): at least the mark value
/// less the vault's band, and a sale in a multiplier window at the higher multiplier's value.
/// A worse one would only fail on-chain.
fn check_quote(obs: &Observation, leg: usize, sell_usdc: bool, amount: u64, route: &Route) -> anyhow::Result<()> {
    let s = obs.state(leg);
    let price = if !sell_usdc && obs.legs[leg].multiplier_changing {
        let mut market = s.market;
        market.multiplier = obs.legs[leg].high_multiplier;
        market.price(s.mark_ticks).map_err(|e| anyhow!("{e}"))?
    } else {
        s.price
    };
    let fair = if sell_usdc {
        price.tokens_for_usd(amount as u128, s.market.decimals, false)
    } else {
        price.usd_value(amount as u128, s.market.decimals, false)
    }
    .map_err(|e| anyhow!("{e}"))?;
    let band = 10_000 - obs.vault.slippage_bps as u128;
    anyhow::ensure!(
        route.out_amount as u128 * 10_000 >= fair * band,
        "Jupiter quotes {} for {amount}, under the band around the mark ({} fair)",
        route.out_amount,
        fair
    );
    Ok(())
}

fn sol_accounts(obs: &Observation, leg: usize) -> (Option<Pubkey>, Option<Pubkey>) {
    if obs.legs[leg].leg.kind == LegKind::NativeSol {
        (
            Some(get_associated_token_address(&obs.vault.authority, &NATIVE_MINT)),
            Some(NATIVE_MINT),
        )
    } else {
        (None, None)
    }
}

/// Solana locks at most this many accounts per transaction.
const TX_ACCOUNTS: usize = 64;

/// Accounts a deploy or unwind of leg `i` leaves its route: Solana's limit less what the
/// transaction names besides the route (the instruction's own 23 accounts, 24 for an unwind,
/// two fewer for a spot leg whose wrapped-SOL accounts are left out; Phoenix's index accounts;
/// every leg; the vault and compute budget programs), plus the accounts the route shares with
/// them (the authority, the buffer, the leg's token account and mint, the token programs).
fn route_room(obs: &Observation, leg: usize, unwind: bool) -> u32 {
    let spot = obs.legs[leg].leg.kind == LegKind::Spot;
    let named = 23 + unwind as usize - if spot { 2 } else { 0 } + 2;
    let legs: usize = obs
        .legs
        .iter()
        .map(|l| match (l.leg.retired, l.leg.kind) {
            (true, _) | (false, LegKind::NativeSol) => 1,
            (false, LegKind::Spot) if l.leg.asset_token_program == TOKEN_2022 => 3,
            (false, LegKind::Spot) => 2,
        })
        .sum();
    let shared = if spot { 6 } else { 5 };
    let used = named + obs.gti.len() + obs.atb.len() + legs;
    (TX_ACCOUNTS + shared).saturating_sub(used).clamp(8, 40) as u32
}

/// Phoenix's Ember: its state and USDC vault are PDAs of Phoenix's program id.
fn ember() -> anyhow::Result<EmberProgramsAccounts> {
    Ok(EmberProgramsAccounts {
        ember_program: EMBER_PROGRAM_ID,
        ember_state: phoenix_rise_ix::constants::get_ember_state_address()
            .map_err(|e| anyhow!("ember state: {e:?}"))?,
        ember_vault: phoenix_rise_ix::constants::get_ember_vault_address()
            .map_err(|e| anyhow!("ember vault: {e:?}"))?,
    })
}

/// The instructions for `action` on the vault as `obs` saw it. Deploys and unwinds ask the
/// backend for a Jupiter route first, and refuse one the program would refuse.
pub fn build<B: Backend + ?Sized>(
    action: &Action,
    obs: &Observation,
    backend: &mut B,
) -> anyhow::Result<Built> {
    let keeper = backend.keeper();
    let (gti, atb) = index_counts(obs);
    let leg_at = |leg: u8| -> anyhow::Result<usize> {
        let i = leg as usize;
        anyhow::ensure!(i < obs.legs.len() && obs.legs[i].live(), "leg {leg} is not live");
        Ok(i)
    };
    let plain = |ix| Built {
        ixs: vec![ix],
        lookup_tables: vec![],
    };
    Ok(match *action {
        Action::SetDepositsOpen(open) => plain(ix(
            accounts::SetDeposits {
                signer: keeper,
                config: obs.config_address,
                vault: obs.vault_address,
            },
            instruction::SetDepositsOpen { open },
        )),
        Action::FulfillWithdraw { request, user, .. } => plain(with_legs(
            obs,
            ix(
                accounts::FulfillWithdraw {
                    cranker: keeper,
                    core: core(obs),
                    request,
                    user,
                    user_usdc: get_associated_token_address(&user, &obs.config.usdc_mint),
                    token_program: TOKEN_PROGRAM,
                },
                instruction::FulfillWithdraw { gti, atb },
            ),
        )),
        Action::ReturnWithdraw { request, .. } => plain(ix(
            accounts::ReturnWithdraw {
                signer: keeper,
                config: obs.config_address,
                vault: obs.vault_address,
                request,
            },
            instruction::ReturnWithdraw {},
        )),
        Action::Rehedge { leg } => {
            let i = leg_at(leg)?;
            let l = &obs.legs[i].leg;
            plain(with_legs(
                obs,
                ix(
                    accounts::Rehedge {
                        keeper,
                        core: core(obs),
                        orderbook: l.orderbook,
                        spline_collection: l.spline_collection,
                    },
                    instruction::Rehedge {
                        gti,
                        atb,
                        leg_index: leg,
                    },
                ),
            ))
        }
        Action::MoveMargin { to_phoenix, amount } => plain(with_legs(
            obs,
            ix(
                accounts::MoveMargin {
                    keeper,
                    core: core(obs),
                    ember: ember()?,
                    phoenix_mint: obs.config.phoenix_mint,
                    withdraw_queue: obs.config.withdraw_queue,
                    global_vault: obs.config.global_vault,
                    token_program: TOKEN_PROGRAM,
                    system_program: SYSTEM_PROGRAM,
                },
                instruction::MoveMargin {
                    gti,
                    atb,
                    to_phoenix,
                    amount,
                },
            ),
        )),
        Action::Deploy { leg, usdc } => {
            let i = leg_at(leg)?;
            let route = backend
                .route(
                    &obs.vault.authority,
                    &obs.config.usdc_mint,
                    &swap_mint(obs, i),
                    usdc,
                    obs.vault.slippage_bps,
                    route_room(obs, i, false),
                )
                .context("Jupiter route")?;
            check_quote(obs, i, true, usdc, &route)?;
            let l = &obs.legs[i].leg;
            let (vault_wsol, native_mint) = sol_accounts(obs, i);
            let mut ix = with_legs(
                obs,
                ix(
                    accounts::Deploy {
                        keeper,
                        core: core(obs),
                        orderbook: l.orderbook,
                        spline_collection: l.spline_collection,
                        swap_program: obs.config.swap_program,
                        vault_wsol,
                        native_mint,
                        token_program: TOKEN_PROGRAM,
                        associated_token_program: ATA_PROGRAM,
                        system_program: SYSTEM_PROGRAM,
                    },
                    instruction::Deploy {
                        gti,
                        atb,
                        leg_index: leg,
                        usdc_amount: usdc,
                        route: route.args.clone(),
                    },
                ),
            );
            ix.accounts.extend(route.metas);
            Built {
                ixs: vec![ix],
                lookup_tables: route.lookup_tables,
            }
        }
        Action::Unwind { leg, units } => {
            let i = leg_at(leg)?;
            let route = backend
                .route(
                    &obs.vault.authority,
                    &swap_mint(obs, i),
                    &obs.config.usdc_mint,
                    units,
                    obs.vault.slippage_bps,
                    route_room(obs, i, true),
                )
                .context("Jupiter route")?;
            check_quote(obs, i, false, units, &route)?;
            let l = &obs.legs[i].leg;
            let (vault_wsol, native_mint) = sol_accounts(obs, i);
            let mut ix = with_legs(
                obs,
                ix(
                    accounts::Unwind {
                        keeper,
                        core: core(obs),
                        orderbook: l.orderbook,
                        spline_collection: l.spline_collection,
                        swap_program: obs.config.swap_program,
                        vault_wsol,
                        native_mint,
                        withdraw_queue: (l.kind == LegKind::NativeSol)
                            .then_some(obs.config.withdraw_queue),
                        token_program: TOKEN_PROGRAM,
                        associated_token_program: ATA_PROGRAM,
                        system_program: SYSTEM_PROGRAM,
                    },
                    instruction::Unwind {
                        gti,
                        atb,
                        leg_index: leg,
                        asset_amount: units,
                        route: route.args.clone(),
                    },
                ),
            );
            ix.accounts.extend(route.metas);
            Built {
                ixs: vec![ix],
                lookup_tables: route.lookup_tables,
            }
        }
    })
}
