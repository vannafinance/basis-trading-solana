//! What the keeper runs against and the policy it follows, read from a TOML file
//! (`keeper.example.toml`). Every policy field has a default.

use anchor_lang::prelude::Pubkey;
use serde::Deserialize;
use std::collections::HashMap;
use std::str::FromStr;

/// The keeper's decision thresholds.
///
/// Rates are capital APRs in percent (8.0 is 8% a year): what a dollar placed in the leg earns,
/// its funding spread over the asset and the margin it takes. A spot leg at 1x margin earns its
/// funding on half its capital; the SOL leg (its own collateral) on all of it. USD amounts are
/// whole dollars; bps are basis points (100 = 1%).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Policy {
    /// A leg is worth entering when its capital APR (on the mean of its 7- and 30-day funding)
    /// is at least this and its 7-day funding is positive.
    pub entry_apr: f64,
    /// A leg is left (unwound to USDC in the vault) when its capital APR falls below this. The
    /// gap to `entry_apr` is the hysteresis: a leg is not bought and sold on every wobble.
    pub exit_apr: f64,
    /// Rotate out of a held leg into a better one only when the better one pays at least this
    /// much more. A rotation costs 0.2–0.6% of what moves; 8% more pays that back in weeks.
    pub rotate_gap_apr: f64,
    /// Let the keeper open and close deposits (the funding guard). Off when the admin manages
    /// them by hand: the keeper would otherwise reopen deposits the admin closed.
    pub manage_deposits: bool,
    /// Close deposits when no active leg pays at least this...
    pub close_deposits_apr: f64,
    /// ...and reopen them once one pays at least this.
    pub reopen_deposits_apr: f64,
    /// The share of a market's open interest the keeper fills a leg's short to. Above ~25% the
    /// vault pushes the funding it earns down on itself.
    pub max_oi_share_bps: u64,
    /// Trim a leg back to `max_oi_share_bps` once its short passes this share (the market's
    /// open interest fell).
    pub trim_oi_share_bps: u64,
    /// The most of the vault's NAV the keeper puts in one leg (the leg's own cap still applies).
    pub max_leg_nav_share_bps: u64,
    /// Phoenix collateral the keeper keeps per dollar of a spot leg's short: 10,000 is 1x.
    pub spot_margin_bps: u64,
    /// Phoenix collateral kept per dollar of the SOL leg's short, its own SOL counted.
    pub sol_margin_bps: u64,
    /// What Phoenix counts a dollar of native SOL as, in collateral (80% on mainnet).
    pub sol_collateral_weight_bps: u64,
    /// The margin target is never below the vault's `margin_bps` plus this.
    pub margin_headroom_bps: u64,
    /// Restore the margin at once (from the buffer, or by selling a spot leg) when it falls
    /// within this of the vault's `margin_bps`.
    pub margin_alarm_bps: u64,
    /// Top the margin up from the free buffer once it is this far below target...
    pub margin_topup_bps: u64,
    /// ...and bring it back to the buffer once it is this far above.
    pub margin_release_bps: u64,
    /// Re-hedge a leg whose short is off its asset by more than this share (and a lot).
    pub hedge_tolerance_bps: u64,
    /// Smallest trade worth making (USD).
    pub min_trade_usd: u64,
    /// Largest single deploy or unwind (USD): what Phoenix's book and Jupiter take near the mark.
    pub max_trade_usd: u64,
    /// USDC kept idle above the vault's own reserve (USD), so small withdrawals leave at once.
    pub buffer_slack_usd: u64,
    /// Refill the buffer (back to the reserve plus the slack) once instant withdrawals take
    /// idle USDC under this share of the reserve.
    pub buffer_refill_bps: u64,
    /// Actions one tick may take; it looks at the vault again before each.
    pub max_actions_per_tick: usize,
    /// How long an action that failed is not tried again (seconds)...
    pub failure_cooldown_secs: i64,
    /// ...or, for one that defends the vault or pays its holders (margin alarm, re-hedge,
    /// requests, buffer refill), this long.
    pub urgent_cooldown_secs: i64,
    /// Funding older than this is not acted on (seconds).
    pub max_funding_age_secs: i64,
    /// Alert when a withdrawal request has waited this long (seconds).
    pub request_alert_secs: i64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            entry_apr: 6.0,
            exit_apr: 2.0,
            rotate_gap_apr: 8.0,
            manage_deposits: true,
            close_deposits_apr: 3.0,
            reopen_deposits_apr: 6.0,
            max_oi_share_bps: 2_500,
            trim_oi_share_bps: 3_500,
            max_leg_nav_share_bps: 5_000,
            spot_margin_bps: 10_000,
            sol_margin_bps: 8_000,
            sol_collateral_weight_bps: 8_000,
            margin_headroom_bps: 2_000,
            margin_alarm_bps: 1_000,
            margin_topup_bps: 1_000,
            margin_release_bps: 2_000,
            hedge_tolerance_bps: 50,
            min_trade_usd: 500,
            max_trade_usd: 50_000,
            buffer_slack_usd: 1_000,
            buffer_refill_bps: 5_000,
            max_actions_per_tick: 8,
            failure_cooldown_secs: 600,
            urgent_cooldown_secs: 60,
            max_funding_age_secs: 3 * 3_600,
            request_alert_secs: 6 * 3_600,
        }
    }
}

/// The keeper's deployment: where it reads, what it signs with, how often it ticks.
#[derive(Clone, Debug, Deserialize)]
pub struct KeeperConfig {
    pub rpc_url: String,
    /// A Solana keypair JSON file: the config's `keeper`. Keep it in a KMS or HSM in
    /// production; the program bounds what it can do (price band, turnover) but it still moves
    /// the vault.
    pub keypair_path: String,
    /// The vault's `id` (part of its address).
    pub vault_id: u16,
    /// The vault's address lookup tables: a deploy or unwind does not fit a transaction without
    /// one.
    #[serde(default)]
    pub lookup_tables: Vec<String>,
    /// Phoenix's API: funding rates and the exchange's index accounts.
    #[serde(default = "default_phoenix_api")]
    pub phoenix_api: String,
    /// Jupiter's swap API (`/quote` and `/swap-instructions` under it).
    #[serde(default = "default_jupiter_api")]
    pub jupiter_api: String,
    #[serde(default)]
    pub jupiter_api_key: Option<String>,
    /// Route size limit for Jupiter: a deploy names every leg too, and Solana allows 64.
    #[serde(default = "default_max_accounts")]
    pub jupiter_max_accounts: u32,
    /// Phoenix symbols by asset id, where the perp asset map's own name is not the API's.
    #[serde(default)]
    pub symbols: HashMap<u32, String>,
    /// How often the keeper looks at the vault: margin, hedges, requests, the buffer.
    #[serde(default = "default_tick")]
    pub tick_secs: u64,
    /// How often it reads funding again (Phoenix pays it hourly).
    #[serde(default = "default_funding_refresh")]
    pub funding_refresh_secs: i64,
    /// Decide and log, send nothing.
    #[serde(default)]
    pub dry_run: bool,
    /// Optional webhook for alerts: Slack (`text`) and Discord (`content`) both read the body.
    #[serde(default)]
    pub alert_webhook: Option<String>,
    /// Priority fee, micro-lamports per compute unit.
    #[serde(default)]
    pub priority_fee_micro_lamports: u64,
    #[serde(default)]
    pub policy: Policy,
}

fn default_phoenix_api() -> String {
    "https://perp-api.phoenix.trade".into()
}
fn default_jupiter_api() -> String {
    "https://lite-api.jup.ag/swap/v1".into()
}
fn default_max_accounts() -> u32 {
    20
}
fn default_tick() -> u64 {
    30
}
fn default_funding_refresh() -> i64 {
    900
}

impl KeeperConfig {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading {path}: {e}"))?;
        Ok(toml::from_str(&text)?)
    }

    pub fn lookup_table_keys(&self) -> anyhow::Result<Vec<Pubkey>> {
        self.lookup_tables
            .iter()
            .map(|k| Pubkey::from_str(k).map_err(|e| anyhow::anyhow!("lookup table {k}: {e}")))
            .collect()
    }

    pub fn settings(&self) -> anyhow::Result<crate::keeper::Settings> {
        Ok(crate::keeper::Settings {
            vault_id: self.vault_id,
            policy: self.policy.clone(),
            dry_run: self.dry_run,
            lookup_tables: self.lookup_table_keys()?,
            funding_refresh_secs: self.funding_refresh_secs,
            priority_fee_micro_lamports: self.priority_fee_micro_lamports,
            symbols: self.symbols.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_config_parses() {
        let text = include_str!("../keeper.example.toml");
        let config: KeeperConfig = toml::from_str(text).unwrap();
        assert_eq!(config.vault_id, 1);
        assert_eq!(config.policy.max_oi_share_bps, 2_500);
        assert!(config.policy.entry_apr > config.policy.exit_apr);
        assert!(config.policy.reopen_deposits_apr > config.policy.close_deposits_apr);
        // Symbol overrides by asset id.
        let text = format!("{text}\n[symbols]\n12 = \"TSLA\"\n");
        let config: KeeperConfig = toml::from_str(&text).unwrap();
        assert_eq!(config.symbols.get(&12).map(String::as_str), Some("TSLA"));
    }
}
