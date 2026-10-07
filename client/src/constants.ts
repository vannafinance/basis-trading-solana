import { PublicKey } from "@solana/web3.js";
import { NATIVE_MINT, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID } from "@solana/spl-token";

/** The vault program. */
export const VAULT_PROGRAM_ID = new PublicKey("GtuQoYxzytGKtLqQGkYrVbsaPHTZN5jboP25DrSmKvGr");

// Phoenix perpetuals (mainnet) and its view and USDC-gateway programs.
export const PHOENIX_PROGRAM_ID = new PublicKey("EtrnLzgbS7nMMy5fbD42kXiUzGg8XQzJ972Xtk1cjWih");
export const PHOENIX_LOG_AUTHORITY = new PublicKey("GdxfTLSsdSY37G6fZoYtdGDSfgFnbT2EmRpuePZxWShS");
export const PHOENIX_GLOBAL_CONFIG = new PublicKey("2zskx2iyCvb6Stg7RBZkt1f6MrF4dpYtMG3yMvKwqtUZ");
export const HAWKEYE_PROGRAM_ID = new PublicKey("RiSeVw3ZjNfsaXPRb4mgaqYaEEt41pNNJoDvVh7pgQj");
export const EMBER_PROGRAM_ID = new PublicKey("EMBERpYNE6ehWmXymZZS2skiFmCa9V5dp14e1iduM5qy");
/** Ember's state and USDC vault: PDAs `[phoenix program, "state" | "vault"]` of Ember. */
export const EMBER_STATE = PublicKey.findProgramAddressSync(
  [PHOENIX_PROGRAM_ID.toBuffer(), Buffer.from("state")],
  EMBER_PROGRAM_ID,
)[0];
export const EMBER_VAULT = PublicKey.findProgramAddressSync(
  [PHOENIX_PROGRAM_ID.toBuffer(), Buffer.from("vault")],
  EMBER_PROGRAM_ID,
)[0];

/** The only swap venue the vault accepts. */
export const JUPITER_PROGRAM_ID = new PublicKey("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4");
export const JUPITER_API = "https://lite-api.jup.ag/swap/v1";

export const USDC_MINT = new PublicKey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
/** Phoenix's collateral token, minted 1:1 for USDC by Ember. */
export const PHUSD_MINT = new PublicKey("PhUsd11YkbjSaWjFncfAAmatntsjx3MgDR9B6g1ks3A");
export { NATIVE_MINT };

export const USDC_DECIMALS = 6;
export const SHARE_DECIMALS = 6;
export const BPS = 10_000n;
/** The share price's virtual offset: 1e6 shares and $1, on both deposit and withdrawal. */
export const VIRTUAL_SHARES = 1_000_000n;
export const VIRTUAL_USD = 1_000_000n;
/** A deposit without a swap must be within this share of the vault's asset/USDC mix. */
export const MIX_TOLERANCE_BPS = 500n;

/**
 * Phoenix's accounts the vault's instructions name. The trader index and buffer lists grow as
 * Phoenix grows: read them fresh with `fetchPhoenixKeys` rather than relying on these.
 */
export interface PhoenixKeys {
  perpAssetMap: PublicKey;
  withdrawQueue: PublicKey;
  globalVault: PublicKey;
  globalTraderIndex: PublicKey[];
  activeTraderBuffer: PublicKey[];
}

/** Phoenix mainnet, read 2026-10-07 from `GET https://perp-api.phoenix.trade/v1/view/exchange`. */
export const PHOENIX_MAINNET: PhoenixKeys = {
  perpAssetMap: new PublicKey("2nHGAaEw3D5dd4hVueaUNoygkQFmoeKqRQWnSPqSMFUC"),
  withdrawQueue: new PublicKey("3c3NTwpg7yW91FxijkHBXwVH1xUifun3Z8TC5eW5Si3K"),
  globalVault: new PublicKey("csZXgw2G58hbiWc9ndxaxrQVYVvqdXgQzYLuznEzHJu"),
  globalTraderIndex: [new PublicKey("HCrPXLByGqRh2szQi3gj7oRdRVBNi1gccAyn4CQCT3HK")],
  activeTraderBuffer: [new PublicKey("2U32rSzzrQS3eVmGHsnbw5kcqKF3wQXpHGd3hMq5YJok")],
};

export type AssetSymbol = "SOL" | "ETH" | "BTC" | "TSLA" | "META";

export interface AssetInfo {
  symbol: AssetSymbol;
  /** The token the vault holds; the native mint for SOL. */
  mint: PublicKey;
  decimals: number;
  tokenProgram: PublicKey;
  /** The Phoenix market the vault shorts. */
  phoenixSymbol: string;
  /** A tokenized stock: Token-2022, a dividend multiplier, US market hours. */
  stock: boolean;
}

export const ASSETS: Record<AssetSymbol, AssetInfo> = {
  SOL: { symbol: "SOL", mint: NATIVE_MINT, decimals: 9, tokenProgram: TOKEN_PROGRAM_ID, phoenixSymbol: "SOL", stock: false },
  ETH: {
    symbol: "ETH",
    mint: new PublicKey("7vfCXTUXx5WJV5JADk17DUJ4ksgau7utNKj4b963voxs"),
    decimals: 8,
    tokenProgram: TOKEN_PROGRAM_ID,
    phoenixSymbol: "ETH",
    stock: false,
  },
  BTC: {
    symbol: "BTC",
    mint: new PublicKey("cbbtcf3aa214zXHbiAZQwf4122FBYbraNdFqgw4iMij"),
    decimals: 8,
    tokenProgram: TOKEN_PROGRAM_ID,
    phoenixSymbol: "BTC",
    stock: false,
  },
  TSLA: {
    symbol: "TSLA",
    mint: new PublicKey("XsDoVfqeBukxuZHWhdvWHBhgEHjGNst4MLodqsJHzoB"),
    decimals: 8,
    tokenProgram: TOKEN_2022_PROGRAM_ID,
    phoenixSymbol: "TSLA",
    stock: true,
  },
  META: {
    symbol: "META",
    mint: new PublicKey("Xsa62P5mvPszXL1krVUnU5ar38bBSVcWAB6fmPCo5Zu"),
    decimals: 8,
    tokenProgram: TOKEN_2022_PROGRAM_ID,
    phoenixSymbol: "META",
    stock: true,
  },
};

/** Phoenix's public REST API (market data, exchange keys). */
export const PHOENIX_API = "https://perp-api.phoenix.trade";
