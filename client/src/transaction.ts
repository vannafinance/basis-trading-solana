import { EventParser } from "@anchor-lang/core";
import {
  AddressLookupTableAccount,
  ComputeBudgetProgram,
  Connection,
  PublicKey,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import idl from "./idl/vanna_phoenix_vault.json";
import type { VaultProgram } from "./vault";

export interface TxOptions {
  /** Compute units: 400k covers a plain deposit or withdrawal, 600k one with a Jupiter route. */
  computeUnits?: number;
  /** Priority fee, in micro-lamports per compute unit. */
  microLamportsPerCu?: number;
}

async function lookupTables(connection: Connection, addresses: PublicKey[]): Promise<AddressLookupTableAccount[]> {
  const unique = [...new Map(addresses.map((a) => [a.toBase58(), a])).values()];
  const tables = await Promise.all(unique.map((a) => connection.getAddressLookupTable(a)));
  return tables.map((t) => t.value).filter((t): t is AddressLookupTableAccount => t !== null);
}

/**
 * A v0 transaction for the wallet to sign. `lookupTableAddresses` is the vault's own table
 * (VAULT_LOOKUP_TABLES in the frontend config) plus a route's Jupiter tables: a vault
 * transaction does not fit Solana's 1,232 bytes without them.
 */
export async function buildTransaction(
  connection: Connection,
  payer: PublicKey,
  instructions: TransactionInstruction[],
  lookupTableAddresses: PublicKey[],
  options: TxOptions = {},
): Promise<VersionedTransaction> {
  const budget = [ComputeBudgetProgram.setComputeUnitLimit({ units: options.computeUnits ?? 600_000 })];
  if (options.microLamportsPerCu) {
    budget.push(ComputeBudgetProgram.setComputeUnitPrice({ microLamports: options.microLamportsPerCu }));
  }
  const [{ blockhash }, tables] = await Promise.all([
    connection.getLatestBlockhash(),
    lookupTables(connection, lookupTableAddresses),
  ]);
  const message = new TransactionMessage({
    payerKey: payer,
    recentBlockhash: blockhash,
    instructions: [...budget, ...instructions],
  }).compileToV0Message(tables);
  return new VersionedTransaction(message);
}

/** A program error, by code and name (see `idl.errors`). */
export interface VaultError {
  code: number;
  name: string;
  message: string;
}

const ERRORS: VaultError[] = (idl as any).errors.map((e: any) => ({ code: e.code, name: e.name, message: e.msg }));

/** The vault program's error in a failed transaction's logs, if it was one of its own. */
export function vaultErrorFromLogs(logs: string[] = []): VaultError | null {
  for (const log of logs) {
    const named = /Error Code: (\w+)\. Error Number: (\d+)/.exec(log);
    if (named) return ERRORS.find((e) => e.code === Number(named[2])) ?? null;
  }
  return null;
}

/** The program's events in a transaction's logs: `Deposited`, `Withdrawn`, `Swapped`, ... */
export function parseEvents(program: VaultProgram, logs: string[] = []): { name: string; data: any }[] {
  const parser = new EventParser(program.programId, program.coder);
  return [...parser.parseLogs(logs)];
}

export interface Simulation {
  ok: boolean;
  logs: string[];
  unitsConsumed?: number;
  events: { name: string; data: any }[];
  error: VaultError | null;
  raw: any;
}

/** Runs a transaction without signing it: previews shares or a payout, and catches errors early. */
export async function simulate(
  connection: Connection,
  program: VaultProgram,
  tx: VersionedTransaction,
): Promise<Simulation> {
  const sim = await connection.simulateTransaction(tx, { sigVerify: false, replaceRecentBlockhash: true });
  const logs = sim.value.logs ?? [];
  return {
    ok: !sim.value.err,
    logs,
    unitsConsumed: sim.value.unitsConsumed,
    events: sim.value.err ? [] : parseEvents(program, logs),
    error: vaultErrorFromLogs(logs),
    raw: sim.value.err,
  };
}
