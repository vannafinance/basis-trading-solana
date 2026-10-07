// The frontend's two actions, end to end: build, preview by simulation, set the minimums, and
// return a transaction for the wallet to sign.
import { Connection, PublicKey, VersionedTransaction } from "@solana/web3.js";
import { PHOENIX_MAINNET, PhoenixKeys } from "./constants";
import { jupiterRoute, JupiterOptions, SwapRoute } from "./jupiter";
import { depositSwapAmount, lessBps, withdrawSwapEstimate } from "./math";
import { getVaultState } from "./state";
import { buildTransaction, simulate, Simulation, TxOptions } from "./transaction";
import {
  depositInstruction,
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
 * A deposit of `usdcAmount` USDC: deposits are USDC only. The part that buys the asset (half
 * for a spot vault, all of it for SOL) goes through a Jupiter route built for that exact amount.
 * `slippageBps` sets `minShares` below the simulated shares.
 */
export async function prepareDeposit(
  ctx: FlowContext,
  user: PublicKey,
  usdcAmount: bigint,
  slippageBps = 50,
): Promise<PreparedTransaction & { shares: bigint; valueUsd: bigint; assetBought: bigint }> {
  const phoenix = ctx.phoenix ?? PHOENIX_MAINNET;
  const amount = depositSwapAmount(usdcAmount, ctx.vault.targetMarginBps);
  const route = await jupiterRoute(ctx.vault, "usdc", amount, ctx.jupiter);
  const tables = [...ctx.vaultLookupTables, ...route.lookupTables];
  const build = async (minShares: bigint) =>
    buildTransaction(
      ctx.connection,
      user,
      [await depositInstruction(ctx.program, { vault: ctx.vault, user, usdcAmount, minShares, route, phoenix })],
      tables,
      { computeUnits: 600_000, ...ctx.tx },
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
    assetBought: BigInt(deposited.data.assetBought.toString()),
  };
}

/**
 * A withdrawal of `shares`, paid in USDC only. The vault sells the user's part of the asset
 * through Jupiter: the route is built for about that amount (`withdrawSwapEstimate`), and the
 * vault resizes it to the exact amount inside the transaction, so nothing is left unsold.
 * `slippageBps` sets `minUsdcOut` below the simulated payout.
 */
export async function prepareWithdraw(
  ctx: FlowContext,
  user: PublicKey,
  shares: bigint,
  slippageBps = 50,
): Promise<PreparedTransaction & { usdcOut: bigint; assetSold: bigint; payoutUsd: bigint; sharesBurned: bigint }> {
  const phoenix = ctx.phoenix ?? PHOENIX_MAINNET;
  const setup = withdrawSetupInstructions(ctx.vault, user);
  const build = async (route: SwapRoute | undefined, minUsdc: bigint) =>
    buildTransaction(
      ctx.connection,
      user,
      [
        ...setup,
        await withdrawInstruction(ctx.program, {
          vault: ctx.vault,
          user,
          shares,
          minUsdcOut: minUsdc,
          route,
          phoenix,
        }),
      ],
      [...ctx.vaultLookupTables, ...(route?.lookupTables ?? [])],
      { computeUnits: route ? 600_000 : 400_000, ...ctx.tx },
    );

  const state = await getVaultState(ctx.connection, ctx.vault, user, phoenix);
  const estimate = withdrawSwapEstimate(
    ctx.vault.kind,
    shares,
    state.supply,
    state.assetUnits,
    state.nav,
    state.tokenPrice,
    ctx.vault.assetDecimals,
  );
  let route = estimate > 0n ? await jupiterRoute(ctx.vault, "asset", estimate, ctx.jupiter) : undefined;
  let preview = await simulate(ctx.connection, ctx.program, await build(route, 0n));
  if (!preview.ok && route && preview.error?.name === "SwapNotNeeded") {
    // Phoenix lets no asset leave now (the SOL vault's SOL held for USDC it owes): USDC only.
    route = undefined;
    preview = await simulate(ctx.connection, ctx.program, await build(route, 0n));
  }
  const out = preview.events.find((e) => e.name === "withdrawn");
  if (!preview.ok || !out) throw new VaultActionError(preview);
  const usdcOut = BigInt(out.data.usdcOut.toString());
  return {
    transaction: await build(route, lessBps(usdcOut, slippageBps)),
    preview,
    route,
    usdcOut,
    assetSold: BigInt(out.data.assetSold.toString()),
    payoutUsd: BigInt(out.data.payoutUsd.toString()),
    sharesBurned: BigInt(out.data.shares.toString()),
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
