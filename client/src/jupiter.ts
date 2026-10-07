// Jupiter v6 routes for single-asset deposits and withdrawals. The vault runs the route itself,
// signed by its authority PDA, so the route is asked for with that PDA as the user.
import { AccountMeta, PublicKey } from "@solana/web3.js";
import { TOKEN_PROGRAM_ID } from "@solana/spl-token";
import { JUPITER_API, JUPITER_PROGRAM_ID } from "./constants";
import { tokenAccount } from "./pda";
import type { Sell } from "./math";
import type { VaultInfo } from "./vault";

/** A route the vault's `deposit`/`withdraw` take: the instruction data and its accounts. */
export interface SwapRoute {
  /** The program's `SwapRoute` argument. */
  args: { accounts: number; data: Buffer };
  /** The route's accounts, appended to `remainingAccounts` after Phoenix's index accounts. */
  metas: AccountMeta[];
  /** Jupiter's address lookup tables for this route; add them to the v0 transaction. */
  lookupTables: PublicKey[];
  inAmount: bigint;
  /** Jupiter's quote. The vault books what actually arrives, never the quote. */
  outAmount: bigint;
  quote: any;
}

export interface JupiterOptions {
  /** Default `https://lite-api.jup.ag/swap/v1`; use `https://api.jup.ag/swap/v1` with a key. */
  apiBase?: string;
  apiKey?: string;
  /** Jupiter's own floor on the route (the vault checks 1% against Phoenix's mark itself). */
  slippageBps?: number;
  /** Keeps the transaction under Solana's account limit; 25 to 32 fits a direct route. */
  maxAccounts?: number;
  /** Restrict to some AMMs, e.g. ["Raydium CLMM"] (Jupiter's labels). */
  dexes?: string[];
}

/**
 * A Jupiter route selling `amount` of `sell` (USDC, or the vault's asset) for the other side,
 * built for the vault authority as the trader. The output goes to the vault's own token account
 * (its wrapped-SOL account for the SOL vault).
 */
export async function jupiterRoute(
  vault: VaultInfo,
  sell: Sell,
  amount: bigint,
  options: JupiterOptions = {},
): Promise<SwapRoute> {
  const api = options.apiBase ?? JUPITER_API;
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (options.apiKey) headers["x-api-key"] = options.apiKey;

  // The SOL vault's asset is the native mint: its side of the swap is wrapped SOL.
  const asset = { mint: vault.assetMint, program: vault.assetTokenProgram };
  const usdc = { mint: vault.usdcMint, program: TOKEN_PROGRAM_ID };
  const [input, output] = sell === "usdc" ? [usdc, asset] : [asset, usdc];
  const destination = tokenAccount(vault.authority, output.mint, output.program);

  const query = new URLSearchParams({
    inputMint: input.mint.toBase58(),
    outputMint: output.mint.toBase58(),
    amount: amount.toString(),
    slippageBps: String(options.slippageBps ?? 100),
    maxAccounts: String(options.maxAccounts ?? 30),
  });
  if (options.dexes?.length) query.set("dexes", options.dexes.join(","));
  const quote = await (await fetch(`${api}/quote?${query}`, { headers })).json();
  if (!quote.routePlan) throw new Error(`Jupiter quote failed: ${JSON.stringify(quote)}`);

  const res = await fetch(`${api}/swap-instructions`, {
    method: "POST",
    headers,
    body: JSON.stringify({
      quoteResponse: quote,
      userPublicKey: vault.authority.toBase58(),
      // The vault wraps and unwraps SOL itself, and opens and closes its own accounts.
      wrapAndUnwrapSol: false,
      useSharedAccounts: true,
      destinationTokenAccount: destination.toBase58(),
      skipUserAccountsRpcCalls: true,
    }),
  });
  const body = await res.json();
  const ix = body.swapInstruction;
  if (!ix) throw new Error(`Jupiter swap-instructions failed: ${JSON.stringify(body)}`);
  if (ix.programId !== JUPITER_PROGRAM_ID.toBase58()) throw new Error(`unexpected swap program ${ix.programId}`);

  // Only the swap instruction: Jupiter's setup and cleanup instructions would need the PDA's
  // signature. The vault signs for its authority itself, so its signer flag is cleared here.
  const metas: AccountMeta[] = ix.accounts.map((a: any) => {
    const pubkey = new PublicKey(a.pubkey);
    return { pubkey, isSigner: a.isSigner && !pubkey.equals(vault.authority), isWritable: a.isWritable };
  });
  return {
    args: { accounts: metas.length, data: Buffer.from(ix.data, "base64") },
    metas,
    lookupTables: (body.addressLookupTableAddresses ?? []).map((a: string) => new PublicKey(a)),
    inAmount: BigInt(quote.inAmount),
    outAmount: BigInt(quote.outAmount),
    quote,
  };
}
