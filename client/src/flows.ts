// The frontend's two actions, end to end: build, preview by simulation, set the minimums, and
// return a transaction for the wallet to sign.
import { Connection, PublicKey, VersionedTransaction } from "@solana/web3.js";
import { PHOENIX_MAINNET, PhoenixKeys } from "./constants";
import { jupiterRoute, JupiterOptions, SwapRoute } from "./jupiter";
import { lessBps, planDepositSwap } from "./math";
import { getVaultState } from "./state";
import { buildTransaction, simulate, Simulation, TxOptions } from "./transaction";
import {
  depositInstruction,
  Receive,
  VaultInfo,
  VaultProgram,
  withdrawInstruction,
  withdrawSetupInstructions,
} from "./vault";

export interface FlowContext {
  connection: Connection;
  program: VaultProgram;
  vault: VaultInfo;
  /** The vault's address lookup table(s), from the frontend config. */
  vaultLookupTables: PublicKey[];
  phoenix?: PhoenixKeys;
  jupiter?: JupiterOptions;
  tx?: TxOptions;
}

export interface PreparedTransaction {
  transaction: VersionedTransaction;
  /** The simulation the minimums were set from. */
  preview: Simulation;
  route?: SwapRoute;
}

export class VaultActionError extends Error {
  constructor(public preview: Simulation) {
    super(preview.error ? `${preview.error.name}: ${preview.error.message}` : "simulation failed");
  }
}

/**
 * A deposit of `assetAmount` and/or `usdcAmount`. Either alone is swapped to the vault's mix
 * through Jupiter; both together are taken as they come and must already be at the mix (use
 * `mixUsdc`). `slippageBps` sets `minShares` below the simulated shares.
 */
export async function prepareDeposit(
  ctx: FlowContext,
  user: PublicKey,
  assetAmount: bigint,
  usdcAmount: bigint,
  slippageBps = 50,
): Promise<PreparedTransaction & { shares: bigint; valueUsd: bigint }> {
  const phoenix = ctx.phoenix ?? PHOENIX_MAINNET;
  const single = assetAmount === 0n || usdcAmount === 0n;
  let route: SwapRoute | undefined;
  if (single) {
    // A single asset needs no price to plan: the split is a fixed fraction.
    const plan = planDepositSwap(1n, ctx.vault.assetDecimals, ctx.vault.targetMarginBps, assetAmount, usdcAmount);
    if (plan) route = await jupiterRoute(ctx.vault, plan.sell, plan.amount, ctx.jupiter);
  }
  const tables = [...ctx.vaultLookupTables, ...(route?.lookupTables ?? [])];
  const build = async (minShares: bigint) =>
    buildTransaction(
      ctx.connection,
      user,
      [await depositInstruction(ctx.program, { vault: ctx.vault, user, assetAmount, usdcAmount, minShares, route, phoenix })],
      tables,
      { computeUnits: route ? 600_000 : 400_000, ...ctx.tx },
    );

  const preview = await simulate(ctx.connection, ctx.program, await build(0n));
  const deposited = preview.events.find((e) => e.name === "deposited");
  if (!preview.ok || !deposited) throw new VaultActionError(preview);
  const shares = BigInt(deposited.data.shares.toString());
  return {
    transaction: await build(lessBps(shares, slippageBps)),
    preview,
    route,
    shares,
    valueUsd: BigInt(deposited.data.valueUsd.toString()),
  };
}

/**
 * A withdrawal of `shares`. `both` pays the user's part of the asset and USDC as they are; `asset`
 * or `usdc` swaps the other part through Jupiter. The vault decides how much it sells inside the
 * transaction, so the route is built from a simulation, a hair under it; whatever it leaves
 * unsold is paid as it is.
 */
export async function prepareWithdraw(
  ctx: FlowContext,
  user: PublicKey,
  shares: bigint,
  receive: Receive,
  slippageBps = 50,
): Promise<PreparedTransaction & { assetOut: bigint; usdcOut: bigint; payoutUsd: bigint; sharesBurned: bigint }> {
  const phoenix = ctx.phoenix ?? PHOENIX_MAINNET;
  const setup = withdrawSetupInstructions(ctx.vault, user);
  const build = async (r: Receive, route: SwapRoute | undefined, minAsset: bigint, minUsdc: bigint) =>
    buildTransaction(
      ctx.connection,
      user,
      [
        ...setup,
        await withdrawInstruction(ctx.program, {
          vault: ctx.vault,
          user,
          shares,
          minAssetOut: minAsset,
          minUsdcOut: minUsdc,
          receive: r,
          route,
          phoenix,
        }),
      ],
      [...ctx.vaultLookupTables, ...(route?.lookupTables ?? [])],
      { computeUnits: route ? 600_000 : 400_000, ...ctx.tx },
    );

  // What the vault pays without a swap.
  const plain = await simulate(ctx.connection, ctx.program, await build("both", undefined, 0n, 0n));
  const out = plain.events.find((e) => e.name === "withdrawn");
  if (!plain.ok || !out) throw new VaultActionError(plain);
  let assetOut = BigInt(out.data.assetOut.toString());
  let usdcOut = BigInt(out.data.usdcOut.toString());

  let route: SwapRoute | undefined;
  if (receive === "usdc" && assetOut > 0n) {
    // A spot vault's asset part is known exactly; the SOL vault's depends on the trade's cost.
    const keep = ctx.vault.kind === "spot" ? 9_995n : 9_960n;
    route = await jupiterRoute(ctx.vault, "asset", (assetOut * keep) / 10_000n, ctx.jupiter);
  } else if (receive === "asset" && usdcOut > 0n) {
    route = await jupiterRoute(ctx.vault, "usdc", (usdcOut * 9_960n) / 10_000n, ctx.jupiter);
  }
  const r: Receive = route ? receive : "both";
  let preview = plain;
  if (route) {
    preview = await simulate(ctx.connection, ctx.program, await build(r, route, 0n, 0n));
    const swapped = preview.events.find((e) => e.name === "withdrawn");
    if (!preview.ok || !swapped) throw new VaultActionError(preview);
    assetOut = BigInt(swapped.data.assetOut.toString());
    usdcOut = BigInt(swapped.data.usdcOut.toString());
  }
  const final = preview.events.find((e) => e.name === "withdrawn")!;
  return {
    transaction: await build(r, route, lessBps(assetOut, slippageBps), lessBps(usdcOut, slippageBps)),
    preview,
    route,
    assetOut,
    usdcOut,
    payoutUsd: BigInt(final.data.payoutUsd.toString()),
    sharesBurned: BigInt(final.data.shares.toString()),
  };
}

/** Whether the deposit button should be live, and why not. */
export async function depositStatus(ctx: FlowContext, payer: PublicKey) {
  const state = await getVaultState(ctx.connection, ctx.vault, payer, ctx.phoenix);
  const reasons: string[] = [];
  if (ctx.vault.paused) reasons.push("The protocol is paused");
  if (!ctx.vault.depositsOpen) reasons.push("Deposits are closed");
  const room = ctx.vault.maxPositionUsd - state.shortNotional;
  return {
    open: reasons.length === 0,
    reasons,
    /** Short notional (USD, 6 dp) the vault can still add under its position cap. */
    positionRoomUsd: room > 0n ? room : 0n,
    /** The most one deposit may add (USD, 6 dp). */
    maxDepositUsd: ctx.vault.maxDepositUsd,
    state,
  };
}
