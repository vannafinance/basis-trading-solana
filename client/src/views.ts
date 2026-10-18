// Everything a vault page shows. Positions, delta, caps and the liquidation distance come from
// the vault's own state (`getVaultState`); funding and open interest from Phoenix's API; the
// realized APY, the last rebalance and a user's earnings from the program's events. Rates are
// percents as `number` (20.5 is 20.5% a year); amounts stay `bigint`, USD in 6 decimals.
import { Connection, PublicKey } from "@solana/web3.js";
import { TOKEN_PROGRAM_ID } from "@solana/spl-token";
import { ASSETS, BPS, PHOENIX_API, PHOENIX_MAINNET, PhoenixKeys } from "./constants";
import { MULTIPLIER_SCALE, sharesForValue, usdValue } from "./math";
import { tokenAccount, userShareAccount } from "./pda";
import { fetchPhoenixMarket, getUserPosition, getVaultState, VaultState } from "./state";
import { parseEvents } from "./transaction";
import type { VaultInfo, VaultProgram } from "./vault";

const HOURS_PER_YEAR = 24 * 365;
const SECONDS_PER_YEAR = 365 * 86_400;
const U64_MAX = 2n ** 64n - 1n;

/** The Phoenix market symbol of a vault ("SOL", "ETH", "BTC", "TSLA", "META"). */
export function vaultSymbol(vault: Pick<VaultInfo, "assetMint">): string {
  const asset = Object.values(ASSETS).find((a) => a.mint.equals(vault.assetMint));
  if (!asset) throw new Error(`no asset for mint ${vault.assetMint.toBase58()}`);
  return asset.phoenixSymbol;
}

// ------------------------------------------------------------------ funding (Phoenix's API)

/** One hour of Phoenix funding: `ratePct` of the position's notional, paid by longs to shorts when positive. */
export interface FundingPoint {
  timestamp: number;
  ratePct: number;
}

/** Phoenix's hourly funding rates for `symbol` over the last `days` (`GET /v1/funding/{symbol}/rates`). */
export async function fetchFundingHistory(symbol: string, days = 30, nowMs = Date.now()): Promise<FundingPoint[]> {
  const query = new URLSearchParams({ startTime: String(nowMs - days * 86_400_000), endTime: String(nowMs) });
  const res = await fetch(`${PHOENIX_API}/v1/funding/${symbol}/rates?${query}`);
  const { rates } = await res.json();
  return (rates ?? []).map((r: any) => ({ timestamp: Number(r.timestamp), ratePct: Number(r.fundingRatePercentage) }));
}

export interface FundingStats {
  /** The last hour's rate, annualised: "Funding Now". */
  nowApr: number;
  avg7dApr: number;
  avg30dApr: number;
  /** UTC days whose funding summed below zero (shorts paid instead of earning), of `days`. */
  negativeDays: number;
  days: number;
}

export function fundingStats(points: FundingPoint[], now = Date.now() / 1000): FundingStats {
  const since = (days: number) => points.filter((p) => p.timestamp >= now - days * 86_400);
  const apr = (ps: FundingPoint[]) =>
    ps.length ? (ps.reduce((sum, p) => sum + p.ratePct, 0) / ps.length) * HOURS_PER_YEAR : 0;
  const byDay = new Map<number, number>();
  for (const p of since(30)) {
    const day = Math.floor(p.timestamp / 86_400);
    byDay.set(day, (byDay.get(day) ?? 0) + p.ratePct);
  }
  return {
    nowApr: points.length ? points[points.length - 1].ratePct * HOURS_PER_YEAR : 0,
    avg7dApr: apr(since(7)),
    avg30dApr: apr(since(30)),
    negativeDays: [...byDay.values()].filter((v) => v < 0).length,
    days: byDay.size,
  };
}

/** What the vault page needs from Phoenix's market data. */
export interface MarketInfo {
  /** Open interest in base lots: what the vault's open-interest cap is measured against. */
  openInterestLots: bigint;
  /** Phoenix's taker fee, in bps (3.5 is 0.035%). */
  takerFeeBps: number;
  status: string;
}

export async function fetchMarketInfo(symbol: string): Promise<MarketInfo> {
  const m = await fetchPhoenixMarket(symbol);
  if (!m) throw new Error(`Phoenix has no market ${symbol}`);
  return {
    openInterestLots: BigInt(m.openInterest.value),
    takerFeeBps: Number(m.fees.takerFeeMicro) / 100,
    status: m.marketStatus,
  };
}

// ------------------------------------------------------------------ positions (vault state)

type VaultShape = Pick<VaultInfo, "kind" | "assetDecimals" | "baseLotDecimals" | "targetMarginBps">;
type StateShape = Pick<
  VaultState,
  "assetUnits" | "assetValue" | "positionLots" | "shortNotional" | "nav" | "phoenixEquity" | "multiplier" | "tokenPrice" | "margin"
>;

export interface VaultPositions {
  /** The asset held: as Phoenix collateral (the SOL vault) or in the vault's token account. */
  long: {
    units: bigint;
    /** In the market's units (coins, or shares for a stock token): what the short is measured in. */
    marketUnits: number;
    usd: bigint;
    heldIn: "phoenixCollateral" | "vaultTokenAccount";
  };
  /** The short on Phoenix. */
  short: { baseLots: bigint; marketUnits: number; usd: bigint };
  /** (held − short) / short, in percent: "Net delta". The hedge rounds to Phoenix's lot size. */
  netDeltaPct: number;
  /** Short / held: "Hedge 1x". */
  hedgeRatio: number;
  /** Where the NAV sits, in percent: the asset held, and the USDC (and PnL) on Phoenix. */
  assetPct: number;
  marginPct: number;
  /** Short notional per dollar of NAV: the part of a deposit that earns funding (1 for SOL, 0.5 for spot vaults at 1x). */
  fundingExposure: number;
  /** How far the price can rise before Phoenix's maintenance margin is reached, in percent; null with no short. */
  liquidationRisePct: number | null;
  /** The price there, per token (USD, 6 dp). */
  liquidationPrice: bigint | null;
}

/**
 * The price rise, in percent, at which the vault's effective collateral meets Phoenix's
 * maintenance margin. Per 1% rise the short loses 1% of its notional (N) and maintenance grows
 * by 1% (M); the SOL vault's SOL adds its haircut value (S) back, while a spot vault's tokens are
 * not on Phoenix and add nothing. Solves E + x·S − x·N = M·(1 + x). Phoenix's leverage tiers
 * and funding move it a little: a guide, not a promise.
 */
export function liquidationRisePct(kind: VaultInfo["kind"], state: Pick<VaultState, "shortNotional" | "margin">): number | null {
  const m = state.margin;
  const notional = Number(state.shortNotional);
  if (notional === 0) return null;
  // The SOL's own effective collateral, as the program reads it (`Margin::sol_effective`).
  const sol = kind === "nativeSol" ? Number(m.effectiveCollateral - m.collateral - m.unsettledFunding - m.discountedUnrealizedPnl) : 0;
  const effective = Number(m.effectiveCollateral);
  const maintenance = Number(m.maintenanceMargin);
  const perRise = notional + maintenance - sol;
  if (perRise <= 0) return null;
  return Math.max(0, ((effective - maintenance) / perRise) * 100);
}

export function vaultPositions(vault: VaultShape, state: StateShape): VaultPositions {
  const scale = 10 ** vault.assetDecimals;
  const longMarket = (Number(state.assetUnits) / scale) * (Number(state.multiplier) / Number(MULTIPLIER_SCALE));
  const shortLots = state.positionLots < 0n ? -state.positionLots : 0n;
  const shortMarket = Number(shortLots) / 10 ** vault.baseLotDecimals;
  const nav = Number(state.nav);
  const rise = liquidationRisePct(vault.kind, state);
  return {
    long: {
      units: state.assetUnits,
      marketUnits: longMarket,
      usd: state.assetValue,
      heldIn: vault.kind === "nativeSol" ? "phoenixCollateral" : "vaultTokenAccount",
    },
    short: { baseLots: shortLots, marketUnits: shortMarket, usd: state.shortNotional },
    netDeltaPct: shortMarket > 0 ? ((longMarket - shortMarket) / shortMarket) * 100 : 0,
    hedgeRatio: longMarket > 0 ? shortMarket / longMarket : 0,
    assetPct: nav > 0 ? (Number(state.assetValue) / nav) * 100 : 0,
    marginPct: nav > 0 ? (Number(state.phoenixEquity) / nav) * 100 : 0,
    fundingExposure: nav > 0 ? Number(state.shortNotional) / nav : 0,
    liquidationRisePct: rise,
    liquidationPrice: rise === null ? null : (state.tokenPrice * BigInt(Math.round((100 + rise) * 100))) / 10_000n,
  };
}

// ------------------------------------------------------------------ caps

export interface VaultCapacity {
  /** The NAV the vault can reach under its caps, at today's price and open interest: "Cap". */
  capUsd: bigint;
  /** The cap that binds first. */
  binding: "position" | "assetUnits" | "openInterest";
  /** Deposits the vault can still take (USD); one deposit is also held to `maxDepositUsd`. */
  roomUsd: bigint;
  caps: { position: bigint; assetUnits: bigint | null; openInterest: bigint | null };
}

/**
 * The caps as NAV, using the vault's mix: a deposit keeps `targetMarginBps` of margin per dollar
 * of short, so each dollar of short is (1 + t) of NAV. The open-interest cap is the vault's
 * share of today's open interest (pass `openInterestLots` from `fetchMarketInfo`); the program
 * checks it after each deposit's own trade, so treat it as about.
 */
export function vaultCapacity(
  vault: Pick<VaultInfo, "maxPositionUsd" | "maxAssetUnits" | "maxOiShareBps" | "targetMarginBps" | "assetDecimals" | "tickSize">,
  state: Pick<VaultState, "nav" | "tokenPrice" | "markPriceTicks">,
  openInterestLots?: bigint,
): VaultCapacity {
  const mix = (usd: bigint) => (usd * (BPS + BigInt(vault.targetMarginBps))) / BPS;
  const position = mix(vault.maxPositionUsd);
  const assetUnits =
    vault.maxAssetUnits === U64_MAX ? null : mix(usdValue(vault.maxAssetUnits, state.tokenPrice, vault.assetDecimals));
  const openInterest =
    openInterestLots === undefined
      ? null
      : mix(((openInterestLots * BigInt(vault.maxOiShareBps)) / BPS) * state.markPriceTicks * vault.tickSize);
  let capUsd = position;
  let binding: VaultCapacity["binding"] = "position";
  if (assetUnits !== null && assetUnits < capUsd) [capUsd, binding] = [assetUnits, "assetUnits"];
  if (openInterest !== null && openInterest < capUsd) [capUsd, binding] = [openInterest, "openInterest"];
  return {
    capUsd,
    binding,
    roomUsd: capUsd > state.nav ? capUsd - state.nav : 0n,
    caps: { position, assetUnits, openInterest },
  };
}

// ------------------------------------------------------------------ events and history

export interface VaultEvent {
  /** `deposited`, `withdrawn`, `swapped`, `rebalanced` or `depositsSet`. */
  name: string;
  data: any;
  signature: string;
  blockTime: number | null;
  slot: number;
}

/**
 * The program's events in the recent transactions of `address`, newest first. Pass the vault's
 * address for its whole feed ("All Transactions"), or a user's share account for theirs. Reads
 * `limit` signatures; page with `before`. For long histories, index these in a backend.
 */
export async function fetchEvents(
  connection: Connection,
  program: VaultProgram,
  address: PublicKey,
  options: { limit?: number; before?: string } = {},
): Promise<VaultEvent[]> {
  const signatures = (await connection.getSignaturesForAddress(address, { limit: options.limit ?? 100, before: options.before }))
    .filter((s) => !s.err)
    .map((s) => s.signature);
  const events: VaultEvent[] = [];
  for (let i = 0; i < signatures.length; i += 25) {
    const chunk = signatures.slice(i, i + 25);
    const txs = await connection.getTransactions(chunk, { maxSupportedTransactionVersion: 0, commitment: "confirmed" });
    txs.forEach((tx, j) => {
      if (!tx?.meta?.logMessages) return;
      for (const e of parseEvents(program, tx.meta.logMessages)) {
        events.push({ ...e, signature: chunk[j], blockTime: tx.blockTime ?? null, slot: tx.slot });
      }
    });
  }
  return events;
}

/** Only `vault`'s events (a transaction can touch several vaults). */
export const ofVault = (events: VaultEvent[], vault: Pick<VaultInfo, "address">) =>
  events.filter((e) => e.data.basisVault?.equals?.(vault.address));

/** The newest keeper rebalance in `events`: "rebalanced 2h ago". */
export function lastRebalance(events: VaultEvent[]): VaultEvent | null {
  return events.find((e) => e.name === "rebalanced") ?? null;
}

/**
 * The share price (USD per whole share, 6 dp) just before a deposit. Shares are priced at the
 * NAV before it: `shares = value × (supply + 1e6) / (NAV + 1e6)`, so `value / shares` is the
 * share price then, to within one share unit's rounding.
 */
export function sharePriceAtDeposit(e: { valueUsd: bigint; shares: bigint }): bigint {
  return e.shares > 0n ? (e.valueUsd * 1_000_000n) / e.shares : 0n;
}

/** Growth from one share price to another over `seconds`, as an APY in percent (compounded). */
export function apyBetween(fromPrice: bigint, toPrice: bigint, seconds: number): number {
  if (fromPrice <= 0n || seconds <= 0) return 0;
  return (Math.pow(Number(toPrice) / Number(fromPrice), SECONDS_PER_YEAR / seconds) - 1) * 100;
}

/**
 * The vault's realized APY over about the last `days`: from the share price at the newest
 * deposit at least `days` old to the share price now. Null when `events` hold no deposit that
 * old; show the estimate (`yieldBreakdown`) then. A backend that snapshots `getVaultState`
 * hourly gives exact windows.
 */
export function realizedApy(
  events: VaultEvent[],
  sharePriceNow: bigint,
  days: number,
  now = Date.now() / 1000,
): { apyPct: number; fromTime: number } | null {
  const target = now - days * 86_400;
  const from = events.find((e) => e.name === "deposited" && e.blockTime !== null && e.blockTime <= target);
  if (!from) return null;
  const price = sharePriceAtDeposit({ valueUsd: big(from.data.valueUsd), shares: big(from.data.shares) });
  return { apyPct: apyBetween(price, sharePriceNow, now - from.blockTime!), fromTime: from.blockTime! };
}

// ------------------------------------------------------------------ yield

export interface YieldBreakdown {
  /** The market's funding times `fundingExposure`: funding per dollar of NAV. */
  fundingPct: number;
  /**
   * What the keeper's rebalances cost the vault (its band and Phoenix's fee on the hedge trades),
   * annualised from `rebalanced` events in the window, as a negative percent. Users' own entry
   * and exit costs (spread, Phoenix fee, Jupiter) are theirs, not the vault's: show them in the
   * deposit and withdraw previews instead.
   */
  rebalancingPct: number;
  /** The program charges no fee. */
  vaultFeePct: number;
  /** The estimated vault APY: the sum. */
  netPct: number;
}

export function yieldBreakdown(
  fundingAprPct: number,
  fundingExposure: number,
  events: VaultEvent[],
  navUsd: bigint,
  days = 7,
  now = Date.now() / 1000,
): YieldBreakdown {
  const since = now - days * 86_400;
  let cost = 0n;
  for (const e of events) {
    if (e.name !== "rebalanced" || e.blockTime === null || e.blockTime < since) continue;
    cost += big(e.data.navBeforeUsd) - big(e.data.navAfterUsd);
  }
  const fundingPct = fundingAprPct * fundingExposure;
  const rebalancingPct = navUsd > 0n ? -(Number(cost) / Number(navUsd)) * (365 / days) * 100 : 0;
  return { fundingPct, rebalancingPct, vaultFeePct: 0, netPct: fundingPct + rebalancingPct };
}

export interface VaultApy {
  apyPct: number;
  estimated: boolean;
  label: "Vault APY (7D)" | "Est. APY";
}

export function vaultApy(apy7d: number | null, yield7d: Pick<YieldBreakdown, "netPct">): VaultApy {
  if (apy7d !== null) return { apyPct: apy7d, estimated: false, label: "Vault APY (7D)" };
  return { apyPct: yield7d.netPct, estimated: true, label: "Est. APY" };
}

// ------------------------------------------------------------------ deposit panel

/**
 * About the shares a deposit of `usdcAmount` mints, without a simulation: its value less
 * `costBps` (spread, Phoenix's fee and the swap; about 10 bps) at the share price now. For the
 * exact number, `prepareDeposit` simulates the deposit.
 */
export function estimateDepositShares(state: Pick<VaultState, "nav" | "supply">, usdcAmount: bigint, costBps = 10): bigint {
  const value = (usdcAmount * (BPS - BigInt(costBps))) / BPS;
  return sharesForValue(value, state.nav, state.supply);
}

/**
 * "Net APY" and "Est. earnings" for `amountUsd` over `days` at the vault's `apyPct`, less
 * `borrowAprPct` when the deposit is made with borrowed USDC (the Vanna credit layer's rate).
 */
export function estimateEarnings(
  amountUsd: bigint,
  apyPct: number,
  days = 365,
  borrowAprPct = 0,
): { netApyPct: number; earningsUsd: bigint } {
  const netApyPct = apyPct - borrowAprPct;
  const earnings = Number(amountUsd) * (netApyPct / 100) * (days / 365);
  return { netApyPct, earningsUsd: BigInt(Math.round(earnings)) };
}

/** A wallet's USDC balance (6 dp), 0 without an account: the deposit panel's balance. */
export async function getUsdcBalance(connection: Connection, vault: Pick<VaultInfo, "usdcMint">, owner: PublicKey): Promise<bigint> {
  const account = tokenAccount(owner, vault.usdcMint, TOKEN_PROGRAM_ID);
  const balance = await connection.getTokenAccountBalance(account).catch(() => null);
  return balance ? BigInt(balance.value.amount) : 0n;
}

// ------------------------------------------------------------------ user

export interface UserSummary {
  shares: bigint;
  /** What the shares are worth now, before the costs of leaving. */
  valueUsd: bigint;
  /** USDC the user put in and took out, from their own deposit and withdrawal events. */
  depositedUsdc: bigint;
  withdrawnUsdc: bigint;
  /**
   * value + withdrawn − deposited: what they have earned so far. Shares received by transfer
   * count in the value but not in the deposits, so this overstates their earnings.
   */
  earningsUsd: bigint;
  events: VaultEvent[];
}

/** "Your Position" and "Earnings". Reads the user's last `limit` vault transactions. */
export async function getUserSummary(
  connection: Connection,
  program: VaultProgram,
  vault: VaultInfo,
  state: VaultState,
  user: PublicKey,
  limit = 200,
): Promise<UserSummary> {
  const [position, all] = await Promise.all([
    getUserPosition(connection, vault, state, user),
    fetchEvents(connection, program, userShareAccount(user, vault.shareMint), { limit }),
  ]);
  const events = ofVault(all, vault).filter((e) => e.data.user?.equals?.(user));
  let depositedUsdc = 0n;
  let withdrawnUsdc = 0n;
  for (const e of events) {
    if (e.name === "deposited") depositedUsdc += big(e.data.usdcAmount);
    if (e.name === "withdrawn") withdrawnUsdc += big(e.data.usdcOut);
  }
  return {
    shares: position.shares,
    valueUsd: position.valueUsd,
    depositedUsdc,
    withdrawnUsdc,
    earningsUsd: position.valueUsd + withdrawnUsdc - depositedUsdc,
    events,
  };
}

// ------------------------------------------------------------------ the whole page

export interface VaultPage {
  state: VaultState;
  positions: VaultPositions;
  capacity: VaultCapacity;
  funding: FundingStats;
  market: MarketInfo;
  /** From the share price history in the events read; null without a deposit that old. */
  apy7d: number | null;
  apy30d: number | null;
  /** Estimated from the last 7 days of funding and rebalances. */
  yield7d: YieldBreakdown;
  apy: VaultApy;
  lastRebalance: VaultEvent | null;
  /** The vault's recent events, newest first: "All Transactions". */
  events: VaultEvent[];
}

/** Everything the vault page shows, read in parallel. `payer` is any funded account (it signs nothing). */
export async function getVaultPage(
  connection: Connection,
  program: VaultProgram,
  vault: VaultInfo,
  payer: PublicKey,
  phoenix: PhoenixKeys = PHOENIX_MAINNET,
  eventLimit = 200,
): Promise<VaultPage> {
  const symbol = vaultSymbol(vault);
  const [state, history, market, all] = await Promise.all([
    getVaultState(connection, vault, payer, phoenix),
    fetchFundingHistory(symbol, 30),
    fetchMarketInfo(symbol),
    fetchEvents(connection, program, vault.address, { limit: eventLimit }),
  ]);
  const events = ofVault(all, vault);
  const funding = fundingStats(history);
  const positions = vaultPositions(vault, state);
  const apy7d = realizedApy(events, state.sharePrice, 7)?.apyPct ?? null;
  const yield7d = yieldBreakdown(funding.avg7dApr, positions.fundingExposure, events, state.nav, 7);
  return {
    state,
    positions,
    capacity: vaultCapacity(vault, state, market.openInterestLots),
    funding,
    market,
    apy7d,
    apy30d: realizedApy(events, state.sharePrice, 30)?.apyPct ?? null,
    yield7d,
    apy: vaultApy(apy7d, yield7d),
    lastRebalance: lastRebalance(events),
    events,
  };
}

function big(v: any): bigint {
  return BigInt(v.toString());
}
