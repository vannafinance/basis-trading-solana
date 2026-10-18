//! What the keeper needs from the world, as one trait: the chain (accounts, read-only views,
//! sending), Phoenix's funding and Jupiter's routes. `live::LiveBackend` talks to mainnet; the
//! tests run the same keeper against the real programs in LiteSVM.

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use vanna_phoenix_multi_vault::state::WithdrawRequest;
use vanna_phoenix_multi_vault::swap::SwapRoute;

use crate::funding::Funding;

/// An account as the chain holds it.
#[derive(Clone, Debug)]
pub struct RawAccount {
    pub owner: Pubkey,
    pub lamports: u64,
    pub data: Vec<u8>,
}

/// A Jupiter route built for the vault authority: what `deploy` and `unwind` take.
#[derive(Clone, Debug)]
pub struct Route {
    pub args: SwapRoute,
    /// The route's accounts, none of them signing: the vault authority signs inside the program.
    pub metas: Vec<AccountMeta>,
    /// What Jupiter quoted for the input amount.
    pub out_amount: u64,
    /// Jupiter's lookup tables for the route's accounts.
    pub lookup_tables: Vec<Pubkey>,
}

pub trait Backend {
    /// The keeper's key: it signs and pays for every transaction.
    fn keeper(&self) -> Pubkey;

    /// Accounts in order, None where there is none.
    fn accounts(&mut self, keys: &[Pubkey]) -> anyhow::Result<Vec<Option<RawAccount>>>;

    /// Phoenix's perp asset map. A backend may read only the slots in use and leave the rest
    /// zero: the program's reads look at those alone.
    fn perp_asset_map(&mut self, key: &Pubkey) -> anyhow::Result<RawAccount> {
        self.accounts(std::slice::from_ref(key))?
            .pop()
            .flatten()
            .ok_or_else(|| anyhow::anyhow!("perp asset map {key} missing"))
    }

    /// The chain's clock (unix seconds).
    fn now(&mut self) -> anyhow::Result<i64>;

    fn rent_exempt_minimum(&mut self, len: usize) -> anyhow::Result<u64>;

    /// Runs a read-only instruction (a Hawkeye view) without committing it and returns what it
    /// returned. Solana trims trailing zeros from return data; the caller pads it.
    fn view(&mut self, ix: Instruction) -> anyhow::Result<Vec<u8>>;

    /// The vault's live withdrawal requests (ids below `next_id`).
    fn withdraw_requests(
        &mut self,
        vault: &Pubkey,
        next_id: u64,
    ) -> anyhow::Result<Vec<(Pubkey, WithdrawRequest)>>;

    /// Phoenix's global trader index and active trader buffer accounts. They change as Phoenix
    /// grows: Phoenix publishes them.
    fn phoenix_index(&mut self) -> anyhow::Result<(Vec<Pubkey>, Vec<Pubkey>)>;

    /// Signs with the keeper's key and sends; returns the signature once confirmed.
    fn send(
        &mut self,
        ixs: Vec<Instruction>,
        lookup_tables: &[Pubkey],
        label: &str,
    ) -> anyhow::Result<String>;

    /// A route selling `amount` of `input` for `output`, with `authority` as the trader, in at
    /// most `max_accounts` accounts.
    fn route(
        &mut self,
        authority: &Pubkey,
        input: &Pubkey,
        output: &Pubkey,
        amount: u64,
        slippage_bps: u16,
        max_accounts: u32,
    ) -> anyhow::Result<Route>;

    /// A Phoenix market's funding, by its symbol.
    fn funding(&mut self, symbol: &str) -> anyhow::Result<Funding>;

    /// Tells the operator something needs a look.
    fn alert(&mut self, message: &str) {
        eprintln!("ALERT: {message}");
    }
}
