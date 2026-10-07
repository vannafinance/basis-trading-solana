// The vault's address lookup table: every account its deposits and withdrawals name that is the
// same for all users. Created once per vault by Vanna (ops), then listed in the frontend config.
import { AddressLookupTableProgram, Connection, PublicKey, SystemProgram, TransactionInstruction } from "@solana/web3.js";
import { ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_PROGRAM_ID } from "@solana/spl-token";
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
  VAULT_PROGRAM_ID,
} from "./constants";
import { configAddress, vaultWsolAccount } from "./pda";
import type { VaultInfo } from "./vault";

/** The accounts a vault's transactions share, for its lookup table. */
export function vaultTableAddresses(vault: VaultInfo, phoenix: PhoenixKeys = PHOENIX_MAINNET): PublicKey[] {
  const keys = [
    VAULT_PROGRAM_ID,
    configAddress(),
    vault.address,
    vault.authority,
    vault.shareMint,
    vault.usdcMint,
    vault.phoenixMint,
    vault.usdcAccount,
    vault.phoenixAccount,
    vault.assetMint,
    vault.assetTokenProgram,
    vault.traderAccount,
    vault.orderbook,
    vault.splineCollection,
    PHOENIX_PROGRAM_ID,
    PHOENIX_LOG_AUTHORITY,
    PHOENIX_GLOBAL_CONFIG,
    HAWKEYE_PROGRAM_ID,
    EMBER_PROGRAM_ID,
    EMBER_STATE,
    EMBER_VAULT,
    phoenix.perpAssetMap,
    phoenix.withdrawQueue,
    phoenix.globalVault,
    ...phoenix.globalTraderIndex,
    ...phoenix.activeTraderBuffer,
    JUPITER_PROGRAM_ID,
    TOKEN_PROGRAM_ID,
    ASSOCIATED_TOKEN_PROGRAM_ID,
    SystemProgram.programId,
  ];
  if (vault.assetAccount) keys.push(vault.assetAccount);
  else keys.push(vaultWsolAccount(vault.authority));
  return [...new Map(keys.map((k) => [k.toBase58(), k])).values()];
}

/**
 * Instructions that create the vault's lookup table and fill it (one transaction for the
 * create and the first 20 addresses, then one per further 20). The table becomes usable one
 * slot after its last extension.
 */
export async function createVaultLookupTable(
  connection: Connection,
  authority: PublicKey,
  vault: VaultInfo,
  phoenix: PhoenixKeys = PHOENIX_MAINNET,
): Promise<{ table: PublicKey; transactions: TransactionInstruction[][] }> {
  const slot = await connection.getSlot("finalized");
  const [create, table] = AddressLookupTableProgram.createLookupTable({ authority, payer: authority, recentSlot: slot });
  const addresses = vaultTableAddresses(vault, phoenix);
  const transactions: TransactionInstruction[][] = [];
  for (let i = 0; i < addresses.length; i += 20) {
    const extend = AddressLookupTableProgram.extendLookupTable({
      lookupTable: table,
      authority,
      payer: authority,
      addresses: addresses.slice(i, i + 20),
    });
    transactions.push(i === 0 ? [create, extend] : [extend]);
  }
  return { table, transactions };
}
