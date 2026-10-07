// The program's own arithmetic (src/math.rs, src/nav.rs, src/swap.rs), in bigint, so the
// frontend computes exactly what the program will: USD values have 6 decimals.
import { BPS, VIRTUAL_SHARES, VIRTUAL_USD } from "./constants";

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
 * How much of a USDC deposit buys the asset (`swap::deposit_swap_amount`): all but the margin
 * the vault keeps, `usdc × 10,000 / (10,000 + t)` with `t` the vault's `targetMarginBps`. Half
 * at 1x; all of it for the SOL vault (`t` = 0). It does not depend on the price, so the route
 * can be built for the exact amount.
 */
export const depositSwapAmount = (usdcAmount: bigint, targetMarginBps: number) =>
  (usdcAmount * BPS) / (BPS + BigInt(targetMarginBps));

/**
 * About how much of the asset a withdrawal of `shares` sells: a spot vault's part of its
 * tokens; for the SOL vault, the payout in SOL less about 0.1% for closing its part of the
 * short. The vault resizes the route to the exact amount, so this only has to be close.
 */
export function withdrawSwapEstimate(
  kind: "nativeSol" | "spot",
  shares: bigint,
  supply: bigint,
  assetUnits: bigint,
  nav: bigint,
  tokenPrice: bigint,
  decimals: number,
): bigint {
  if (supply === 0n) return 0n;
  if (kind === "spot") return (assetUnits * shares) / supply;
  const payout = (((nav * shares) / supply) * 999n) / 1_000n;
  return tokensForUsd(payout, tokenPrice, decimals);
}

/** Shares a deposit adding `value` (USD, 6 dp) mints in a vault worth `nav` with `supply`. */
export const sharesForValue = (value: bigint, nav: bigint, supply: bigint) =>
  (value * (supply + VIRTUAL_SHARES)) / (nav + VIRTUAL_USD);

/** What `shares` are worth (USD, 6 dp) at the price the program pays them out at, before costs. */
export const shareValue = (shares: bigint, nav: bigint, supply: bigint) =>
  (shares * (nav + VIRTUAL_USD)) / (supply + VIRTUAL_SHARES);

/** `amount` less `bps`: a minimum-out from a simulated amount. */
export const lessBps = (amount: bigint, bps: number) => (amount * (BPS - BigInt(bps))) / BPS;
