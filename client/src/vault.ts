import { BN, Program, Provider } from "@anchor-lang/core";
import {
  AccountMeta,
  Connection,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
} from "@solana/web3.js";
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  createAssociatedTokenAccountIdempotentInstruction,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import idl from "./idl/vanna_phoenix_vault.json";
import type { VannaPhoenixVault } from "./idl/vanna_phoenix_vault";
import {
  EMBER_PROGRAM_ID,
  EMBER_STATE,
  EMBER_VAULT,
  HAWKEYE_PROGRAM_ID,
  JUPITER_PROGRAM_ID,
  PHOENIX_GLOBAL_CONFIG,
  PHOENIX_LOG_AUTHORITY,
  PHOENIX_MAINNET,
  PHOENIX_PROGRAM_ID,
  PhoenixKeys,
} from "./constants";
import { basisVaultAddress, configAddress, tokenAccount, userShareAccount, vaultWsolAccount } from "./pda";
import type { SwapRoute } from "./jupiter";

export type VaultProgram = Program<VannaPhoenixVault>;

/**
 * The program, for building instructions and decoding accounts and events. A connection is
 * enough: transactions are signed by the frontend's wallet, not by Anchor.
 */
export function vaultProgram(connection: Connection): VaultProgram {
  return new Program(idl as VannaPhoenixVault, { connection } as Provider);
}

/** Everything about a vault the frontend needs, from its account and the protocol config. */
export interface VaultInfo {
  address: PublicKey;
  kind: "nativeSol" | "spot";
  assetMint: PublicKey;
  assetDecimals: number;
  assetTokenProgram: PublicKey;
  shareMint: PublicKey;
  authority: PublicKey;
  usdcAccount: PublicKey;
  phoenixAccount: PublicKey;
  /** The vault's asset token account; null for the SOL vault (its SOL sits in Phoenix). */
  assetAccount: PublicKey | null;
  traderAccount: PublicKey;
  assetId: number;
  orderbook: PublicKey;
  splineCollection: PublicKey;
  baseLotDecimals: number;
  /** Quote lots (USD 1e-6) per base lot per price tick. */
  tickSize: bigint;
  // Caps and settings (USD amounts in 6 decimals).
  maxPositionUsd: bigint;
  maxAssetUnits: bigint;
  maxOiShareBps: number;
  maxDepositUsd: bigint;
  slippageBps: number;
  keeperBandBps: number;
  marginBps: number;
  /** USDC set aside per dollar of asset, in bps: 10,000 (1x) for spot vaults, 0 for SOL. */
  targetMarginBps: number;
  depositsOpen: boolean;
  afterHoursDeposits: boolean;
  // From the protocol config.
  usdcMint: PublicKey;
  phoenixMint: PublicKey;
  paused: boolean;
  swapProgram: PublicKey;
}

const bigint = (n: BN) => BigInt(n.toString());

/** Builds `VaultInfo` from decoded `BasisVault` and `Config` accounts. */
export function toVaultInfo(address: PublicKey, v: any, config: any): VaultInfo {
  const spot = "spot" in v.kind;
  return {
    address,
    kind: spot ? "spot" : "nativeSol",
    assetMint: v.assetMint,
    assetDecimals: v.assetDecimals,
    assetTokenProgram: v.assetTokenProgram,
    shareMint: v.shareMint,
    authority: v.authority,
    usdcAccount: v.usdcAccount,
    phoenixAccount: v.phoenixAccount,
    assetAccount: spot ? v.assetAccount : null,
    traderAccount: v.traderAccount,
    assetId: v.assetId,
    orderbook: v.orderbook,
    splineCollection: v.splineCollection,
    baseLotDecimals: v.baseLotDecimals,
    tickSize: bigint(v.tickSize),
    maxPositionUsd: bigint(v.maxPositionUsd),
    maxAssetUnits: bigint(v.maxAssetUnits),
    maxOiShareBps: v.maxOiShareBps,
    maxDepositUsd: bigint(v.maxDepositUsd),
    slippageBps: v.slippageBps,
    keeperBandBps: v.keeperBandBps,
    marginBps: v.marginBps,
    targetMarginBps: v.targetMarginBps,
    depositsOpen: v.depositsOpen,
    afterHoursDeposits: v.afterHoursDeposits,
    usdcMint: config.usdcMint,
    phoenixMint: config.phoenixMint,
    paused: config.paused,
    swapProgram: config.swapProgram,
  };
}

/** Reads an asset's vault (pass the asset's mint; the native mint for SOL). */
export async function fetchVault(program: VaultProgram, assetMint: PublicKey): Promise<VaultInfo> {
  const address = basisVaultAddress(assetMint);
  const [vault, config] = await Promise.all([
    program.account.basisVault.fetch(address),
    program.account.config.fetch(configAddress()),
  ]);
  return toVaultInfo(address, vault, config);
}

/** The accounts every vault instruction shares (`VaultCore` in the program). */
export function coreAccounts(vault: VaultInfo, phoenix: PhoenixKeys) {
  return {
    config: configAddress(),
    basisVault: vault.address,
    authority: vault.authority,
    shareMint: vault.shareMint,
    usdcMint: vault.usdcMint,
    phoenixMint: vault.phoenixMint,
    vaultUsdc: vault.usdcAccount,
    vaultPhoenix: vault.phoenixAccount,
    vaultAsset: vault.assetAccount,
    assetMint: vault.assetMint,
    assetTokenProgram: vault.assetTokenProgram,
    traderAccount: vault.traderAccount,
    phoenix: {
      phoenixProgram: PHOENIX_PROGRAM_ID,
      logAuthority: PHOENIX_LOG_AUTHORITY,
      globalConfig: PHOENIX_GLOBAL_CONFIG,
      hawkeyeProgram: HAWKEYE_PROGRAM_ID,
      emberProgram: EMBER_PROGRAM_ID,
      emberState: EMBER_STATE,
      emberVault: EMBER_VAULT,
    },
    perpAssetMap: phoenix.perpAssetMap,
    orderbook: vault.orderbook,
    splineCollection: vault.splineCollection,
    withdrawQueue: phoenix.withdrawQueue,
    globalVault: phoenix.globalVault,
  };
}

/** Phoenix's trader index accounts, which lead `remainingAccounts`, then the route's. */
function remainingAccounts(phoenix: PhoenixKeys, route?: SwapRoute): AccountMeta[] {
  const index = [...phoenix.globalTraderIndex, ...phoenix.activeTraderBuffer].map((pubkey) => ({
    pubkey,
    isSigner: false,
    isWritable: true,
  }));
  return [...index, ...(route?.metas ?? [])];
}

export interface DepositParams {
  vault: VaultInfo;
  user: PublicKey;
  /** In the asset's smallest units (lamports for SOL). */
  assetAmount: bigint;
  /** In USDC's smallest units (6 decimals). */
  usdcAmount: bigint;
  /** The least shares the user accepts; the transaction fails below it. */
  minShares: bigint;
  /** A Jupiter route, for a single-asset deposit (see `planDepositSwap`). */
  route?: SwapRoute;
  phoenix?: PhoenixKeys;
}

/** The `deposit` instruction. The program creates the user's share account if needed. */
export async function depositInstruction(
  program: VaultProgram,
  p: DepositParams,
): Promise<TransactionInstruction> {
  const phoenix = p.phoenix ?? PHOENIX_MAINNET;
  const { vault, user, route } = p;
  const spot = vault.kind === "spot";
  return program.methods
    .deposit(
      phoenix.globalTraderIndex.length,
      phoenix.activeTraderBuffer.length,
      new BN(p.assetAmount.toString()),
      new BN(p.usdcAmount.toString()),
      new BN(p.minShares.toString()),
      route ? { accounts: route.args.accounts, data: route.args.data } : null,
    )
    .accountsStrict({
      user,
      core: coreAccounts(vault, phoenix),
      userShares: userShareAccount(user, vault.shareMint),
      // Only the accounts the deposit takes from: a USDC-only depositor needs no asset account.
      userAsset: spot && p.assetAmount > 0n ? tokenAccount(user, vault.assetMint, vault.assetTokenProgram) : null,
      userUsdc: p.usdcAmount > 0n ? tokenAccount(user, vault.usdcMint, TOKEN_PROGRAM_ID) : null,
      swapProgram: route ? JUPITER_PROGRAM_ID : null,
      vaultWsol: route && !spot ? vaultWsolAccount(vault.authority) : null,
      tokenProgram: TOKEN_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    })
    .remainingAccounts(remainingAccounts(phoenix, route))
    .instruction();
}

/** How a withdrawal pays: the user's part as it is, or all in one asset (needs a route). */
export type Receive = "both" | "asset" | "usdc";

export interface WithdrawParams {
  vault: VaultInfo;
  user: PublicKey;
  shares: bigint;
  /** Least asset (smallest units) and USDC (6 dp) the user accepts. */
  minAssetOut: bigint;
  minUsdcOut: bigint;
  receive: Receive;
  /** A Jupiter route when `receive` is `asset` or `usdc` (see `planWithdrawSwap`). */
  route?: SwapRoute;
  phoenix?: PhoenixKeys;
}

/** The `withdraw` instruction. */
export async function withdrawInstruction(
  program: VaultProgram,
  p: WithdrawParams,
): Promise<TransactionInstruction> {
  const phoenix = p.phoenix ?? PHOENIX_MAINNET;
  const { vault, user, route } = p;
  const spot = vault.kind === "spot";
  return program.methods
    .withdraw(
      phoenix.globalTraderIndex.length,
      phoenix.activeTraderBuffer.length,
      new BN(p.shares.toString()),
      new BN(p.minAssetOut.toString()),
      new BN(p.minUsdcOut.toString()),
      { [p.receive]: {} } as any,
      route ? { accounts: route.args.accounts, data: route.args.data } : null,
    )
    .accountsStrict({
      user,
      core: coreAccounts(vault, phoenix),
      userShares: userShareAccount(user, vault.shareMint),
      userAsset: spot ? tokenAccount(user, vault.assetMint, vault.assetTokenProgram) : null,
      userUsdc: tokenAccount(user, vault.usdcMint, TOKEN_PROGRAM_ID),
      swapProgram: route ? JUPITER_PROGRAM_ID : null,
      vaultWsol: route && !spot ? vaultWsolAccount(vault.authority) : null,
      tokenProgram: TOKEN_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    })
    .remainingAccounts(remainingAccounts(phoenix, route))
    .instruction();
}

/**
 * Creates (if missing) the token accounts a withdrawal pays into: the user's USDC account and,
 * for a spot vault, their asset account under the asset's token program. Put these before the
 * withdraw instruction; they cost nothing when the accounts exist.
 */
export function withdrawSetupInstructions(vault: VaultInfo, user: PublicKey): TransactionInstruction[] {
  const ixs = [
    createAssociatedTokenAccountIdempotentInstruction(
      user,
      tokenAccount(user, vault.usdcMint, TOKEN_PROGRAM_ID),
      user,
      vault.usdcMint,
      TOKEN_PROGRAM_ID,
    ),
  ];
  if (vault.kind === "spot") {
    ixs.push(
      createAssociatedTokenAccountIdempotentInstruction(
        user,
        tokenAccount(user, vault.assetMint, vault.assetTokenProgram),
        user,
        vault.assetMint,
        vault.assetTokenProgram,
      ),
    );
  }
  return ixs;
}
