//! The keeper's decisions. `plan` looks at one observation and lists what is worth doing, most
//! urgent first, with the alternatives each rule has (another leg to sell, another market to
//! fill); the keeper does the first that is not cooling down after a failure, looks again,
//! and repeats. Each rule is plain arithmetic on the program's own numbers:
//!
//! 1. **Margin alarm:** the shared margin near the vault's floor: post free USDC, or sell part of
//!    a spot leg (which closes its short too), the largest first.
//! 2. **Re-hedge** a leg whose short no longer matches its asset (liquidation, donation, a stock
//!    token's dividend multiplier), outside multiplier windows.
//! 3. **Withdrawal requests,** oldest first: return one the program cannot pay (its user's USDC
//!    account closed, given away or frozen, or a payout of nothing); pay the next when the
//!    buffer holds it, else raise the USDC (margin above target first, then the lowest-paying
//!    legs, one after another).
//! 4. **Buffer refill** after instant withdrawals drained it: the same way, back to its reserve.
//! 5. **Margin top-up** from the free buffer when it drifts below target.
//! 6. **Funding guard:** close deposits when no active leg with room pays, reopen when one does.
//! 7. **Exits:** unwind (to USDC, in the vault) a leg that stopped paying, was deactivated, or
//!    grew past its share of the market's open interest.
//! 8. **Deploy** free buffer into the best-paying leg with room, margin first.
//! 9. **Rotate** out of a held leg when another pays enough more to cover the trades.
//! 10. **Margin release** back to the buffer when Phoenix holds more than the target.
//!
//! A move that failed is skipped while it cools down, and the next one of its kind is half its
//! size (a market that could not take $50k may take $25k). When nothing applies the plan is
//! empty and the keeper does nothing: legs keep earning, USDC stays in the vault, and every
//! holder can still withdraw.

use anchor_lang::prelude::Pubkey;
use std::collections::{HashMap, HashSet};
use vanna_phoenix_multi_vault::constants::{BPS_SCALE, PHOENIX_COST_CUSHION_BPS};
use vanna_phoenix_multi_vault::math::Price;
use vanna_phoenix_multi_vault::nav::withdrawal_payout;
use vanna_phoenix_multi_vault::state::LegKind;

use crate::config::Policy;
use crate::funding::Funding;
use crate::observe::Observation;

/// One dollar in the program's units (USD, 6 dp).
pub const USD: u128 = 1_000_000;
const BPS: u128 = BPS_SCALE;
/// How far above the vault's margin floor a SOL sale that thins the margin must leave it.
const SOL_SELL_SAFETY_BPS: u128 = 200;
/// Phoenix counts at most this much native SOL per trader account as collateral.
const PHOENIX_NATIVE_SOL_CAP: u64 = 500 * 1_000_000_000;
/// The keeper alerts when its own SOL (fees, rent) falls under this.
const KEEPER_SOL_FLOOR: u64 = 50_000_000;
/// A request whose payment failed this often is returned.
const FULFILL_ATTEMPTS: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    SetDepositsOpen(bool),
    FulfillWithdraw { request: Pubkey, user: Pubkey, id: u64 },
    ReturnWithdraw { request: Pubkey, id: u64 },
    Rehedge { leg: u8 },
    MoveMargin { to_phoenix: bool, amount: u64 },
    Deploy { leg: u8, usdc: u64 },
    Unwind { leg: u8, units: u64 },
}

impl Action {
    /// The move, without its size: what a failure is remembered by (with its rule).
    pub fn key(&self) -> String {
        match self {
            Action::SetDepositsOpen(open) => format!("deposits:{open}"),
            Action::FulfillWithdraw { id, .. } => format!("fulfill:{id}"),
            Action::ReturnWithdraw { id, .. } => format!("return:{id}"),
            Action::Rehedge { leg } => format!("rehedge:{leg}"),
            Action::MoveMargin { to_phoenix, .. } => {
                format!("margin:{}", if *to_phoenix { "in" } else { "out" })
            }
            Action::Deploy { leg, .. } => format!("deploy:{leg}"),
            Action::Unwind { leg, .. } => format!("unwind:{leg}"),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Action::SetDepositsOpen(_) => "set_deposits_open",
            Action::FulfillWithdraw { .. } => "fulfill_withdraw",
            Action::ReturnWithdraw { .. } => "return_withdraw",
            Action::Rehedge { .. } => "rehedge",
            Action::MoveMargin { .. } => "move_margin",
            Action::Deploy { .. } => "deploy",
            Action::Unwind { .. } => "unwind",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Decision {
    /// The rule that made it: "alarm", "rehedge", "requests", "refill", "topup", "guard",
    /// "exit", "trim", "deploy", "rotate", "release".
    pub rule: &'static str,
    pub action: Action,
    pub reason: String,
    /// For a rotation: the market it buys into and roughly how much USDC it brings there. The
    /// keeper checks that market has a route before selling anything.
    pub into: Option<(Pubkey, u64)>,
}

impl Decision {
    /// What a failure cools down: this rule's move of this kind (on this leg). A failed
    /// rotation does not hold up the margin alarm.
    pub fn key(&self) -> String {
        format!("{}/{}", self.rule, self.action.key())
    }

    /// Moves that defend the vault or pay its holders: retried sooner after a failure.
    pub fn urgent(&self) -> bool {
        matches!(self.rule, "alarm" | "rehedge" | "requests" | "refill")
    }
}

/// What the keeper remembers between ticks: moves cooling down after a failure, and how many
/// times in a row each has failed.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    pub cooling: HashSet<String>,
    pub failures: HashMap<String, u32>,
}

fn usd(v: u128) -> String {
    format!("${:.2}", v as f64 / USD as f64)
}

/// The rules over one observation.
pub struct Planner<'a> {
    pub obs: &'a Observation,
    pub funding: &'a HashMap<String, Funding>,
    pub policy: &'a Policy,
    memory: Option<&'a Memory>,
}

impl<'a> Planner<'a> {
    pub fn new(obs: &'a Observation, funding: &'a HashMap<String, Funding>, policy: &'a Policy) -> Self {
        Self {
            obs,
            funding,
            policy,
            memory: None,
        }
    }

    pub fn with_memory(mut self, memory: &'a Memory) -> Self {
        self.memory = Some(memory);
        self
    }

    // ------------------------------------------------------------ memory

    fn cooling(&self, rule: &str, action: &Action) -> bool {
        self.memory
            .is_some_and(|m| m.cooling.contains(&format!("{rule}/{}", action.key())))
    }

    fn failures(&self, rule: &str, action: &Action) -> u32 {
        self.memory
            .and_then(|m| m.failures.get(&format!("{rule}/{}", action.key())).copied())
            .unwrap_or(0)
    }

    /// `amount` halved for each failure in a row of this move.
    fn shrink(&self, rule: &str, action: &Action, amount: u128) -> u128 {
        amount >> self.failures(rule, action).min(6)
    }

    // ------------------------------------------------------------ numbers

    fn usd_policy(&self, dollars: u64) -> u128 {
        dollars as u128 * USD
    }

    fn min_trade(&self) -> u128 {
        self.usd_policy(self.policy.min_trade_usd)
    }

    fn max_trade(&self) -> u128 {
        self.usd_policy(self.policy.max_trade_usd)
    }

    fn kind(&self, i: usize) -> LegKind {
        self.obs.legs[i].leg.kind
    }

    /// Phoenix collateral kept per dollar of the leg's short.
    fn leg_margin_bps(&self, kind: LegKind) -> u128 {
        match kind {
            LegKind::Spot => self.policy.spot_margin_bps as u128,
            LegKind::NativeSol => self.policy.sol_margin_bps as u128,
        }
    }

    /// Collateral the leg's own asset brings per dollar (the SOL leg's SOL).
    fn weight_bps(&self, kind: LegKind) -> u128 {
        match kind {
            LegKind::Spot => 0,
            LegKind::NativeSol => self.policy.sol_collateral_weight_bps as u128,
        }
    }

    /// USDC margin a dollar more of the leg takes, beyond the asset itself.
    fn extra_margin_bps(&self, kind: LegKind) -> u128 {
        self.leg_margin_bps(kind).saturating_sub(self.weight_bps(kind))
    }

    /// The leg's funding, if fresh.
    pub fn funding(&self, i: usize) -> Option<Funding> {
        let f = *self.funding.get(&self.obs.legs[i].symbol)?;
        (self.obs.now - f.as_of <= self.policy.max_funding_age_secs).then_some(f)
    }

    /// What a dollar of capital in the leg earns a year, in percent: its funding over the asset
    /// and the margin it takes together.
    pub fn capital_apr(&self, i: usize) -> Option<f64> {
        let f = self.funding(i)?;
        let extra = self.extra_margin_bps(self.kind(i)) as f64;
        Some(f.score() * BPS as f64 / (BPS as f64 + extra))
    }

    /// Phoenix effective collateral the keeper aims for: each leg's share, and never under the
    /// vault's floor plus headroom. `extra` adds notional to one leg (a planned deploy).
    pub fn margin_target(&self, extra: Option<(usize, u128)>) -> u128 {
        let (mut by_legs, mut total) = (0u128, 0u128);
        for i in 0..self.obs.legs.len() {
            let mut n = self.obs.notional(i);
            if let Some((j, add)) = extra {
                if i == j {
                    n += add;
                }
            }
            total += n;
            by_legs += n * self.leg_margin_bps(self.kind(i)) / BPS;
        }
        let floor = total
            * (self.obs.vault.margin_bps as u128 + self.policy.margin_headroom_bps as u128)
            / BPS;
        by_legs.max(floor)
    }

    fn effective(&self) -> i128 {
        self.obs.snapshot.margin.effective
    }

    /// Margin Phoenix can give back without going under target. While a SOL leg is held, the
    /// USDC its exit will need on Phoenix (its short's closing fees) stays: taking it out only
    /// to post it again before the sale would go round in circles.
    pub fn releasable(&self) -> u128 {
        let withdrawable = self.obs.snapshot.margin.withdrawable as u128;
        if self.obs.total_notional() == 0 {
            return withdrawable;
        }
        let excess = (self.effective() - self.margin_target(None) as i128).max(0) as u128;
        let sol_exit: u128 = (0..self.obs.legs.len())
            .filter(|&i| self.held(i) && self.kind(i) == LegKind::NativeSol)
            .map(|i| (self.obs.value(i) * 30 / BPS).max(10 * USD) + USD)
            .sum();
        let spare = (self.obs.snapshot.margin.equity - sol_exit as i128).max(0) as u128;
        excess.min(withdrawable).min(if sol_exit > 0 { spare } else { u128::MAX })
    }

    /// Free buffer that can go to Phoenix now: a little under all of it, as the program works
    /// the reserve out again a moment later at its own NAV.
    fn postable(&self) -> u128 {
        let free = self.obs.free_usdc() as u128;
        free.saturating_sub((free * 20 / BPS).max(USD))
    }

    /// What may go to Phoenix out of the reserve: what the vault owes there, and with
    /// `fees` the fees of closing its shorts too (`PHOENIX_COST_CUSHION_BPS`), a dollar under
    /// the program's limit.
    fn owed_to_phoenix(&self, fees: bool) -> u128 {
        let deficit = (-self.obs.snapshot.margin.equity).max(0) as u128;
        if deficit == 0 && !fees {
            return 0;
        }
        let cushion = if fees {
            self.obs.total_notional() * PHOENIX_COST_CUSHION_BPS / BPS
        } else {
            0
        };
        (deficit + cushion).saturating_sub(USD).min(self.obs.snapshot.idle_usdc as u128)
    }

    /// USDC to post before deploying `x` into leg `i`.
    pub fn margin_needed(&self, i: usize, x: u128) -> u128 {
        let target = self.margin_target(Some((i, x))) as i128;
        let brought = (x * self.weight_bps(self.kind(i)) / BPS) as i128;
        (target - self.effective() - brought).max(0) as u128
    }

    /// The most leg `i` can take now (USD), by its caps and the keeper's.
    pub fn capacity(&self, i: usize) -> u128 {
        self.room(i, true)
    }

    /// `capacity`, with or without the shares of the NAV (which a deposit raises itself).
    fn room(&self, i: usize, nav_caps: bool) -> u128 {
        let o = &self.obs.legs[i];
        let s = self.obs.state(i);
        let (value, notional) = (self.obs.value(i), self.obs.notional(i));
        let by_notional = (o.leg.max_notional_usd as u128).saturating_sub(notional);
        let by_units = s
            .price
            .usd_value(o.leg.max_asset_units.saturating_sub(s.units) as u128, s.market.decimals, false)
            .unwrap_or(0);
        let oi_share = (o.leg.max_oi_share_bps as u128).min(self.policy.max_oi_share_bps as u128);
        let oi_lots = (o.open_interest_lots as u128 * oi_share / BPS).saturating_sub(s.short_lots() as u128);
        let by_oi = s
            .market
            .notional(oi_lots.min(u64::MAX as u128) as u64, s.mark_ticks)
            .unwrap_or(0);
        let nav_share = (o.leg.max_nav_share_bps as u128).min(self.policy.max_leg_nav_share_bps as u128);
        let by_nav = if nav_share >= BPS || !nav_caps {
            u128::MAX
        } else {
            (self.obs.nav() * nav_share / BPS).saturating_sub(value)
        };
        // Phoenix counts no more than 500 SOL per trader as collateral.
        let by_sol_cap = if o.leg.kind == LegKind::NativeSol {
            s.price
                .usd_value(PHOENIX_NATIVE_SOL_CAP.saturating_sub(s.units) as u128, s.market.decimals, false)
                .unwrap_or(0)
        } else {
            u128::MAX
        };
        // A hair under: fills land a little either side of the mark.
        by_notional.min(by_units).min(by_oi).min(by_nav).min(by_sol_cap) / 100 * 99
    }

    /// The largest deploy into leg `i` that `budget` of free USDC pays for, margin included.
    pub fn deploy_size(&self, i: usize, budget: u128, cap: u128) -> u128 {
        let (mut lo, mut hi) = (0u128, budget.min(cap));
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if mid + self.margin_needed(i, mid) <= budget {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    /// What a sale of leg `i` is checked at: the mark, or in a multiplier window the higher of
    /// its two multipliers' value, as the program checks it.
    pub fn sale_price(&self, i: usize) -> Price {
        let o = &self.obs.legs[i];
        let s = self.obs.state(i);
        if !o.multiplier_changing {
            return s.price;
        }
        let mut market = s.market;
        market.multiplier = o.high_multiplier;
        market.price(s.mark_ticks).unwrap_or(s.price)
    }

    /// An unwind of `usd` of leg `i` for `rule`: all of it when what would be left is dust and
    /// the turnover allows. None when it comes to nothing, or while it cools down.
    ///
    /// The SOL leg's short settles in USDC on Phoenix (its fees, and its loss after a rally)
    /// while its collateral is the SOL itself. With Phoenix's USDC equity below zero, every SOL
    /// sold thins the margin, and with nothing but SOL left Phoenix will not let the last of it
    /// go. So the keeper posts the deficit first, from the free buffer or (the program allows
    /// it) the reserve; without either, it sells only the SOL that keeps the margin above the
    /// vault's floor, and posts what that raised before selling the rest.
    fn unwind(&self, rule: &'static str, i: usize, usd_wanted: u128, reason: String) -> Option<Decision> {
        let leg = self.obs.legs[i].index;
        let probe = Action::Unwind { leg, units: 0 };
        if self.cooling(rule, &probe) {
            return None;
        }
        // Valued as the program values the sale (a multiplier window prices it higher).
        let value = self
            .sale_price(i)
            .usd_value(self.obs.state(i).units as u128, self.obs.state(i).market.decimals, false)
            .unwrap_or(0);
        let left = self.obs.turnover_left() as u128;
        let usd_wanted = self.shrink(rule, &probe, usd_wanted);
        let mut amount = usd_wanted.min(value).min(self.max_trade()).min(left);
        let mut all = self.failures(rule, &probe) == 0
            && value <= left
            && (amount >= value || (usd_wanted >= value && value - amount < self.min_trade()));
        if self.kind(i) == LegKind::NativeSol {
            let costs = (amount * 30 / BPS).max(10 * USD) as i128;
            let deficit = (costs - self.obs.snapshot.margin.equity).max(0) as u128;
            if deficit > 0 {
                let post = deficit.min(self.postable().max(self.owed_to_phoenix(true)));
                if self.live() && post >= USD.min(deficit) {
                    return Some(Decision {
                        rule,
                        into: None,
                        action: Action::MoveMargin {
                            to_phoenix: true,
                            amount: post as u64,
                        },
                        reason: format!(
                            "{reason}: first {} of USDC to cover the short on Phoenix",
                            usd(post)
                        ),
                    });
                }
                // What stays must carry the deficit at the vault's floor, and a little more.
                let room = self
                    .weight_bps(LegKind::NativeSol)
                    .saturating_sub(self.obs.vault.margin_bps as u128 + SOL_SELL_SAFETY_BPS);
                let keep = (deficit * BPS).checked_div(room).unwrap_or(value);
                amount = amount.min(value.saturating_sub(keep));
                all = false;
            }
        }
        let units = if all {
            self.obs.state(i).units
        } else {
            let s = self.obs.state(i);
            let units = self
                .sale_price(i)
                .tokens_for_usd(amount, s.market.decimals, false)
                .unwrap_or(0);
            units.min(s.units as u128) as u64
        };
        (units > 0).then_some(Decision {
            rule,
            into: None,
            action: Action::Unwind { leg, units },
            reason,
        })
    }

    fn swaps_on(&self) -> bool {
        self.obs.config.swap_program != Pubkey::default()
    }

    fn live(&self) -> bool {
        !self.obs.config.paused
    }

    fn held(&self, i: usize) -> bool {
        self.obs.legs[i].live() && self.obs.value(i) >= USD
    }

    /// Legs the keeper may deploy into, best-paying first.
    fn candidates(&self) -> Vec<(usize, f64)> {
        let mut out: Vec<(usize, f64)> = (0..self.obs.legs.len())
            .filter(|&i| {
                let o = &self.obs.legs[i];
                o.live()
                    && o.leg.active
                    && (!o.after_hours || o.leg.after_hours_trading)
                    && !o.multiplier_changing
                    && self.obs.state(i).mark_ticks > 0
            })
            .filter_map(|i| {
                let f = self.funding(i)?;
                let apr = self.capital_apr(i)?;
                (f.apr_7d > 0.0 && apr >= self.policy.entry_apr).then_some((i, apr))
            })
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }

    fn deploy_cooling(&self, i: usize) -> bool {
        let leg = self.obs.legs[i].index;
        self.cooling("deploy", &Action::Deploy { leg, usdc: 0 })
    }

    // ------------------------------------------------------------ rules

    fn margin_alarm(&self, out: &mut Vec<Decision>) {
        let total = self.obs.total_notional();
        if total == 0 {
            return;
        }
        let alarm = total * (self.obs.vault.margin_bps as u128 + self.policy.margin_alarm_bps as u128) / BPS;
        let e = self.effective();
        if e >= alarm as i128 {
            return;
        }
        let need = (self.margin_target(None) as i128 - e).max(0) as u128;
        let ratio = format!(
            "margin {:.1}% of every short, floor {:.1}%",
            e as f64 / total as f64 * 100.0,
            self.obs.vault.margin_bps as f64 / 100.0
        );
        let post = need.min(self.postable().max(self.owed_to_phoenix(false)));
        if self.live() && post >= USD {
            out.push(Decision {
                rule: "alarm",
                into: None,
                action: Action::MoveMargin {
                    to_phoenix: true,
                    amount: post as u64,
                },
                reason: format!("{ratio}: post {} of the buffer", usd(post)),
            });
        }
        if !self.swaps_on() {
            return;
        }
        // Selling a spot leg closes the same share of its short and frees USDC to post: the
        // largest first, one in a multiplier window last.
        let mut spot: Vec<usize> = (0..self.obs.legs.len())
            .filter(|&i| self.held(i) && self.kind(i) == LegKind::Spot)
            .collect();
        spot.sort_by_key(|&i| (self.obs.legs[i].multiplier_changing, std::cmp::Reverse(self.obs.value(i))));
        for i in spot {
            let reason = format!("{ratio}: sell {} of {} into margin", usd(need), self.obs.legs[i].symbol);
            if let Some(d) = self.unwind("alarm", i, need.max(self.min_trade()), reason) {
                out.push(d);
            }
        }
    }

    fn rehedge(&self, out: &mut Vec<Decision>) {
        for (i, o) in self.obs.legs.iter().enumerate() {
            if !o.live() || o.multiplier_changing {
                continue;
            }
            let s = self.obs.state(i);
            let Ok(target) = s.market.hedge_lots(s.units) else {
                continue;
            };
            let short = -(s.position_lots as i128);
            let off = (target as i128 - short).unsigned_abs();
            if off >= 1 && off * BPS > target as u128 * self.policy.hedge_tolerance_bps as u128 {
                out.push(Decision {
                    rule: "rehedge",
                    into: None,
                    action: Action::Rehedge { leg: o.index },
                    reason: format!(
                        "{} short {short} lots, its asset hedges {target}",
                        o.symbol
                    ),
                });
            }
        }
    }

    /// Whether any live leg still holds something worth selling.
    fn anything_held(&self) -> bool {
        (0..self.obs.legs.len()).any(|i| self.held(i))
    }

    /// Raises `shortfall` of buffer USDC for `why`: margin above target back from Phoenix
    /// first (no trade), then a sale of the lowest-paying leg, whose margin a spot leg frees
    /// too, with every other leg as an alternative should it fail. A leg in a multiplier
    /// window, or with no fresh funding, is sold last; while the vault owes Phoenix USDC, spot
    /// legs before SOL (whose sale Phoenix holds up). With nothing left worth selling, the last
    /// of it (dust, margin under $1) too.
    fn raise(&self, rule: &'static str, shortfall: u128, why: &str, out: &mut Vec<Decision>) {
        let releasable = self.releasable();
        let flat = !self.anything_held();
        if releasable > 0 && (releasable >= shortfall.min(USD) || flat) {
            let amount = releasable.min(shortfall).max(releasable.min(USD));
            out.push(Decision {
                rule,
                into: None,
                action: Action::MoveMargin {
                    to_phoenix: false,
                    amount: amount as u64,
                },
                reason: format!("{why} needs {} more: {} of margin back from Phoenix", usd(shortfall), usd(amount)),
            });
        }
        if !self.swaps_on() {
            return;
        }
        let owing = self.obs.snapshot.margin.equity < 0;
        let mut legs: Vec<usize> = (0..self.obs.legs.len())
            .filter(|&i| {
                self.obs.legs[i].live()
                    && self.obs.state(i).units > 0
                    && (self.held(i) || flat)
            })
            .collect();
        legs.sort_by(|&a, &b| {
            let rank = |i: usize| {
                (
                    self.obs.legs[i].multiplier_changing,
                    owing && self.kind(i) == LegKind::NativeSol,
                )
            };
            let apr = |i| self.capital_apr(i).unwrap_or(f64::INFINITY);
            rank(a).cmp(&rank(b)).then(apr(a).total_cmp(&apr(b)))
        });
        for i in legs {
            // 1% and $10 over: the sale's fees and the margin it frees land a little short of
            // the mark, and a second, tiny sale would cost more than the extra left in the buffer.
            let raise = shortfall + shortfall / 100 + 10 * USD;
            let x = raise * BPS / (BPS + self.extra_margin_bps(self.kind(i)));
            let reason = format!("{why} needs {} more: sell {} of {}", usd(shortfall), usd(x), self.obs.legs[i].symbol);
            if let Some(d) = self.unwind(rule, i, x.max(10 * USD), reason) {
                out.push(d);
            }
        }
    }

    fn requests(&self, out: &mut Vec<Decision>) {
        let nav = self.obs.nav();
        for r in &self.obs.requests {
            let request = &r.request;
            let pay = withdrawal_payout(nav, nav, request.shares, self.obs.snapshot.supply)
                .map(|gross| gross - (gross * self.obs.vault.exit_fee_bps as u128).div_ceil(BPS))
                .unwrap_or(0);
            let fulfill = Action::FulfillWithdraw {
                request: r.address,
                user: request.user,
                id: request.id,
            };
            // One the program cannot pay goes back to its user, and the queue moves on.
            let unpayable = if pay == 0 {
                Some("pays nothing")
            } else if !r.payable {
                Some("its user's USDC account is closed, not theirs, or frozen")
            } else if self.failures("requests", &fulfill) >= FULFILL_ATTEMPTS {
                Some("it failed to pay three times")
            } else {
                None
            };
            if let Some(why) = unpayable {
                out.push(Decision {
                    rule: "requests",
                    into: None,
                    action: Action::ReturnWithdraw {
                        request: r.address,
                        id: request.id,
                    },
                    reason: format!("request {} {why}: returned to its user", request.id),
                });
                continue;
            }
            // A little over: the program pays at its own NAV a moment later. Once everything
            // is USDC the NAV cannot move, and the last holder's payout is all of it.
            let need = pay + (pay / 1_000).max(10_000);
            let idle = self.obs.snapshot.idle_usdc as u128;
            let all_cash = !self.anything_held() && self.releasable() == 0;
            if idle >= need || (all_cash && idle >= pay) {
                out.push(Decision {
                    rule: "requests",
                    into: None,
                    action: fulfill,
                    reason: format!("request {} pays {} from the buffer", request.id, usd(pay)),
                });
            } else {
                let target = if all_cash { pay } else { need };
                self.raise("requests", target.saturating_sub(idle).max(1), &format!("request {}", request.id), out);
            }
            // Oldest first: the next request waits for this one.
            return;
        }
    }

    /// Instant withdrawals drain the buffer and nothing on-chain refills it. Under its reserve,
    /// the keeper brings back margin Phoenix holds over target (no trade); under
    /// `buffer_refill_bps` of it, it also sells the lowest-paying leg. Either way back to the
    /// reserve plus the slack. Requests come first (they reserve the buffer themselves).
    fn buffer_refill(&self, out: &mut Vec<Decision>) {
        if !self.obs.requests.is_empty() {
            return;
        }
        let reserved = self.obs.reserved();
        let idle = self.obs.snapshot.idle_usdc as u128;
        if reserved == 0 || idle >= reserved {
            return;
        }
        let target = reserved + self.usd_policy(self.policy.buffer_slack_usd);
        let why = format!("buffer {} under its {} reserve", usd(idle), usd(reserved));
        if idle * BPS < reserved * self.policy.buffer_refill_bps as u128 {
            self.raise("refill", target - idle, &why, out);
            return;
        }
        let releasable = self.releasable();
        if releasable >= USD {
            let amount = releasable.min(target - idle);
            out.push(Decision {
                rule: "refill",
                into: None,
                action: Action::MoveMargin {
                    to_phoenix: false,
                    amount: amount as u64,
                },
                reason: format!("{why}: {} of margin back from Phoenix", usd(amount)),
            });
        }
    }

    fn margin_topup(&self, out: &mut Vec<Decision>) {
        if !self.live() || self.obs.total_notional() == 0 {
            return;
        }
        let target = self.margin_target(None);
        let e = self.effective();
        let low = target * (BPS - (self.policy.margin_topup_bps as u128).min(BPS)) / BPS;
        let postable = self.postable();
        if e < low as i128 && postable >= USD {
            let amount = ((target as i128 - e) as u128).min(postable);
            out.push(Decision {
                rule: "topup",
                into: None,
                action: Action::MoveMargin {
                    to_phoenix: true,
                    amount: amount as u64,
                },
                reason: format!(
                    "margin {} under its target {}: post {}",
                    usd(e.max(0) as u128),
                    usd(target),
                    usd(amount)
                ),
            });
        }
    }

    /// Deposits stay open while some active leg with room pays: new USDC that cannot be put to
    /// work would only dilute what the holders earn.
    fn guard(&self, out: &mut Vec<Decision>) {
        if !self.policy.manage_deposits || !self.live() {
            return;
        }
        let scored: Vec<(usize, f64)> = (0..self.obs.legs.len())
            .filter(|&i| self.obs.legs[i].live() && self.obs.legs[i].leg.active)
            .filter_map(|i| self.capital_apr(i).map(|apr| (i, apr)))
            .collect();
        if scored.is_empty() {
            // No funding read: no reason to change anything.
            return;
        }
        let best = scored
            .iter()
            .filter(|&&(i, _)| self.room(i, false) >= self.min_trade())
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .copied();
        let (what, best) = match best {
            Some((i, apr)) => (format!("{} pays {apr:.2}%", self.obs.legs[i].symbol), apr),
            None => ("no paying leg has room".to_string(), f64::NEG_INFINITY),
        };
        let open = self.obs.vault.deposits_open;
        if open && best < self.policy.close_deposits_apr {
            out.push(Decision {
                rule: "guard",
                into: None,
                action: Action::SetDepositsOpen(false),
                reason: format!("{what}, under {:.2}%: close deposits", self.policy.close_deposits_apr),
            });
        } else if !open && best >= self.policy.reopen_deposits_apr {
            out.push(Decision {
                rule: "guard",
                into: None,
                action: Action::SetDepositsOpen(true),
                reason: format!("{what}, at least {:.2}%: reopen deposits", self.policy.reopen_deposits_apr),
            });
        }
    }

    fn exits(&self, out: &mut Vec<Decision>) {
        if !self.swaps_on() {
            return;
        }
        let mut exits: Vec<(f64, Decision)> = vec![];
        for i in (0..self.obs.legs.len()).filter(|&i| self.held(i)) {
            let o = &self.obs.legs[i];
            // A token in a multiplier window waits for it to close: its sale is checked at the
            // higher multiplier, which the market may not pay yet.
            if o.multiplier_changing {
                continue;
            }
            let value = self.obs.value(i);
            let cause = if !o.leg.active {
                Some((f64::NEG_INFINITY, "deactivated".to_string()))
            } else {
                self.capital_apr(i)
                    .filter(|apr| *apr < self.policy.exit_apr)
                    .map(|apr| {
                        (apr, format!("pays {apr:.2}% < exit {:.2}%", self.policy.exit_apr))
                    })
            };
            if let Some((rank, why)) = cause {
                let reason = format!("{} {why}: unwind {} to USDC", o.symbol, usd(value));
                if let Some(d) = self.unwind("exit", i, value, reason) {
                    exits.push((rank, d));
                }
                continue;
            }
            // Past its share of the market: trim back to the keeper's share.
            let s = self.obs.state(i);
            let oi = o.open_interest_lots as u128;
            if s.short_lots() as u128 * BPS > oi * self.policy.trim_oi_share_bps as u128 {
                let keep = oi * self.policy.max_oi_share_bps as u128 / BPS;
                let excess = (s.short_lots() as u128).saturating_sub(keep);
                let trim = s
                    .market
                    .notional(excess.min(u64::MAX as u128) as u64, s.mark_ticks)
                    .unwrap_or(0);
                let reason = format!(
                    "{} short is {:.1}% of open interest: trim {}",
                    o.symbol,
                    s.short_lots() as f64 / oi.max(1) as f64 * 100.0,
                    usd(trim)
                );
                if let Some(d) = self.unwind("trim", i, trim, reason) {
                    exits.push((f64::MAX, d));
                }
            }
        }
        exits.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.extend(exits.into_iter().map(|(_, d)| d));
    }

    fn deploy(&self, out: &mut Vec<Decision>) {
        if !self.live() || !self.swaps_on() {
            return;
        }
        let free = self.obs.free_usdc() as u128;
        let budget = free.saturating_sub(self.usd_policy(self.policy.buffer_slack_usd));
        if budget < self.min_trade() {
            return;
        }
        let limit = self.max_trade().min(self.obs.turnover_left() as u128);
        for (i, apr) in self.candidates() {
            // A market whose last deploy failed waits out its cooldown, and gets half as much.
            if self.deploy_cooling(i) {
                continue;
            }
            let leg = self.obs.legs[i].index;
            let cap = self.shrink("deploy", &Action::Deploy { leg, usdc: 0 }, self.capacity(i).min(limit));
            if cap < self.min_trade() {
                continue;
            }
            let x = self.deploy_size(i, budget, cap);
            if x < self.min_trade() {
                continue;
            }
            let symbol = &self.obs.legs[i].symbol;
            let need = self.margin_needed(i, x);
            out.push(if need > 0 {
                Decision {
                    rule: "deploy",
                    into: None,
                    action: Action::MoveMargin {
                        to_phoenix: true,
                        amount: need as u64,
                    },
                    reason: format!("margin for {} into {symbol} ({apr:.2}%): post {}", usd(x), usd(need)),
                }
            } else {
                Decision {
                    rule: "deploy",
                    into: None,
                    action: Action::Deploy { leg, usdc: x as u64 },
                    reason: format!("{symbol} pays {apr:.2}%: deploy {} of {} free", usd(x), usd(free)),
                }
            });
        }
    }

    fn rotate(&self, out: &mut Vec<Decision>) {
        if !self.live() || !self.swaps_on() {
            return;
        }
        let limit = self.max_trade().min(self.obs.turnover_left() as u128 / 2);
        // Not into a market whose deploy is cooling down: the keeper would sell A, fail to
        // buy B, and buy A back.
        let Some((b, best)) = self
            .candidates()
            .into_iter()
            .find(|&(i, _)| !self.deploy_cooling(i) && self.capacity(i).min(limit) >= self.min_trade())
        else {
            return;
        };
        let worst = (0..self.obs.legs.len())
            .filter(|&i| {
                i != b
                    && self.held(i)
                    && !self.obs.legs[i].multiplier_changing
                    && self.obs.value(i) >= self.min_trade()
            })
            .filter_map(|i| self.capital_apr(i).map(|apr| (i, apr)))
            .min_by(|x, y| x.1.total_cmp(&y.1));
        let Some((a, apr)) = worst else {
            return;
        };
        if best - apr < self.policy.rotate_gap_apr {
            return;
        }
        // B's room takes its asset and its margin; selling A frees its asset and its margin.
        // $30k of room in a spot leg at 1x is $60k of SOL sold, or $30k of another spot leg.
        // Half as much for each deploy into B that failed in a row.
        let leg_b = self.obs.legs[b].index;
        let room = self.shrink("deploy", &Action::Deploy { leg: leg_b, usdc: 0 }, self.capacity(b));
        let cash = room * (BPS + self.extra_margin_bps(self.kind(b))) / BPS;
        let amount = (cash * BPS / (BPS + self.extra_margin_bps(self.kind(a))))
            .min(self.obs.value(a))
            .min(limit);
        if amount < self.min_trade() {
            return;
        }
        let reason = format!(
            "rotate {} of {} ({apr:.2}%) into {} ({best:.2}%)",
            usd(amount),
            self.obs.legs[a].symbol,
            self.obs.legs[b].symbol
        );
        if let Some(mut d) = self.unwind("rotate", a, amount, reason) {
            let mint = match self.kind(b) {
                LegKind::NativeSol => vanna_phoenix_multi_vault::constants::NATIVE_MINT,
                LegKind::Spot => self.obs.legs[b].leg.asset_mint,
            };
            d.into = Some((mint, room.min(amount) as u64));
            out.push(d);
        }
    }

    fn margin_release(&self, out: &mut Vec<Decision>) {
        let total = self.obs.total_notional();
        let amount = self.releasable();
        let worth = if total == 0 {
            amount >= USD
        } else {
            let high = self.margin_target(None)
                * (BPS + self.policy.margin_release_bps as u128)
                / BPS;
            self.effective() > high as i128 && amount >= self.min_trade()
        };
        if worth {
            out.push(Decision {
                rule: "release",
                into: None,
                action: Action::MoveMargin {
                    to_phoenix: false,
                    amount: amount as u64,
                },
                reason: format!(
                    "Phoenix holds {} over the margin target {}: back to the buffer",
                    usd(amount),
                    usd(self.margin_target(None))
                ),
            });
        }
    }

    /// Everything worth doing now, most urgent first, skipping moves that are cooling down.
    pub fn plan(&self) -> Vec<Decision> {
        let mut out = vec![];
        self.margin_alarm(&mut out);
        self.rehedge(&mut out);
        self.requests(&mut out);
        self.buffer_refill(&mut out);
        self.margin_topup(&mut out);
        self.guard(&mut out);
        self.exits(&mut out);
        self.deploy(&mut out);
        self.rotate(&mut out);
        self.margin_release(&mut out);
        out.retain(|d| !self.cooling(d.rule, &d.action));
        out
    }

    /// What an operator should look at, as (stable key, message): nothing here is the keeper's
    /// to fix alone. The key is what repeats are counted by, the message carries the numbers.
    pub fn health(&self) -> Vec<(String, String)> {
        let mut alerts: Vec<(String, String)> = vec![];
        let mut alert = |key: String, message: String| alerts.push((key, message));
        let o = self.obs;
        if o.config.paused {
            alert("paused".into(), "the protocol is paused: only unwinds, re-hedges and withdrawals run".into());
        }
        if o.snapshot.equity().map(|e| e < 0).unwrap_or(false) {
            alert("insolvent".into(), "the vault owes Phoenix more than it holds".into());
        }
        let total = o.total_notional();
        if total > 0 {
            let alarm = total * (o.vault.margin_bps as u128 + self.policy.margin_alarm_bps as u128) / BPS;
            if self.effective() < alarm as i128 {
                alert(
                    "margin".into(),
                    format!(
                        "margin {:.1}% of every short, floor {:.1}%",
                        self.effective() as f64 / total as f64 * 100.0,
                        o.vault.margin_bps as f64 / 100.0
                    ),
                );
            }
        }
        if let Some(r) = o.requests.first() {
            let r = &r.request;
            if o.now - r.created_at > self.policy.request_alert_secs {
                alert(
                    format!("request-waits:{}", r.id),
                    format!("withdrawal request {} has waited {}h", r.id, (o.now - r.created_at) / 3_600),
                );
            }
            if o.turnover_left() < USD as u64 {
                alert("turnover".into(), "withdrawal requests wait and the daily turnover limit is reached".into());
            }
            if !self.swaps_on() {
                alert("swaps-off".into(), "withdrawal requests wait and swaps are off".into());
            }
        }
        for (i, l) in o.legs.iter().enumerate() {
            if self.held(i) && self.funding(i).is_none() {
                alert(
                    format!("funding:{}", l.symbol),
                    format!("no fresh funding for {}, which holds {}", l.symbol, usd(o.value(i))),
                );
            }
            if l.live() && l.leg.kind == LegKind::NativeSol && l.leg.max_asset_units > PHOENIX_NATIVE_SOL_CAP {
                alert(
                    "sol-cap".into(),
                    "the SOL leg may hold more than the 500 SOL Phoenix counts as collateral: cap it with max_asset_units".into(),
                );
            }
        }
        if o.keeper_lamports < KEEPER_SOL_FLOOR {
            alert(
                "keeper-sol".into(),
                format!("the keeper has {:.4} SOL left for fees", o.keeper_lamports as f64 / 1e9),
            );
        }
        alerts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::{LegObs, RequestObs};
    use vanna_phoenix_multi_vault::constants::{MULTIPLIER_SCALE, TURNOVER_WINDOW_SECONDS};
    use vanna_phoenix_multi_vault::nav::{LegState, Margin, Market, Snapshot};
    use vanna_phoenix_multi_vault::state::{Config, Leg, Vault, WithdrawRequest};

    const NOW: i64 = 1_800_000_000;
    const SOL: Market = Market {
        decimals: 9,
        base_lot_decimals: 2,
        tick_size: 10,
        multiplier: MULTIPLIER_SCALE,
    };
    const ETH: Market = Market {
        decimals: 8,
        base_lot_decimals: 3,
        tick_size: 10,
        multiplier: MULTIPLIER_SCALE,
    };

    fn leg(index: u8, kind: LegKind) -> Leg {
        Leg {
            vault: Pubkey::default(),
            index,
            kind,
            asset_mint: Pubkey::new_unique(),
            asset_decimals: if kind == LegKind::NativeSol { 9 } else { 8 },
            asset_token_program: anchor_spl::token::ID,
            asset_account: Pubkey::new_unique(),
            asset_id: index as u32,
            orderbook: Pubkey::new_unique(),
            spline_collection: Pubkey::new_unique(),
            base_lot_decimals: if kind == LegKind::NativeSol { 2 } else { 3 },
            tick_size: 10,
            active: true,
            retired: false,
            max_notional_usd: u64::MAX,
            max_asset_units: u64::MAX,
            max_oi_share_bps: 10_000,
            max_nav_share_bps: 10_000,
            after_hours_trading: true,
            bump: 0,
        }
    }

    /// $100,000 in the buffer, a SOL leg at $150 and an ETH leg at $3,500, both flat, with
    /// plenty of open interest.
    fn vault() -> Observation {
        let legs = vec![leg(0, LegKind::NativeSol), leg(1, LegKind::Spot)];
        let states = vec![
            LegState {
                market: SOL,
                mark_ticks: 150_000,
                price: SOL.price(150_000).unwrap(),
                position_lots: 0,
                units: 0,
            },
            LegState {
                market: ETH,
                mark_ticks: 350_000,
                price: ETH.price(350_000).unwrap(),
                position_lots: 0,
                units: 0,
            },
        ];
        Observation {
            now: NOW,
            config_address: Pubkey::new_unique(),
            config: Config {
                admin: Pubkey::new_unique(),
                keeper: Pubkey::new_unique(),
                usdc_mint: Pubkey::new_unique(),
                phoenix_mint: Pubkey::new_unique(),
                perp_asset_map: Pubkey::new_unique(),
                withdraw_queue: Pubkey::new_unique(),
                global_vault: Pubkey::new_unique(),
                swap_program: Pubkey::new_unique(),
                paused: false,
                bump: 0,
            },
            vault_address: Pubkey::new_unique(),
            vault: Vault {
                id: 1,
                share_mint: Pubkey::new_unique(),
                authority: Pubkey::new_unique(),
                usdc_account: Pubkey::new_unique(),
                phoenix_account: Pubkey::new_unique(),
                escrow_shares: Pubkey::new_unique(),
                trader_account: Pubkey::new_unique(),
                leg_count: 2,
                next_request_id: 0,
                pending_shares: 0,
                max_tvl_usd: u64::MAX,
                max_deposit_usd: u64::MAX,
                entry_fee_bps: 0,
                exit_fee_bps: 0,
                buffer_bps: 500,
                margin_bps: 5_000,
                slippage_bps: 100,
                max_turnover_usd: 10_000_000 * USD as u64,
                turnover_window_start: NOW - TURNOVER_WINDOW_SECONDS,
                turnover_window_usd: 0,
                deposits_open: true,
                bump: 0,
                authority_nonce: [0; 8],
                authority_bump: 0,
            },
            legs: legs
                .into_iter()
                .enumerate()
                .map(|(i, leg)| LegObs {
                    index: i as u8,
                    address: Pubkey::new_unique(),
                    leg,
                    symbol: ["SOL", "ETH"][i].into(),
                    open_interest_lots: 100_000_000,
                    after_hours: false,
                    multiplier_changing: false,
                    high_multiplier: MULTIPLIER_SCALE,
                })
                .collect(),
            snapshot: Snapshot {
                legs: states,
                idle_usdc: 100_000 * USD as u64,
                margin: Margin::default(),
                supply: 100_000 * USD as u64,
                pending_shares: 0,
            },
            requests: vec![],
            gti: vec![],
            atb: vec![],
            keeper_lamports: 1_000_000_000,
        }
    }

    fn funding(sol: f64, eth: f64) -> HashMap<String, Funding> {
        [("SOL".to_string(), Funding::flat(sol, NOW)), ("ETH".to_string(), Funding::flat(eth, NOW))].into()
    }

    #[test]
    fn a_spot_leg_earns_on_half_its_capital_and_sol_on_all_of_it() {
        let obs = vault();
        let f = funding(10.0, 10.0);
        let p = Policy::default();
        let planner = Planner::new(&obs, &f, &p);
        assert!((planner.capital_apr(0).unwrap() - 10.0).abs() < 1e-9);
        assert!((planner.capital_apr(1).unwrap() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn free_usdc_goes_to_the_best_leg_with_its_margin_first() {
        let obs = vault();
        // ETH's 30% funding is 15% on capital: better than SOL's 12%.
        let f = funding(12.0, 30.0);
        let p = Policy::default();
        let planner = Planner::new(&obs, &f, &p);
        let plan = planner.plan();
        // $100,000 less the 5% buffer and $1,000 slack: $94,000, half to buy ETH, half margin.
        let first = &plan[0];
        let Action::MoveMargin { to_phoenix: true, amount } = first.action else {
            panic!("{plan:?}");
        };
        assert!((amount as i128 - 47_000 * USD as i128).abs() <= USD as i128, "{amount}");
        // With the margin posted, the same plan deploys ETH.
        let mut posted = obs.clone();
        posted.snapshot.idle_usdc -= amount;
        posted.snapshot.margin.effective = amount as i128;
        posted.snapshot.margin.equity = amount as i128;
        let planner = Planner::new(&posted, &f, &p);
        let plan = planner.plan();
        let Action::Deploy { leg: 1, usdc } = plan[0].action else {
            panic!("{plan:?}");
        };
        assert!((usdc as i128 - 47_000 * USD as i128).abs() <= USD as i128);
    }

    #[test]
    fn the_sol_leg_takes_the_whole_budget_as_its_own_margin() {
        let obs = vault();
        let f = funding(20.0, 8.0);
        let mut p = Policy::default();
        // The keeper's 50% cap on one leg: half the NAV, less 1% for the fill.
        let plan = Planner::new(&obs, &f, &p).plan();
        let Action::Deploy { leg: 0, usdc } = plan[0].action else {
            panic!("{plan:?}");
        };
        assert_eq!(usdc as u128, 49_500 * USD);
        // Without it, as much as Phoenix counts as collateral: 500 SOL ($75,000), less 1%. SOL
        // needs no USDC margin: at 80% it is its own (above the 50% floor plus 20% headroom).
        p.max_leg_nav_share_bps = 10_000;
        p.max_trade_usd = 1_000_000;
        let plan = Planner::new(&obs, &f, &p).plan();
        let Action::Deploy { leg: 0, usdc } = plan[0].action else {
            panic!("{plan:?}");
        };
        assert_eq!(usdc as u128, 74_250 * USD);
    }

    #[test]
    fn nothing_worth_entering_means_nothing_to_do_but_close_deposits() {
        let obs = vault();
        let f = funding(1.0, -4.0);
        let p = Policy::default();
        let plan = Planner::new(&obs, &f, &p).plan();
        assert_eq!(plan.len(), 1, "{plan:?}");
        assert_eq!(plan[0].action, Action::SetDepositsOpen(false));
        // Without funding data the keeper changes nothing at all.
        let none = HashMap::new();
        assert!(Planner::new(&obs, &none, &p).plan().is_empty());
    }

    /// The vault with $40,000 of ETH held and shorted at 1x margin and $10,000 idle.
    fn eth_vault() -> Observation {
        let mut obs = vault();
        let units = 40_000 * USD as u64 / 3_500 * 100; // ETH (8 dp) worth $40,000
        let lots = ETH.hedge_lots(units).unwrap();
        obs.snapshot.legs[1].units = units;
        obs.snapshot.legs[1].position_lots = -(lots as i64);
        let notional = ETH.notional(lots, 350_000).unwrap() as i128;
        obs.snapshot.margin = Margin {
            equity: notional,
            effective: notional,
            initial: 0,
            withdrawable: notional as u64,
        };
        obs.snapshot.idle_usdc = 10_000 * USD as u64;
        obs.snapshot.supply = 90_000 * USD as u64;
        obs
    }

    #[test]
    fn a_leg_that_stops_paying_is_unwound_to_usdc_in_the_vault() {
        let obs = eth_vault();
        let f = funding(1.0, -10.0);
        let p = Policy::default();
        let plan = Planner::new(&obs, &f, &p).plan();
        let unwind = plan
            .iter()
            .find(|d| matches!(d.action, Action::Unwind { leg: 1, .. }))
            .unwrap_or_else(|| panic!("{plan:?}"));
        let Action::Unwind { units, .. } = unwind.action else { unreachable!() };
        assert_eq!(units, obs.state(1).units, "all of it");
        // The same funding held above the exit rate keeps the leg (hysteresis).
        let f = funding(1.0, 5.0);
        let plan = Planner::new(&obs, &f, &p).plan();
        assert!(!plan.iter().any(|d| matches!(d.action, Action::Unwind { .. })), "{plan:?}");
    }

    #[test]
    fn a_request_is_paid_from_the_buffer_or_raised_from_margin_then_legs() {
        let mut obs = eth_vault();
        let user = Pubkey::new_unique();
        let vault = obs.vault_address;
        let request = |shares| RequestObs {
            address: Pubkey::new_unique(),
            request: WithdrawRequest {
                vault,
                user,
                id: 0,
                shares,
                created_at: NOW,
                bump: 0,
                returned: false,
            },
            payable: true,
        };
        // 5,000 shares of 90,000 in a $90,000 vault: $5,000, the buffer holds it.
        obs.requests = vec![request(5_000 * USD as u64)];
        obs.vault.pending_shares = 5_000 * USD as u64;
        obs.snapshot.pending_shares = obs.vault.pending_shares;
        let f = funding(10.0, 30.0);
        let p = Policy::default();
        let plan = Planner::new(&obs, &f, &p).plan();
        assert!(matches!(plan[0].action, Action::FulfillWithdraw { id: 0, .. }), "{plan:?}");

        // 30,000 shares: $30,000. The margin is at target, so the keeper sells ETH (and its
        // margin follows).
        obs.requests = vec![request(30_000 * USD as u64)];
        obs.vault.pending_shares = 30_000 * USD as u64;
        obs.snapshot.pending_shares = obs.vault.pending_shares;
        let plan = Planner::new(&obs, &f, &p).plan();
        let Action::Unwind { leg: 1, units } = plan[0].action else {
            panic!("{plan:?}");
        };
        let sold = obs.state(1).price.usd_value(units as u128, 8, false).unwrap();
        // $20,030 short: half of it (and 1% and $10 over) in ETH, its margin the other half.
        assert!((sold as i128 - 10_120 * USD as i128).abs() < 20 * USD as i128, "{sold}");
        // Nothing is deployed while the request waits.
        assert!(!plan.iter().any(|d| matches!(d.action, Action::Deploy { .. })));
    }

    #[test]
    fn rotation_needs_the_gap_to_pay_for_itself() {
        let mut obs = eth_vault();
        obs.snapshot.idle_usdc = 5_000 * USD as u64; // just the buffer
        let p = Policy::default();
        // ETH pays 10% (5% on capital), SOL 12%: a 7% gap, under 8. Hold.
        let plan = Planner::new(&obs, &funding(12.0, 10.0), &p).plan();
        assert!(!plan.iter().any(|d| matches!(d.action, Action::Unwind { .. })), "{plan:?}");
        // SOL 20%: rotate.
        let plan = Planner::new(&obs, &funding(20.0, 10.0), &p).plan();
        assert!(plan.iter().any(|d| matches!(d.action, Action::Unwind { leg: 1, .. })), "{plan:?}");
    }

    #[test]
    fn a_thin_margin_is_restored_before_anything_else() {
        let mut obs = eth_vault();
        // ETH rallied: the short lost $17,000 of margin, under the 50% floor plus 10%.
        obs.snapshot.margin.effective -= 17_000 * USD as i128;
        obs.snapshot.idle_usdc = 3_000 * USD as u64; // under the reserve
        let p = Policy::default();
        let plan = Planner::new(&obs, &funding(12.0, 30.0), &p).plan();
        assert!(matches!(plan[0].action, Action::Unwind { leg: 1, .. }), "{plan:?}");
    }

    #[test]
    fn a_donation_is_rehedged() {
        let mut obs = eth_vault();
        obs.snapshot.legs[1].units += 100_000_000; // one more ETH
        let p = Policy::default();
        let plan = Planner::new(&obs, &funding(12.0, 30.0), &p).plan();
        assert_eq!(plan[0].action, Action::Rehedge { leg: 1 });
    }
}
