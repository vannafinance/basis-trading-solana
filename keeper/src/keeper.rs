//! The keeper's loop. Each tick it reads the vault, refreshes funding when it is due, raises
//! alerts, then plans and acts one move at a time, reading the vault again after each: every
//! decision sees what the last one did. A move that fails cools down (urgent ones briefly) and
//! is retried at half the size; the planner offers the next alternative meanwhile, so a market
//! with no route, or a limit the program refuses, does not stop the rest.

use anchor_lang::prelude::Pubkey;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use std::collections::HashMap;

use crate::actions;
use crate::backend::Backend;
use crate::config::Policy;
use crate::funding::Funding;
use crate::observe::{observe, Observation};
use crate::policy::{Decision, Memory, Planner};

/// Compute for one vault instruction: a deploy across eight legs takes about 650k.
const COMPUTE_UNITS: u32 = 1_400_000;
/// The same alert is sent again after this long (seconds).
const ALERT_REPEAT_SECS: i64 = 3_600;

#[derive(Clone, Debug)]
pub struct Settings {
    pub vault_id: u16,
    pub policy: Policy,
    pub dry_run: bool,
    /// The vault's own lookup tables, added to every transaction.
    pub lookup_tables: Vec<Pubkey>,
    pub funding_refresh_secs: i64,
    pub priority_fee_micro_lamports: u64,
    /// Phoenix symbols by asset id, where the perp asset map's name is not the API's.
    pub symbols: HashMap<u32, String>,
}

impl Settings {
    pub fn new(vault_id: u16, policy: Policy) -> Self {
        Self {
            vault_id,
            policy,
            dry_run: false,
            lookup_tables: vec![],
            funding_refresh_secs: 900,
            priority_fee_micro_lamports: 0,
            symbols: HashMap::new(),
        }
    }
}

/// What one tick did.
#[derive(Debug, Default)]
pub struct Report {
    /// Moves sent and confirmed, with their signatures.
    pub executed: Vec<(Decision, String)>,
    /// Moves that failed to build or send, with why.
    pub failed: Vec<(Decision, String)>,
    /// In a dry run: everything the keeper would do now, most urgent first.
    pub planned: Vec<Decision>,
    pub alerts: Vec<String>,
}

/// URLs cut to their scheme and host: RPC and webhook URLs carry API keys, and errors repeat
/// them.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("http") {
        let (before, url) = rest.split_at(at);
        out.push_str(before);
        let end = url
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | '>' | ','))
            .unwrap_or(url.len());
        let (url, after) = url.split_at(end);
        match url.find("://") {
            Some(scheme) => {
                let host_end = url[scheme + 3..]
                    .find(['/', '?', '#'])
                    .map_or(url.len(), |i| scheme + 3 + i);
                out.push_str(&url[..host_end]);
                if host_end < url.len() {
                    out.push_str("/…");
                }
            }
            None => out.push_str(url),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

pub fn log(message: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    eprintln!("[{now}] {}", redact(message));
}

pub struct Keeper<B: Backend> {
    pub backend: B,
    pub settings: Settings,
    /// Funding by Phoenix symbol, as last read.
    pub funding: HashMap<String, Funding>,
    funding_read_at: Option<i64>,
    /// Moves that failed, by `Decision::key`: when they may be tried again.
    cooldowns: HashMap<String, i64>,
    /// How many times in a row each move has failed.
    failures: HashMap<String, u32>,
    /// Alerts sent, by key, and when.
    alerted: HashMap<String, i64>,
}

impl<B: Backend> Keeper<B> {
    pub fn new(backend: B, settings: Settings) -> Self {
        Self {
            backend,
            settings,
            funding: HashMap::new(),
            funding_read_at: None,
            cooldowns: HashMap::new(),
            failures: HashMap::new(),
            alerted: HashMap::new(),
        }
    }

    pub fn observe(&mut self) -> anyhow::Result<Observation> {
        observe(&mut self.backend, self.settings.vault_id, &self.settings.symbols)
    }

    /// Reads every live leg's funding. A market whose read fails keeps its last reading, which
    /// goes stale (and is no longer acted on) after `max_funding_age_secs`.
    pub fn refresh_funding(&mut self, obs: &Observation) {
        let mut symbols: Vec<&str> = obs
            .legs
            .iter()
            .filter(|l| l.live())
            .map(|l| l.symbol.as_str())
            .collect();
        symbols.sort_unstable();
        symbols.dedup();
        for symbol in symbols {
            match self.backend.funding(symbol) {
                Ok(f) => {
                    self.funding.insert(symbol.to_string(), f);
                }
                Err(e) => log(&format!("funding for {symbol}: {e:#}")),
            }
        }
        self.funding_read_at = Some(obs.now);
    }

    /// Sends `message` unless the same `key` was alerted within the hour.
    fn alert(&mut self, now: i64, key: &str, message: &str) {
        self.alerted.retain(|_, at| now - *at < 86_400);
        let due = self
            .alerted
            .get(key)
            .is_none_or(|at| now - at >= ALERT_REPEAT_SECS);
        if due {
            self.alerted.insert(key.to_string(), now);
            self.backend.alert(&redact(message));
        }
    }

    /// The moves cooling down now, and their failure counts, for the planner.
    pub fn memory(&self, now: i64) -> Memory {
        Memory {
            cooling: self
                .cooldowns
                .iter()
                .filter(|(_, until)| now < **until)
                .map(|(k, _)| k.clone())
                .collect(),
            failures: self.failures.clone(),
        }
    }

    /// Builds and sends one decision; returns its signature.
    pub fn execute(&mut self, decision: &Decision, obs: &Observation) -> anyhow::Result<String> {
        // A rotation sells only once the market it buys into has a route: otherwise the
        // keeper would sell, fail to buy, and buy back what it sold.
        if let Some((mint, usdc)) = decision.into {
            self.backend
                .route(
                    &obs.vault.authority,
                    &obs.config.usdc_mint,
                    &mint,
                    usdc.max(1),
                    obs.vault.slippage_bps,
                    40,
                )
                .map_err(|e| anyhow::anyhow!("no route into {mint}: {e:#}"))?;
        }
        let built = actions::build(&decision.action, obs, &mut self.backend)?;
        let mut ixs = vec![ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNITS)];
        if self.settings.priority_fee_micro_lamports > 0 {
            ixs.push(ComputeBudgetInstruction::set_compute_unit_price(
                self.settings.priority_fee_micro_lamports,
            ));
        }
        ixs.extend(built.ixs);
        let mut tables = self.settings.lookup_tables.clone();
        for table in built.lookup_tables {
            if !tables.contains(&table) {
                tables.push(table);
            }
        }
        self.backend.send(ixs, &tables, decision.action.name())
    }

    /// One pass: look, decide, act, up to `max_actions_per_tick` moves.
    pub fn tick(&mut self) -> anyhow::Result<Report> {
        let mut report = Report::default();
        for step in 0..self.settings.policy.max_actions_per_tick.max(1) {
            let obs = self.observe()?;
            if step == 0 {
                let due = self
                    .funding_read_at
                    .is_none_or(|at| obs.now - at >= self.settings.funding_refresh_secs);
                if due {
                    self.refresh_funding(&obs);
                }
                log(&obs.summary());
                let alerts = Planner::new(&obs, &self.funding, &self.settings.policy).health();
                for (key, message) in &alerts {
                    self.alert(obs.now, key, message);
                }
                report.alerts = alerts.into_iter().map(|(_, m)| m).collect();
            }
            let memory = self.memory(obs.now);
            let plan = Planner::new(&obs, &self.funding, &self.settings.policy)
                .with_memory(&memory)
                .plan();
            if self.settings.dry_run {
                for d in &plan {
                    log(&format!("would {}: {:?} ({})", d.action.name(), d.action, d.reason));
                }
                report.planned = plan;
                break;
            }
            let Some(decision) = plan.into_iter().next() else {
                break;
            };
            log(&format!("{} ({}): {}", decision.action.name(), decision.rule, decision.reason));
            let key = decision.key();
            match self.execute(&decision, &obs) {
                Ok(signature) => {
                    log(&format!("  sent {signature}"));
                    self.failures.remove(&key);
                    self.cooldowns.remove(&key);
                    report.executed.push((decision, signature));
                }
                Err(e) => {
                    let why = redact(&format!("{e:#}"));
                    log(&format!("  failed: {why}"));
                    let policy = &self.settings.policy;
                    let base = if decision.urgent() {
                        policy.urgent_cooldown_secs
                    } else {
                        policy.failure_cooldown_secs
                    };
                    let failures = self.failures.entry(key.clone()).or_default();
                    *failures += 1;
                    // Twice as long after each failure in a row, up to a day.
                    let cooldown = (base << (*failures - 1).min(8)).min(86_400);
                    self.cooldowns.insert(key.clone(), obs.now + cooldown);
                    self.alert(
                        obs.now,
                        &format!("failed:{key}"),
                        &format!("{} failed ({}): {}", decision.action.name(), decision.reason, first_line(&why)),
                    );
                    report.failed.push((decision, why));
                }
            }
        }
        Ok(report)
    }

    /// Ticks forever, `tick_secs` apart. A tick that fails (an RPC outage) is logged and
    /// alerted (once an hour); the next one starts fresh.
    pub fn run(&mut self, tick_secs: u64) -> ! {
        loop {
            if let Err(e) = self.tick() {
                let why = format!("tick failed: {e:#}");
                log(&why);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                self.alert(now, "tick", first_line(&why));
            }
            std::thread::sleep(std::time::Duration::from_secs(tick_secs));
        }
    }
}

fn first_line(text: &str) -> &str {
    let line = text.lines().next().unwrap_or(text);
    let mut end = line.len().min(300);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    &line[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_lose_their_keys() {
        assert_eq!(
            redact("error sending request for url (https://rpc.example.com/?api-key=SECRET): timeout"),
            "error sending request for url (https://rpc.example.com/…): timeout"
        );
        assert_eq!(
            redact("POST https://hooks.slack.com/services/T0/B0/XYZ failed"),
            "POST https://hooks.slack.com/… failed"
        );
        assert_eq!(redact("no urls here"), "no urls here");
    }
}
