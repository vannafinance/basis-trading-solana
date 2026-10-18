//! The keeper against a real cluster: Solana JSON-RPC for accounts, views and transactions,
//! Phoenix's API for funding and its index accounts, Jupiter's for routes.

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::AccountDeserialize;
use anyhow::{anyhow, bail, Context};
use base64::Engine;
use serde_json::{json, Value};
use solana_account_decoder_client_types::{UiAccount, UiAccountEncoding, UiDataSliceConfig};
use solana_address_lookup_table_interface::state::AddressLookupTable;
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_message::{v0, AddressLookupTableAccount, VersionedMessage};
use solana_rpc_client::rpc_client::RpcClient;
use solana_rpc_client_api::config::{RpcAccountInfoConfig, RpcSimulateTransactionConfig};
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use solana_transaction::Transaction;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::{Duration, Instant};
use vanna_phoenix_multi_vault::constants::JUPITER_PROGRAM_ID;
use vanna_phoenix_multi_vault::state::{request_address, WithdrawRequest};
use vanna_phoenix_multi_vault::swap::SwapRoute;

use crate::backend::{Backend, RawAccount, Route};
use crate::config::KeeperConfig;
use crate::funding::Funding;
use crate::keeper::log;

const CLOCK: Pubkey = anchor_lang::prelude::pubkey!("SysvarC1ock11111111111111111111111111111111");
/// Phoenix's index accounts are read again after this long.
const INDEX_REFRESH: Duration = Duration::from_secs(600);
/// Lookup tables are read again after this long (a table may be extended).
const TABLE_REFRESH: Duration = Duration::from_secs(600);
/// Phoenix's perp asset map: a 48-byte header (slots in use at 32), then 1,024 slots of 1,584.
const ASSET_MAP_HEADER: usize = 48;
const ASSET_SLOT: usize = 1_584;
const ASSET_MAP_LEN: usize = ASSET_MAP_HEADER + 1_024 * ASSET_SLOT;
/// Jupiter v6's `route` and `shared_accounts_route`: the routes the vault resizes.
const JUPITER_ROUTES: [[u8; 8]; 2] = [
    [229, 23, 203, 151, 122, 227, 173, 42],
    [193, 32, 155, 51, 65, 214, 156, 129],
];

pub struct LiveBackend {
    rpc: RpcClient,
    http: reqwest::blocking::Client,
    keypair: Keypair,
    phoenix_api: String,
    jupiter_api: String,
    jupiter_api_key: Option<String>,
    jupiter_max_accounts: u32,
    alert_webhook: Option<String>,
    tables: HashMap<Pubkey, (Instant, AddressLookupTableAccount)>,
    rent: HashMap<usize, u64>,
    index: Option<(Instant, Vec<Pubkey>, Vec<Pubkey>)>,
    /// Requests below this id are all closed (or never existed): scanning starts here.
    request_floor: HashMap<Pubkey, u64>,
    /// The slot of the last confirmed send: reads must be at least this fresh, so the keeper
    /// never decides on a node that has not seen its own last move.
    min_slot: Option<u64>,
}

/// A Solana keypair JSON file (64 numbers).
pub fn read_keypair(path: &str) -> anyhow::Result<Keypair> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
    let bytes: Vec<u8> = serde_json::from_str(&text).with_context(|| format!("{path} is not a keypair file"))?;
    Keypair::try_from(bytes.as_slice()).map_err(|e| anyhow!("{path}: {e}"))
}

fn raw(account: &UiAccount) -> anyhow::Result<RawAccount> {
    Ok(RawAccount {
        owner: Pubkey::from_str(&account.owner).map_err(|e| anyhow!("owner {}: {e}", account.owner))?,
        lamports: account.lamports,
        data: account.data.decode().context("account data encoding")?,
    })
}

fn pubkey(value: &Value) -> anyhow::Result<Pubkey> {
    let text = value.as_str().context("expected an address")?;
    Pubkey::from_str(text).map_err(|e| anyhow!("{text}: {e}"))
}

fn pubkeys(value: &Value) -> anyhow::Result<Vec<Pubkey>> {
    value
        .as_array()
        .context("expected a list of addresses")?
        .iter()
        .map(pubkey)
        .collect()
}

impl LiveBackend {
    pub fn new(config: &KeeperConfig) -> anyhow::Result<Self> {
        Ok(Self {
            rpc: RpcClient::new_with_timeout_and_commitment(
                config.rpc_url.clone(),
                Duration::from_secs(60),
                CommitmentConfig::confirmed(),
            ),
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent(concat!("vanna-keeper/", env!("CARGO_PKG_VERSION")))
                .build()?,
            keypair: read_keypair(&config.keypair_path)?,
            phoenix_api: config.phoenix_api.trim_end_matches('/').to_string(),
            jupiter_api: config.jupiter_api.trim_end_matches('/').to_string(),
            jupiter_api_key: config.jupiter_api_key.clone(),
            jupiter_max_accounts: config.jupiter_max_accounts,
            alert_webhook: config.alert_webhook.clone(),
            tables: HashMap::new(),
            rent: HashMap::new(),
            index: None,
            request_floor: HashMap::new(),
            min_slot: None,
        })
    }

    fn read_config(&self, slice: Option<(usize, usize)>) -> RpcAccountInfoConfig {
        RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            data_slice: slice.map(|(offset, length)| UiDataSliceConfig { offset, length }),
            commitment: Some(CommitmentConfig::confirmed()),
            min_context_slot: self.min_slot,
        }
    }

    fn sliced(&self, key: &Pubkey, offset: usize, length: usize) -> anyhow::Result<RawAccount> {
        let account = self
            .rpc
            .get_ui_account_with_config(key, self.read_config(Some((offset, length))))?
            .value
            .with_context(|| format!("{key} missing"))?;
        raw(&account)
    }

    fn get_json(&self, url: &str, query: &[(&str, String)], jupiter: bool) -> anyhow::Result<Value> {
        let mut request = self.http.get(url).query(query);
        if jupiter {
            if let Some(key) = &self.jupiter_api_key {
                request = request.header("x-api-key", key);
            }
        }
        let response = request.send().with_context(|| format!("GET {url}"))?;
        let status = response.status();
        let body: Value = response.json().with_context(|| format!("GET {url}: not JSON"))?;
        if !status.is_success() {
            bail!("GET {url}: {status} {body}");
        }
        Ok(body)
    }

    fn lookup_table(&mut self, key: &Pubkey) -> anyhow::Result<AddressLookupTableAccount> {
        if let Some((at, table)) = self.tables.get(key) {
            if at.elapsed() < TABLE_REFRESH {
                return Ok(table.clone());
            }
        }
        let account = self.rpc.get_account(key).with_context(|| format!("lookup table {key}"))?;
        let table = AddressLookupTable::deserialize(&account.data)
            .map_err(|e| anyhow!("lookup table {key}: {e:?}"))?;
        let table = AddressLookupTableAccount {
            key: *key,
            addresses: table.addresses.to_vec(),
        };
        self.tables.insert(*key, (Instant::now(), table.clone()));
        Ok(table)
    }
}

impl Backend for LiveBackend {
    fn keeper(&self) -> Pubkey {
        self.keypair.pubkey()
    }

    fn accounts(&mut self, keys: &[Pubkey]) -> anyhow::Result<Vec<Option<RawAccount>>> {
        let mut out = Vec::with_capacity(keys.len());
        for chunk in keys.chunks(100) {
            let found = self
                .rpc
                .get_multiple_ui_accounts_with_config(chunk, self.read_config(None))?
                .value;
            for account in found {
                out.push(account.as_ref().map(raw).transpose()?);
            }
        }
        Ok(out)
    }

    /// Only the slots Phoenix uses (about 160 KB of 1.6 MB), the rest left zero.
    fn perp_asset_map(&mut self, key: &Pubkey) -> anyhow::Result<RawAccount> {
        let header = self.sliced(key, 0, ASSET_MAP_HEADER)?;
        anyhow::ensure!(header.data.len() == ASSET_MAP_HEADER, "perp asset map header");
        let slots = u32::from_le_bytes(header.data[32..36].try_into().unwrap()) as usize;
        anyhow::ensure!(slots <= 1_024, "perp asset map: {slots} slots");
        let used = self.sliced(key, 0, ASSET_MAP_HEADER + slots * ASSET_SLOT)?;
        let mut data = used.data;
        data.resize(ASSET_MAP_LEN, 0);
        Ok(RawAccount {
            owner: used.owner,
            lamports: used.lamports,
            data,
        })
    }

    fn now(&mut self) -> anyhow::Result<i64> {
        let clock = self.rpc.get_account(&CLOCK)?;
        anyhow::ensure!(clock.data.len() >= 40, "clock sysvar too short");
        Ok(i64::from_le_bytes(clock.data[32..40].try_into().unwrap()))
    }

    fn rent_exempt_minimum(&mut self, len: usize) -> anyhow::Result<u64> {
        if let Some(lamports) = self.rent.get(&len) {
            return Ok(*lamports);
        }
        let lamports = self.rpc.get_minimum_balance_for_rent_exemption(len)?;
        self.rent.insert(len, lamports);
        Ok(lamports)
    }

    fn view(&mut self, ix: Instruction) -> anyhow::Result<Vec<u8>> {
        let tx = Transaction::new_with_payer(&[ix], Some(&self.keypair.pubkey()));
        let config = RpcSimulateTransactionConfig {
            sig_verify: false,
            replace_recent_blockhash: true,
            commitment: Some(CommitmentConfig::confirmed()),
            min_context_slot: self.min_slot,
            ..Default::default()
        };
        let result = self.rpc.simulate_transaction_with_config(&tx, config)?.value;
        if let Some(err) = result.err {
            bail!("view failed: {err:?}: {:?}", result.logs.unwrap_or_default());
        }
        let data = result.return_data.map(|r| r.data.0).unwrap_or_default();
        Ok(base64::engine::general_purpose::STANDARD.decode(data)?)
    }

    /// Every request PDA from the oldest still open up to `next_id`, read in batches: no
    /// getProgramAccounts, which public RPCs refuse or rate-limit.
    fn withdraw_requests(
        &mut self,
        vault: &Pubkey,
        next_id: u64,
    ) -> anyhow::Result<Vec<(Pubkey, WithdrawRequest)>> {
        let floor = self.request_floor.get(vault).copied().unwrap_or(0).min(next_id);
        let mut out = vec![];
        let mut lowest_open = None;
        let ids: Vec<u64> = (floor..next_id).collect();
        for chunk in ids.chunks(100) {
            let keys: Vec<Pubkey> = chunk.iter().map(|id| request_address(vault, *id).0).collect();
            for ((id, key), account) in chunk.iter().zip(&keys).zip(self.accounts(&keys)?) {
                let Some(account) = account else { continue };
                if account.owner != vanna_phoenix_multi_vault::ID {
                    continue;
                }
                let request = WithdrawRequest::try_deserialize(&mut &account.data[..])
                    .map_err(|e| anyhow!("request {key}: {e}"))?;
                lowest_open.get_or_insert(*id);
                out.push((*key, request));
            }
        }
        self.request_floor.insert(*vault, lowest_open.unwrap_or(next_id));
        Ok(out)
    }

    fn phoenix_index(&mut self) -> anyhow::Result<(Vec<Pubkey>, Vec<Pubkey>)> {
        if let Some((at, gti, atb)) = &self.index {
            if at.elapsed() < INDEX_REFRESH {
                return Ok((gti.clone(), atb.clone()));
            }
        }
        let fetched = self
            .get_json(&format!("{}/v1/view/exchange/keys", self.phoenix_api), &[], false)
            .and_then(|keys| {
                let gti = pubkeys(&keys["globalTraderIndex"])?;
                let atb = pubkeys(&keys["activeTraderBuffer"])?;
                anyhow::ensure!(!gti.is_empty() && !atb.is_empty(), "Phoenix lists no index accounts");
                Ok((gti, atb))
            });
        match (fetched, &self.index) {
            (Ok((gti, atb)), _) => {
                self.index = Some((Instant::now(), gti.clone(), atb.clone()));
                Ok((gti, atb))
            }
            // Phoenix's API down: the accounts it last listed almost always still are them, and
            // Phoenix checks them in every call anyway.
            (Err(e), Some((_, gti, atb))) => {
                log(&format!("Phoenix index accounts: {e:#}; using the last ones"));
                Ok((gti.clone(), atb.clone()))
            }
            (Err(e), None) => Err(e),
        }
    }

    fn send(
        &mut self,
        ixs: Vec<Instruction>,
        lookup_tables: &[Pubkey],
        label: &str,
    ) -> anyhow::Result<String> {
        let tables = lookup_tables
            .iter()
            .map(|k| self.lookup_table(k))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let blockhash = self.rpc.get_latest_blockhash()?;
        let message = v0::Message::try_compile(&self.keypair.pubkey(), &ixs, &tables, blockhash)
            .map_err(|e| anyhow!("{label}: {e:?}"))?;
        let tx = VersionedTransaction::try_new(VersionedMessage::V0(message), &[&self.keypair])
            .map_err(|e| anyhow!("{label}: signing: {e:?}"))?;
        let sent = self.rpc.send_and_confirm_transaction(&tx);
        let signature = match sent {
            Ok(signature) => signature,
            Err(e) => {
                // A table that changed or closed shows up as a send failure: read them again.
                self.tables.clear();
                return Err(anyhow::Error::from(e).context(label.to_string()));
            }
        };
        self.min_slot = self.rpc.get_slot().ok();
        Ok(signature.to_string())
    }

    fn route(
        &mut self,
        authority: &Pubkey,
        input: &Pubkey,
        output: &Pubkey,
        amount: u64,
        slippage_bps: u16,
        max_accounts: u32,
    ) -> anyhow::Result<Route> {
        let max_accounts = max_accounts.min(self.jupiter_max_accounts);
        let quote = self.get_json(
            &format!("{}/quote", self.jupiter_api),
            &[
                ("inputMint", input.to_string()),
                ("outputMint", output.to_string()),
                ("amount", amount.to_string()),
                ("slippageBps", slippage_bps.to_string()),
                ("maxAccounts", max_accounts.to_string()),
                ("swapMode", "ExactIn".to_string()),
            ],
            true,
        )?;
        let out_amount: u64 = quote["outAmount"]
            .as_str()
            .context("Jupiter quote without outAmount")?
            .parse()?;
        // The vault authority trades: its own token accounts on both sides, no wrapping (the
        // program wraps SOL itself) and no setup instructions.
        let body = json!({
            "quoteResponse": quote,
            "userPublicKey": authority.to_string(),
            "wrapAndUnwrapSol": false,
            "useSharedAccounts": true,
            "skipUserAccountsRpcCalls": true,
        });
        let mut request = self
            .http
            .post(format!("{}/swap-instructions", self.jupiter_api))
            .json(&body);
        if let Some(key) = &self.jupiter_api_key {
            request = request.header("x-api-key", key);
        }
        let response: Value = request.send()?.json()?;
        if let Some(error) = response.get("error") {
            bail!("Jupiter: {error}");
        }
        let swap = &response["swapInstruction"];
        anyhow::ensure!(
            pubkey(&swap["programId"])? == JUPITER_PROGRAM_ID,
            "Jupiter routed through another program"
        );
        let data = base64::engine::general_purpose::STANDARD
            .decode(swap["data"].as_str().context("route without data")?)?;
        anyhow::ensure!(
            data.len() >= 8 && JUPITER_ROUTES.iter().any(|d| data[..8] == d[..]),
            "Jupiter returned a route the vault cannot size"
        );
        let metas = swap["accounts"]
            .as_array()
            .context("route without accounts")?
            .iter()
            .map(|a| {
                Ok(AccountMeta {
                    pubkey: pubkey(&a["pubkey"])?,
                    // The vault authority signs inside the program; nothing signs here.
                    is_signer: false,
                    is_writable: a["isWritable"].as_bool().unwrap_or(false),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        anyhow::ensure!(metas.len() <= u8::MAX as usize, "route too long");
        let lookup_tables = match response.get("addressLookupTableAddresses") {
            Some(v) if !v.is_null() => pubkeys(v)?,
            _ => vec![],
        };
        Ok(Route {
            args: SwapRoute {
                accounts: metas.len() as u8,
                data,
            },
            metas,
            out_amount,
            lookup_tables,
        })
    }

    fn funding(&mut self, symbol: &str) -> anyhow::Result<Funding> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as i64;
        let body = self.get_json(
            &format!("{}/v1/funding/{symbol}/rates", self.phoenix_api),
            &[
                ("startTime", (now_ms - 30 * 86_400_000).to_string()),
                ("limit", "1000".to_string()),
            ],
            false,
        )?;
        let points = body["rates"]
            .as_array()
            .context("no rates")?
            .iter()
            .filter_map(|p| {
                // Numbers or numeric strings, whichever the API sends.
                let num = |v: &Value| v.as_f64().or_else(|| v.as_str()?.parse::<f64>().ok());
                Some((num(&p["timestamp"])? as i64, num(&p["fundingRatePercentage"])?))
            })
            .collect::<Vec<_>>();
        Funding::from_rates(&points).with_context(|| format!("no funding rates for {symbol}"))
    }

    fn alert(&mut self, message: &str) {
        log(&format!("ALERT: {message}"));
        if let Some(url) = &self.alert_webhook {
            let text = format!("vanna-keeper: {message}");
            if let Err(e) = self
                .http
                .post(url)
                .json(&json!({ "text": text, "content": text }))
                .send()
            {
                log(&format!("alert webhook: {e}"));
            }
        }
    }
}
