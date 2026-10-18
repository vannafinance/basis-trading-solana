//! The keeper's backend over the vault's test harness: accounts and views from LiteSVM, the
//! mock Jupiter's routes at Phoenix's mark, and funding the test sets.

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::AccountDeserialize;
use std::collections::{HashMap, HashSet};
use vanna_keeper::{Backend, Funding, Keeper, Policy, RawAccount, Route, Settings};
use vanna_phoenix_multi_vault::state::{request_address, WithdrawRequest};
use vanna_phoenix_multi_vault::swap::Sell;

use crate::env::*;

pub struct Sim {
    pub env: Env,
    /// Funding by Phoenix symbol, as the test sets it.
    pub funding: HashMap<String, Funding>,
    pub alerts: Vec<String>,
    /// Mints Jupiter finds no route for.
    pub no_route: HashSet<Pubkey>,
}

impl Sim {
    pub fn new(env: Env) -> Self {
        Self {
            env,
            funding: HashMap::new(),
            alerts: vec![],
            no_route: HashSet::new(),
        }
    }
}

impl Backend for Sim {
    fn keeper(&self) -> Pubkey {
        self.env.keeper
    }

    fn accounts(&mut self, keys: &[Pubkey]) -> anyhow::Result<Vec<Option<RawAccount>>> {
        Ok(keys
            .iter()
            .map(|k| {
                self.env.ctx.svm.get_account(k).map(|a| RawAccount {
                    owner: a.owner,
                    lamports: a.lamports,
                    data: a.data,
                })
            })
            .collect())
    }

    fn now(&mut self) -> anyhow::Result<i64> {
        Ok(self.env.now())
    }

    fn rent_exempt_minimum(&mut self, len: usize) -> anyhow::Result<u64> {
        Ok(self.env.ctx.svm.minimum_balance_for_rent_exemption(len))
    }

    fn view(&mut self, ix: Instruction) -> anyhow::Result<Vec<u8>> {
        let seed = self.env.admin_seed();
        let meta = self
            .env
            .ctx
            .try_send_instructions_with_metadata(vec![ix], seed)
            .map_err(|f| anyhow::anyhow!("view failed:\n{}", f.meta.logs.join("\n")))?;
        Ok(meta.return_data.data)
    }

    fn withdraw_requests(
        &mut self,
        vault: &Pubkey,
        next_id: u64,
    ) -> anyhow::Result<Vec<(Pubkey, WithdrawRequest)>> {
        Ok((0..next_id)
            .filter_map(|id| {
                let key = request_address(vault, id).0;
                let account = self.env.ctx.svm.get_account(&key)?;
                let request = WithdrawRequest::try_deserialize(&mut &account.data[..]).ok()?;
                Some((key, request))
            })
            .collect())
    }

    fn phoenix_index(&mut self) -> anyhow::Result<(Vec<Pubkey>, Vec<Pubkey>)> {
        Ok((self.env.gti.clone(), self.env.atb.clone()))
    }

    fn send(&mut self, ixs: Vec<Instruction>, _lookup_tables: &[Pubkey], label: &str) -> anyhow::Result<String> {
        let seed = self.env.keeper_seed();
        self.env
            .ctx
            .try_send_instructions_with_metadata(ixs, seed)
            .map(|meta| meta.signature.to_string())
            .map_err(|f| anyhow::anyhow!("{label} failed:\n{}", f.meta.logs.join("\n")))
    }

    fn route(
        &mut self,
        _authority: &Pubkey,
        input: &Pubkey,
        output: &Pubkey,
        amount: u64,
        _slippage_bps: u16,
        _max_accounts: u32,
    ) -> anyhow::Result<Route> {
        if self.no_route.contains(input) || self.no_route.contains(output) {
            anyhow::bail!("Jupiter: no route");
        }
        let (sell, mint) = if *input == self.env.usdc_mint {
            (Sell::Usdc, output)
        } else {
            (Sell::Asset, input)
        };
        let asset = Asset::EVERY
            .into_iter()
            .find(|a| a.mint() == *mint)
            .ok_or_else(|| anyhow::anyhow!("no market for {mint}"))?;
        let route = self.env.route_for(asset, sell, amount);
        let data = &route.args.data;
        let out_amount = u64::from_le_bytes(data[16..24].try_into().unwrap());
        Ok(Route {
            args: route.args,
            metas: route.metas,
            out_amount,
            lookup_tables: vec![],
        })
    }

    fn funding(&mut self, symbol: &str) -> anyhow::Result<Funding> {
        self.funding
            .get(symbol)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("no funding for {symbol}"))
    }

    fn alert(&mut self, message: &str) {
        eprintln!("ALERT: {message}");
        self.alerts.push(message.to_string());
    }
}

/// The keeper over `env`, reading funding on every tick.
pub fn keeper(env: Env, policy: Policy) -> Keeper<Sim> {
    let mut settings = Settings::new(VAULT_ID, policy);
    settings.funding_refresh_secs = 0;
    Keeper::new(Sim::new(env), settings)
}

/// Funding (APR, percent) for each market, as of now.
pub fn set_funding(keeper: &mut Keeper<Sim>, rates: &[(Asset, f64)]) {
    let now = keeper.backend.env.now();
    for (asset, apr) in rates {
        keeper
            .backend
            .funding
            .insert(asset.symbol().to_string(), Funding::flat(*apr, now));
    }
}
