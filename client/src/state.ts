// Reading a vault's value the way the program does: Phoenix's Hawkeye views (simulated, no
// signature or fee) for the margin and position, the vault's token balances, the share supply.
import {
  AccountInfo,
  Connection,
  PublicKey,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import {
  HAWKEYE_PROGRAM_ID,
  PHOENIX_API,
  PHOENIX_GLOBAL_CONFIG,
  PHOENIX_MAINNET,
  PHOENIX_PROGRAM_ID,
  PhoenixKeys,
} from "./constants";
import { MULTIPLIER_SCALE, shareValue, tokenPrice, usdValue } from "./math";
import { userShareAccount } from "./pda";
import type { VaultInfo } from "./vault";

// Hawkeye instruction discriminators (phoenix-rise-ix 0.6.12).
const VIEW_MARGIN = Buffer.from("b20a7cadecd27506", "hex");
const VIEW_MARGIN_FOR_ASSET = Buffer.from("2012affea1ed4d9b", "hex");

function hawkeyeAccounts(trader: PublicKey, phoenix: PhoenixKeys) {
  return [
    PHOENIX_PROGRAM_ID,
    PHOENIX_GLOBAL_CONFIG,
    ...phoenix.globalTraderIndex,
    ...phoenix.activeTraderBuffer,
    phoenix.perpAssetMap,
    trader,
  ].map((pubkey) => ({ pubkey, isSigner: false, isWritable: false }));
}

/** Hawkeye's `view_margin` of a Phoenix trader account. */
export function viewMarginInstruction(trader: PublicKey, phoenix: PhoenixKeys): TransactionInstruction {
  return new TransactionInstruction({
    programId: HAWKEYE_PROGRAM_ID,
    keys: hawkeyeAccounts(trader, phoenix),
    data: VIEW_MARGIN,
  });
}

/** Hawkeye's `view_margin_for_asset`: the trader's position on market `assetId` and its mark. */
export function viewAssetInstruction(trader: PublicKey, assetId: number, phoenix: PhoenixKeys): TransactionInstruction {
  const params = Buffer.alloc(8);
  params.writeUInt32LE(assetId);
  return new TransactionInstruction({
    programId: HAWKEYE_PROGRAM_ID,
    keys: hawkeyeAccounts(trader, phoenix),
    data: Buffer.concat([VIEW_MARGIN_FOR_ASSET, params]),
  });
}

/** Runs one view instruction in a simulation and returns its return data. */
async function simulateView(
  connection: Connection,
  payer: PublicKey,
  ix: TransactionInstruction,
  size: number,
): Promise<Buffer> {
  const { blockhash } = await connection.getLatestBlockhash();
  const message = new TransactionMessage({ payerKey: payer, recentBlockhash: blockhash, instructions: [ix] });
  const tx = new VersionedTransaction(message.compileToV0Message());
  const sim = await connection.simulateTransaction(tx, { sigVerify: false, replaceRecentBlockhash: true });
  const data = sim.value.returnData?.data?.[0];
  if (sim.value.err || !data) {
    throw new Error(`Hawkeye view failed: ${JSON.stringify(sim.value.err)} ${sim.value.logs?.join("\n")}`);
  }
  // Solana trims trailing zero bytes from return data; the views are fixed-size.
  const bytes = Buffer.alloc(size);
  Buffer.from(data, "base64").copy(bytes);
  return bytes;
}

/** Phoenix's margin view of a trader, in USD (6 dp). */
export interface MarginView {
  collateral: bigint;
  effectiveCollateral: bigint;
  withdrawable: bigint;
  initialMargin: bigint;
  maintenanceMargin: bigint;
  unrealizedPnl: bigint;
  discountedUnrealizedPnl: bigint;
  unsettledFunding: bigint;
  isLiquidatable: boolean;
}

export async function viewMargin(
  connection: Connection,
  payer: PublicKey,
  trader: PublicKey,
  phoenix: PhoenixKeys = PHOENIX_MAINNET,
): Promise<MarginView> {
  const b = await simulateView(connection, payer, viewMarginInstruction(trader, phoenix), 112);
  return {
    isLiquidatable: b.readUInt8(14) !== 0,
    collateral: b.readBigInt64LE(16),
    effectiveCollateral: b.readBigInt64LE(24),
    withdrawable: b.readBigUInt64LE(40),
    initialMargin: b.readBigUInt64LE(48),
    maintenanceMargin: b.readBigUInt64LE(56),
    unrealizedPnl: b.readBigInt64LE(88),
    discountedUnrealizedPnl: b.readBigInt64LE(96),
    unsettledFunding: b.readBigInt64LE(104),
  };
}

/** Phoenix's view of a trader's position on one market. */
export interface AssetView {
  assetId: number;
  /** Base lots; negative is short. */
  baseLots: bigint;
  markPriceTicks: bigint;
  entryPrice: bigint;
  unrealizedPnl: bigint;
  unsettledFunding: bigint;
}

export async function viewAsset(
  connection: Connection,
  payer: PublicKey,
  trader: PublicKey,
  assetId: number,
  phoenix: PhoenixKeys = PHOENIX_MAINNET,
): Promise<AssetView> {
  const b = await simulateView(connection, payer, viewAssetInstruction(trader, assetId, phoenix), 128);
  return {
    assetId: b.readUInt32LE(8),
    baseLots: b.readBigInt64LE(24),
    markPriceTicks: b.readBigUInt64LE(40),
    entryPrice: b.readBigUInt64LE(48),
    unrealizedPnl: b.readBigInt64LE(64),
    unsettledFunding: b.readBigInt64LE(80),
  };
}

/**
 * A token's dividend multiplier over `MULTIPLIER_SCALE` (Token-2022 scaled UI amount, type 25),
 * as the program reads it: the new one once its start time has passed. 1 for other tokens.
 */
export function readMultiplier(mint: AccountInfo<Buffer>, now: number): bigint {
  const data = mint.data;
  let at = 166;
  while (data.length >= at + 4) {
    const kind = data.readUInt16LE(at);
    const len = data.readUInt16LE(at + 2);
    if (kind === 25) {
      const v = data.subarray(at + 4, at + 4 + len);
      const starts = Number(v.readBigInt64LE(40));
      const multiplier = now >= starts ? v.readDoubleLE(48) : v.readDoubleLE(32);
      return BigInt(Math.floor(multiplier * Number(MULTIPLIER_SCALE)));
    }
    at += 4 + len;
  }
  return MULTIPLIER_SCALE;
}

/** A vault's state, valued like the program values it. USD amounts have 6 decimals. */
export interface VaultState {
  markPriceTicks: bigint;
  /** USD per share (stock) or per coin at Phoenix's mark. */
  markPrice: bigint;
  /** Market units (shares) per token over `MULTIPLIER_SCALE`. */
  multiplier: bigint;
  /** USD per whole asset token: mark × multiplier. */
  tokenPrice: bigint;
  /** The asset held, in smallest units. */
  assetUnits: bigint;
  /** The short in base lots (negative). */
  positionLots: bigint;
  /** USD of the asset held. */
  assetValue: bigint;
  /** USDC collateral + unrealized PnL + unsettled funding on Phoenix (negative: owed). */
  phoenixEquity: bigint;
  nav: bigint;
  supply: bigint;
  /** USD per whole share (6 dp), at the withdrawal price. */
  sharePrice: bigint;
  /** USD notional of the short. */
  shortNotional: bigint;
  /** Phoenix effective collateral per dollar of short, in bps. */
  marginBps: bigint;
  margin: MarginView;
}

/** Reads a vault's value. `payer` is any funded account (the user's wallet): it signs nothing. */
export async function getVaultState(
  connection: Connection,
  vault: VaultInfo,
  payer: PublicKey,
  phoenix: PhoenixKeys = PHOENIX_MAINNET,
): Promise<VaultState> {
  const [margin, asset, accounts] = await Promise.all([
    viewMargin(connection, payer, vault.traderAccount, phoenix),
    viewAsset(connection, payer, vault.traderAccount, vault.assetId, phoenix),
    connection.getMultipleAccountsInfo([vault.shareMint, vault.assetMint, vault.assetAccount ?? vault.traderAccount]),
  ]);
  const [shareMintInfo, assetMintInfo, holding] = accounts;
  if (!shareMintInfo || !assetMintInfo || !holding) throw new Error("vault accounts not found");
  const supply = shareMintInfo.data.readBigUInt64LE(36);
  const multiplier = readMultiplier(assetMintInfo, Math.floor(Date.now() / 1000));
  let assetUnits: bigint;
  if (vault.assetAccount) {
    assetUnits = holding.data.readBigUInt64LE(64);
  } else {
    // The SOL vault: the trader account's lamports above its rent floor.
    const floor = BigInt(await connection.getMinimumBalanceForRentExemption(holding.data.length));
    assetUnits = BigInt(holding.lamports) > floor ? BigInt(holding.lamports) - floor : 0n;
  }
  const price = tokenPrice(asset.markPriceTicks, vault.tickSize, vault.baseLotDecimals, multiplier);
  const assetValue = usdValue(assetUnits, price, vault.assetDecimals);
  const phoenixEquity = margin.collateral + margin.unsettledFunding + margin.unrealizedPnl;
  const equity = assetValue + phoenixEquity;
  const nav = equity > 0n ? equity : 0n;
  const shortLots = asset.baseLots < 0n ? -asset.baseLots : asset.baseLots;
  const shortNotional = shortLots * asset.markPriceTicks * vault.tickSize;
  return {
    markPriceTicks: asset.markPriceTicks,
    markPrice: tokenPrice(asset.markPriceTicks, vault.tickSize, vault.baseLotDecimals),
    multiplier,
    tokenPrice: price,
    assetUnits,
    positionLots: asset.baseLots,
    assetValue,
    phoenixEquity,
    nav,
    supply,
    sharePrice: shareValue(1_000_000n, nav, supply),
    shortNotional,
    marginBps: shortNotional > 0n ? (margin.effectiveCollateral * 10_000n) / shortNotional : 0n,
    margin,
  };
}

/** A user's shares and what they are worth now, before the costs of leaving. */
export async function getUserPosition(connection: Connection, vault: VaultInfo, state: VaultState, user: PublicKey) {
  const balance = await connection.getTokenAccountBalance(userShareAccount(user, vault.shareMint)).catch(() => null);
  const shares = balance ? BigInt(balance.value.amount) : 0n;
  return { shares, valueUsd: shareValue(shares, state.nav, state.supply) };
}

/**
 * Phoenix's market data for display: open interest, its cap, funding and session status.
 * Read-only from Phoenix's API; the program enforces its own caps on-chain.
 */
export async function fetchPhoenixMarket(symbol: string): Promise<any> {
  const res = await fetch(`${PHOENIX_API}/v1/view/markets`);
  const { markets } = await res.json();
  return markets.find((m: any) => m.symbol === symbol);
}

/** Phoenix's current trader index and buffer accounts (they grow as Phoenix grows). */
export async function fetchPhoenixKeys(): Promise<PhoenixKeys> {
  const res = await fetch(`${PHOENIX_API}/v1/view/exchange`);
  const { keys } = await res.json();
  const pk = (s: string) => new PublicKey(s);
  return {
    perpAssetMap: pk(keys.perpAssetMap),
    withdrawQueue: pk(keys.withdrawQueue),
    globalVault: pk(keys.globalVault),
    globalTraderIndex: keys.globalTraderIndex.map(pk),
    activeTraderBuffer: keys.activeTraderBuffer.map(pk),
  };
}
