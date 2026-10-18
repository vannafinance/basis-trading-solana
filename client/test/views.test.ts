// Checks the vault page's getters on known numbers: the SOL vault's $1,500 at $150 from the
// program's tests, the UI's sample positions, Phoenix's launch caps. No network needed.
import assert from "node:assert/strict";
import { PublicKey } from "@solana/web3.js";
import {
  apyBetween,
  estimateDepositShares,
  estimateEarnings,
  FundingPoint,
  fundingStats,
  liquidationRisePct,
  MULTIPLIER_SCALE,
  realizedApy,
  sharePriceAtDeposit,
  VaultEvent,
  vaultApy,
  vaultCapacity,
  vaultPositions,
  yieldBreakdown,
} from "../src";

const usd = (n: number) => BigInt(Math.round(n * 1_000_000));
const near = (actual: number, expected: number, tolerance: number, label: string) =>
  assert.ok(Math.abs(actual - expected) <= tolerance, `${label}: ${actual} is not ${expected}`);
const ok = (label: string) => console.log(`ok  ${label}`);

const margin = (effective: number, collateral: number, maintenance: number) => ({
  collateral: usd(collateral),
  effectiveCollateral: usd(effective),
  withdrawable: 0n,
  initialMargin: 0n,
  maintenanceMargin: usd(maintenance),
  unrealizedPnl: 0n,
  discountedUnrealizedPnl: 0n,
  unsettledFunding: 0n,
  isLiquidatable: false,
});

// Funding: 30 days at 0.002% an hour, one UTC day at −0.01% an hour three weeks ago.
const now = 20_733 * 86_400;
const points: FundingPoint[] = [];
for (let h = 30 * 24; h > 0; h--) {
  const timestamp = now - h * 3_600;
  points.push({ timestamp, ratePct: h > 21 * 24 && h <= 22 * 24 ? -0.01 : 0.002 });
}
const funding = fundingStats(points, now);
near(funding.nowApr, 17.52, 1e-9, "funding now");
near(funding.avg7dApr, 17.52, 1e-9, "7-day funding");
assert.ok(funding.avg30dApr < funding.avg7dApr);
assert.equal(funding.negativeDays, 1);
ok(`funding: now ${funding.nowApr.toFixed(2)}%, 30 days ${funding.avg30dApr.toFixed(2)}%, ${funding.negativeDays}/${funding.days} negative days`);

// The SOL vault after $1,500 at $150: 10 SOL held as collateral (80% counted), 10 SOL short.
const solVault = { kind: "nativeSol" as const, assetDecimals: 9, baseLotDecimals: 2, targetMarginBps: 0 };
const solState = {
  assetUnits: 10_000_000_000n,
  assetValue: usd(1_500),
  positionLots: -1_000n,
  shortNotional: usd(1_500),
  nav: usd(1_498.73),
  phoenixEquity: usd(-1.27),
  multiplier: MULTIPLIER_SCALE,
  tokenPrice: usd(150),
  margin: margin(1_200, 0, 30),
};
const sol = vaultPositions(solVault, solState);
assert.equal(sol.long.heldIn, "phoenixCollateral");
assert.equal(sol.netDeltaPct, 0);
assert.equal(sol.hedgeRatio, 1);
near(sol.fundingExposure, 1.0008, 1e-4, "SOL vault: all of the NAV earns funding");
// (1,200 − 30) / (1,500 + 30 − 1,200): SOL can rise 354.5% before Phoenix's maintenance margin.
near(sol.liquidationRisePct!, 354.5, 0.1, "SOL vault liquidation distance");
near(Number(sol.liquidationPrice) / 1e6, 681.82, 0.01, "SOL price at liquidation");
ok(`SOL vault: delta 0%, hedge 1x, funding on ${(sol.fundingExposure * 100).toFixed(0)}% of NAV, liquidation at +${sol.liquidationRisePct!.toFixed(1)}%`);

// An ETH vault at 1x: the tokens are not on Phoenix, so only the USDC margin holds the short.
const eth = vaultPositions(
  { kind: "spot", assetDecimals: 8, baseLotDecimals: 3, targetMarginBps: 10_000 },
  {
    assetUnits: 500_000_000n,
    assetValue: usd(17_500),
    positionLots: -5_000n,
    shortNotional: usd(17_500),
    nav: usd(34_985),
    phoenixEquity: usd(17_485),
    multiplier: MULTIPLIER_SCALE,
    tokenPrice: usd(3_500),
    margin: margin(17_485, 17_485, 350),
  },
);
near(eth.liquidationRisePct!, 96, 0.1, "ETH vault liquidation distance");
near(eth.fundingExposure, 0.5, 0.001, "ETH vault: half the NAV earns funding");
near(eth.assetPct, 50, 0.1, "ETH vault: half the NAV is ETH");
ok(`ETH vault: funding on ${(eth.fundingExposure * 100).toFixed(0)}% of NAV, liquidation at +${eth.liquidationRisePct!.toFixed(1)}%`);

// The UI's sample: 4,383.6 SOL held, 4,381.0 SOL short.
const sample = vaultPositions(solVault, { ...solState, assetUnits: 4_383_600_000_000n, positionLots: -438_100n });
near(sample.netDeltaPct, 0.0593, 1e-4, "sample net delta");
ok(`UI sample: net delta +${sample.netDeltaPct.toFixed(2)}%`);

// Launch caps: the SOL vault stops at 450 SOL ($67,500 at $150) before $100,000 or 25% of OI.
const cap = vaultCapacity(
  { maxPositionUsd: usd(100_000), maxAssetUnits: 450_000_000_000n, maxOiShareBps: 2_500, targetMarginBps: 0, assetDecimals: 9, tickSize: 10n },
  { nav: usd(1_498.73), tokenPrice: usd(150), markPriceTicks: 150_000n },
  6_513_227n,
);
assert.equal(cap.binding, "assetUnits");
assert.equal(cap.capUsd, usd(67_500));
assert.equal(cap.roomUsd, usd(66_001.27));
const ethCap = vaultCapacity(
  { maxPositionUsd: usd(300_000), maxAssetUnits: 2n ** 64n - 1n, maxOiShareBps: 2_500, targetMarginBps: 10_000, assetDecimals: 8, tickSize: 10n },
  { nav: 0n, tokenPrice: usd(3_500), markPriceTicks: 350_000n },
);
assert.equal(ethCap.capUsd, usd(600_000), "a 1x vault's NAV is twice its short");
ok(`caps: SOL $${Number(cap.capUsd) / 1e6} (${cap.binding}), ETH $${Number(ethCap.capUsd) / 1e6} (${ethCap.binding})`);

// Share price history from deposits, and the APY from it.
assert.equal(sharePriceAtDeposit({ valueUsd: usd(1_798.47), shares: usd(1_798.47) }), usd(1));
near(apyBetween(usd(1), usd(1.002), 7 * 86_400), 10.98, 0.01, "APY from +0.2% in a week");
const event = (name: string, daysAgo: number, data: any): VaultEvent => ({
  name,
  data,
  signature: name + daysAgo,
  blockTime: now - daysAgo * 86_400,
  slot: 0,
});
const events = [
  event("rebalanced", 1, { navBeforeUsd: usd(15_031.40), navAfterUsd: usd(15_031.22) }),
  event("deposited", 3, { valueUsd: usd(1_000), shares: usd(999) }),
  event("deposited", 8, { valueUsd: usd(1_500), shares: usd(1_500) }),
];
const apy = realizedApy(events, usd(1.002), 7, now)!;
near(apy.apyPct, 9.6, 0.1, "realized 7-day APY");
assert.equal(realizedApy(events, usd(1.002), 30, now), null, "no deposit 30 days old");
ok(`realized APY ${apy.apyPct.toFixed(2)}% from the deposit 8 days ago; 30 days: not enough history`);

// The yield: 20.33% funding on the whole NAV, less what the keeper's rebalances cost.
const y = yieldBreakdown(20.33, 1, events, usd(15_031.22), 7, now);
near(y.fundingPct, 20.33, 1e-9, "funding");
near(y.rebalancingPct, -0.0624, 1e-3, "rebalancing");
assert.equal(y.vaultFeePct, 0);
ok(`yield: funding ${y.fundingPct.toFixed(2)}%, rebalancing ${y.rebalancingPct.toFixed(3)}%, no fee`);

assert.deepEqual(vaultApy(apy.apyPct, y), { apyPct: apy.apyPct, estimated: false, label: "Vault APY (7D)" });
const newVault = vaultApy(realizedApy(events.slice(0, 2), usd(1.002), 7, now)?.apyPct ?? null, y);
assert.deepEqual(newVault, { apyPct: y.netPct, estimated: true, label: "Est. APY" });
assert.equal(vaultApy(0, y).estimated, false, "a flat share price is a real 0%");
ok(`vault APY: ${apy.apyPct.toFixed(2)}% realized; a vault under 7 days old shows ${newVault.apyPct.toFixed(2)}% estimated`);

// The deposit panel.
assert.equal(estimateDepositShares({ nav: 0n, supply: 0n }, usd(1_500)), usd(1_498.5));
const e = estimateEarnings(usd(1_500), 20, 365, 2.12);
near(e.netApyPct, 17.88, 1e-9, "net APY");
assert.equal(e.earningsUsd, usd(268.2));
ok("deposit panel: $1,500 → about 1,498.5 shares; at 20% less a 2.12% borrow: 17.88%, $268.20 a year");

void PublicKey;
