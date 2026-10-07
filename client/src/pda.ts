import { PublicKey } from "@solana/web3.js";
import { getAssociatedTokenAddressSync, NATIVE_MINT, TOKEN_PROGRAM_ID } from "@solana/spl-token";
import { PHOENIX_PROGRAM_ID, VAULT_PROGRAM_ID } from "./constants";

const pda = (seeds: Buffer[], programId = VAULT_PROGRAM_ID) =>
  PublicKey.findProgramAddressSync(seeds, programId)[0];

/** The protocol's config: `["config"]`. */
export const configAddress = () => pda([Buffer.from("config")]);

/** An asset's vault: `["basis_vault", asset mint]` (the native mint for SOL). */
export const basisVaultAddress = (assetMint: PublicKey) =>
  pda([Buffer.from("basis_vault"), assetMint.toBuffer()]);

/** A vault's share mint: `["share_mint", basis vault]`. */
export const shareMintAddress = (basisVault: PublicKey) =>
  pda([Buffer.from("share_mint"), basisVault.toBuffer()]);

/**
 * A vault's authority: `["authority", basis vault, nonce (u64 little-endian)]`. The nonce is
 * the one the vault was created with; read the vault account instead of deriving this.
 */
export const authorityAddress = (basisVault: PublicKey, nonce: bigint) => {
  const n = Buffer.alloc(8);
  n.writeBigUInt64LE(nonce);
  return pda([Buffer.from("authority"), basisVault.toBuffer(), n]);
};

/** The Phoenix trader account of a vault authority (Phoenix's default cross-margin account). */
export const traderAccountAddress = (authority: PublicKey) =>
  pda([Buffer.from("trader"), authority.toBuffer(), Buffer.from([0, 0])], PHOENIX_PROGRAM_ID);

/** A user's share account: their associated token account of the share mint (SPL Token). */
export const userShareAccount = (user: PublicKey, shareMint: PublicKey) =>
  getAssociatedTokenAddressSync(shareMint, user, true, TOKEN_PROGRAM_ID);

/** An owner's associated token account of `mint` under `tokenProgram`. */
export const tokenAccount = (owner: PublicKey, mint: PublicKey, tokenProgram: PublicKey) =>
  getAssociatedTokenAddressSync(mint, owner, true, tokenProgram);

/** The vault authority's wrapped-SOL account, which the SOL vault opens around a swap. */
export const vaultWsolAccount = (authority: PublicKey) =>
  getAssociatedTokenAddressSync(NATIVE_MINT, authority, true, TOKEN_PROGRAM_ID);
