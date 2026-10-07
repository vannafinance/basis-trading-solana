// The program's own arithmetic (src/math.rs, src/nav.rs, src/swap.rs), in bigint, so the
// frontend computes exactly what the program will: USD values have 6 decimals.
import { BPS, MIX_TOLERANCE_BPS, VIRTUAL_SHARES, VIRTUAL_USD } from "./constants";

const pow10 = (n: number) => 10n ** BigInt(n);
const ceilDiv = (a: bigint, b: bigint) => (a + b - 1n) / b;

/** Fixed-point scale of a token's dividend multiplier (1e12 = one share per token). */
export const MULTIPLIER_SCALE = 1_000_000_000_000n;

/**
 * USD (6 dp) per whole asset token at Phoenix mark `markTicks`: the market's price times the
 * token's multiplier (above 1 for a stock token that reinvested dividends).
 */
export function tokenPrice(
  markTicks: bigint,
  tickSize: bigint,
  baseLotDecimals: number,
  multiplier: bigint = MULTIPLIER_SCALE,
): bigint {
  return (markTicks * tickSize * pow10(baseLotDecimals) * multiplier) / MULTIPLIER_SCALE;
}

/** USD (6 dp) of `amount` smallest units of a token with `decimals`, at `price`. */
export const usdValue = (amount: bigint, price: bigint, decimals: number, roundUp = false) =>
  roundUp ? ceilDiv(amount * price, pow10(decimals)) : (amount * price) / pow10(decimals);

/** Token amount (smallest units) worth `usd` (6 dp) at `price`. */
export const tokensForUsd = (usd: bigint, price: bigint, decimals: number) =>
  (usd * pow10(decimals)) / price;

export type Sell = "usdc" | "asset";

/**
 * How much of a deposit the vault swaps to reach its mix (`swap::plan_deposit_swap`), or null
 * when it is already there. USDC alone swaps `usdc × 10,000 / (10,000 + t)`; the asset alone
 * swaps `asset × t / (10,000 + t)`, with `t` the vault's `targetMarginBps`. Neither depends on
 * the price, so the route can be built for the exact amount.
 */
export function planDepositSwap(
  price: bigint,
  decimals: number,
  targetMarginBps: number,
  assetAmount: bigint,
  usdcAmount: bigint,
): { sell: Sell; amount: bigint } | null {
  const t = BigInt(targetMarginBps);
  let sell: Sell;
  let amount: bigint;
  if (assetAmount === 0n) {
    [sell, amount] = ["usdc", (usdcAmount * BPS) / (BPS + t)];
  } else if (usdcAmount === 0n) {
    [sell, amount] = ["asset", (assetAmount * t) / (BPS + t)];
  } else {
    const usdc = usdcAmount * BPS;
    const wanted = usdValue(assetAmount, price, decimals) * t;
    if (usdc >= wanted) {
      [sell, amount] = ["usdc", (usdc - wanted) / (BPS + t)];
    } else {
      [sell, amount] = ["asset", tokensForUsd((wanted - usdc) / (BPS + t), price, decimals)];
    }
  }
  return amount === 0n ? null : { sell, amount };
}

/** The USDC that goes with `assetAmount` in a deposit at the vault's mix (no swap needed). */
export const mixUsdc = (assetAmount: bigint, price: bigint, decimals: number, targetMarginBps: number) =>
  (usdValue(assetAmount, price, decimals) * BigInt(targetMarginBps)) / BPS;

/** Whether a deposit without a route is close enough to the mix for the program (5%). */
export function onMix(
  price: bigint,
  decimals: number,
  targetMarginBps: number,
  assetAmount: bigint,
  usdcAmount: bigint,
): boolean {
  const plan = planDepositSwap(price, decimals, targetMarginBps, assetAmount, usdcAmount);
  if (!plan) return true;
  const off = plan.sell === "usdc" ? plan.amount : usdValue(plan.amount, price, decimals, true);
  return off * BPS <= (usdValue(assetAmount, price, decimals) + usdcAmount) * MIX_TOLERANCE_BPS;
}

/** Shares a deposit adding `value` (USD, 6 dp) mints in a vault worth `nav` with `supply`. */
export const sharesForValue = (value: bigint, nav: bigint, supply: bigint) =>
  (value * (supply + VIRTUAL_SHARES)) / (nav + VIRTUAL_USD);

/** What `shares` are worth (USD, 6 dp) at the price the program pays them out at, before costs. */
export const shareValue = (shares: bigint, nav: bigint, supply: bigint) =>
  (shares * (nav + VIRTUAL_USD)) / (supply + VIRTUAL_SHARES);

/** `amount` less `bps`: a minimum-out from a simulated amount. */
export const lessBps = (amount: bigint, bps: number) => (amount * (BPS - BigInt(bps))) / BPS;
