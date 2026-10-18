# Frontend integration: Vanna multi-strategy basis vault

This guide is for engineers building the web app for the multi-strategy vault
(`vanna_phoenix_multi_vault`). It covers every account to read, every instruction a user signs,
how to value the vault and preview amounts, and the errors to handle. The keeper bot and the
admin setup are in [KEEPER_OPERATIONS.md](KEEPER_OPERATIONS.md).

Examples use TypeScript with `@solana/web3.js` 1.x, `@solana/spl-token` and
`@anchor-lang/core` 1.x (Anchor's TypeScript client), the same stack as the single-asset vault's
client.

## Contents

1. [How the vault works](#1-how-the-vault-works)
2. [Addresses and constants](#2-addresses-and-constants)
3. [Reading the vault](#3-reading-the-vault)
4. [Valuing the vault](#4-valuing-the-vault)
5. [Building a transaction](#5-building-a-transaction)
6. [User flows](#6-user-flows)
7. [Previews and quotes](#7-previews-and-quotes)
8. [Yield, composition and history](#8-yield-composition-and-history)
9. [Vault states to show](#9-vault-states-to-show)
10. [Events](#10-events)
11. [Errors](#11-errors)
12. [Checklist](#12-checklist)

---

## 1. How the vault works

**One USDC vault, many hedged markets.** Users deposit USDC and get vault shares. A keeper bot
spreads the USDC over up to 8 live **legs**. Each leg holds a spot asset (SOL, BTC, ETH, xStocks
such as TSLAx and METAx) and shorts the same amount on Phoenix perps. The vault's value stays flat
in USD while it collects the funding that longs pay shorts.

**What users do and what the keeper does:**

| Who | Instructions |
|---|---|
| Users | `deposit`, `withdraw`, `request_withdraw`, `cancel_withdraw` (and, rarely, `fulfill_withdraw`) |
| Keeper | Everything else (deploying, unwinding, margin, re-hedging); see [KEEPER_OPERATIONS.md](KEEPER_OPERATIONS.md) |

**The buffer.** Deposits land as idle USDC in the vault's buffer; the keeper deploys them later.
Withdrawals are paid from the buffer:

- **Instant** (`withdraw`): when the buffer holds enough, above what pending requests are owed.
- **Requested** (`request_withdraw`): otherwise. The user's shares go into escrow and keep
  earning. The keeper sells part of the legs and pays the request, usually within minutes. The user
  can cancel until then.

**Share price** = NAV / share supply, where:

```
NAV = idle USDC
    + Σ legs (asset held × Phoenix mark × token multiplier)
    + Phoenix equity (USDC collateral + unrealized PnL + unsettled funding)
```

The first deposit gets 1 share per dollar. A virtual offset of 1 share / $1 on both sides of every
price stops share-price manipulation.

**Units.** Every USD amount on-chain (NAV, payouts, caps) uses 6 decimals, the same as USDC.
Shares also have 6 decimals.

---

## 2. Addresses and constants

### Programs and mints (mainnet)

| Name | Address |
|---|---|
| Vault program | `ppTyvPjN6xCEvCX7zsjPBip393fVVm8RMvkiU3Lz9WA` |
| Phoenix (perps) | `EtrnLzgbS7nMMy5fbD42kXiUzGg8XQzJ972Xtk1cjWih` |
| Phoenix log authority | `GdxfTLSsdSY37G6fZoYtdGDSfgFnbT2EmRpuePZxWShS` |
| Phoenix global config | `2zskx2iyCvb6Stg7RBZkt1f6MrF4dpYtMG3yMvKwqtUZ` |
| Hawkeye (Phoenix views) | `RiSeVw3ZjNfsaXPRb4mgaqYaEEt41pNNJoDvVh7pgQj` |
| Phoenix perp asset map | `2nHGAaEw3D5dd4hVueaUNoygkQFmoeKqRQWnSPqSMFUC` (also in `Config.perp_asset_map`) |
| USDC mint | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` |
| SPL Token / Token-2022 | `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA` / `TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb` |

**Phoenix's index accounts.** Every instruction that values the vault needs Phoenix's *global
trader index* and *active trader buffer* accounts. These are dynamic, so read them from Phoenix's
API and cache them for about 10 minutes:

```
GET https://perp-api.phoenix.trade/v1/view/exchange/keys
→ { "globalTraderIndex": ["HCrPXLByGqRh2szQi3gj7oRdRVBNi1gccAyn4CQCT3HK"],
    "activeTraderBuffer": ["2U32rSzzrQS3eVmGHsnbw5kcqKF3wQXpHGd3hMq5YJok"], ... }
```

(These are the values on 2026-10-16. If the API is down, use the last ones you read.)

### Frontend configuration

| Setting | Where it comes from |
|---|---|
| `VAULT_ID` | `u16` the admin chose at `create_vault` (part of the vault's address) |
| `VAULT_LOOKUP_TABLE` | The vault's address lookup table, from ops ([KEEPER_OPERATIONS.md §4.6](KEEPER_OPERATIONS.md#46-address-lookup-table)) |
| `RPC_URL` | A mainnet RPC that supports `simulateTransaction` with return data |
| IDL | [`client/src/idl/vanna_phoenix_multi_vault.json`](../../client/src/idl/vanna_phoenix_multi_vault.json) and TS types [`client/src/idl/vanna_phoenix_multi_vault.ts`](../../client/src/idl/vanna_phoenix_multi_vault.ts) (regenerate with `anchor idl build -p vanna_phoenix_multi_vault -o client/src/idl/vanna_phoenix_multi_vault.json -t client/src/idl/vanna_phoenix_multi_vault.ts` after any program change) |

### Program-derived addresses

| Account | Seeds (program = vault program) |
|---|---|
| Config | `["config"]` |
| Vault | `["vault", id as u16 little-endian]` |
| Leg *i* | `["leg", vault, [i as u8]]` |
| Withdraw request *n* | `["withdraw_request", vault, n as u64 little-endian]` |
| Share mint | `["share_mint", vault]` |
| Authority | `["authority", vault, nonce as u64 LE]`. Read it from `Vault.authority`; don't derive it |

The vault's own token accounts are stored in the `Vault` account; read them from there:

- `usdc_account` (the buffer);
- `phoenix_account`;
- `escrow_shares`;
- `trader_account` (its Phoenix account).

```ts
// constants.ts
import { PublicKey } from "@solana/web3.js";

export const PROGRAM_ID = new PublicKey("ppTyvPjN6xCEvCX7zsjPBip393fVVm8RMvkiU3Lz9WA");
export const PHOENIX_PROGRAM_ID = new PublicKey("EtrnLzgbS7nMMy5fbD42kXiUzGg8XQzJ972Xtk1cjWih");
export const PHOENIX_LOG_AUTHORITY = new PublicKey("GdxfTLSsdSY37G6fZoYtdGDSfgFnbT2EmRpuePZxWShS");
export const PHOENIX_GLOBAL_CONFIG = new PublicKey("2zskx2iyCvb6Stg7RBZkt1f6MrF4dpYtMG3yMvKwqtUZ");
export const HAWKEYE_PROGRAM_ID = new PublicKey("RiSeVw3ZjNfsaXPRb4mgaqYaEEt41pNNJoDvVh7pgQj");
export const USD = 1_000_000n; // 6 decimals
export const VIRTUAL = 1_000_000n; // the share price's virtual offset (1 share, $1)
export const BPS = 10_000n;

const u16le = (n: number) => { const b = Buffer.alloc(2); b.writeUInt16LE(n); return b; };
const u64le = (n: bigint) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(n); return b; };
const pda = (seeds: Buffer[]) => PublicKey.findProgramAddressSync(seeds, PROGRAM_ID)[0];

export const configAddress = () => pda([Buffer.from("config")]);
export const vaultAddress = (id: number) => pda([Buffer.from("vault"), u16le(id)]);
export const legAddress = (vault: PublicKey, index: number) =>
  pda([Buffer.from("leg"), vault.toBuffer(), Buffer.from([index])]);
export const requestAddress = (vault: PublicKey, id: bigint) =>
  pda([Buffer.from("withdraw_request"), vault.toBuffer(), u64le(id)]);
```

---

## 3. Reading the vault

```ts
import { AnchorProvider, Program } from "@anchor-lang/core";
import idl from "./idl/vanna_phoenix_multi_vault.json"; // client/src/idl
import type { VannaPhoenixMultiVault } from "./idl/vanna_phoenix_multi_vault";

const program = new Program<VannaPhoenixMultiVault>(idl as VannaPhoenixMultiVault, provider);
const vaultKey = vaultAddress(VAULT_ID);
const [config, vault] = await Promise.all([
  program.account.config.fetch(configAddress()),
  program.account.vault.fetch(vaultKey),
]);
```

Anchor's TypeScript client turns the IDL's snake_case names into camelCase
(`vault.pendingShares`, `leg.assetAccount`). The tables below use the on-chain names.

### `Config` (one per program)

| Field | Use |
|---|---|
| `admin`, `keeper` | Who may run admin and keeper instructions |
| `usdc_mint`, `phoenix_mint` | USDC and Phoenix's collateral token (PhUSD) |
| `perp_asset_map`, `withdraw_queue`, `global_vault` | Phoenix accounts the vault uses |
| `swap_program` | Jupiter v6, or the default key when swaps are off |
| `paused` | When true, deposits are refused; withdrawals still work |

### `Vault`

| Field | Use |
|---|---|
| `id` | The vault's id |
| `share_mint` | Shares (classic SPL token, 6 decimals, no freeze authority) |
| `authority` | Owner of every vault token account; the Phoenix trader |
| `usdc_account` | **The buffer**: idle USDC |
| `phoenix_account`, `escrow_shares`, `trader_account` | PhUSD account; shares of pending requests; Phoenix trader account |
| `leg_count` | Legs `0..leg_count` exist (retired ones included) |
| `next_request_id` | The id the next `request_withdraw` will get |
| `pending_shares` | Shares in live (not returned) requests: their value is reserved in the buffer |
| `max_tvl_usd`, `max_deposit_usd` | Caps (6 dp) |
| `entry_fee_bps`, `exit_fee_bps` | Fees (kept in the vault); the exit fee also applies to cancels |
| `buffer_bps` | Idle USDC the keeper keeps per dollar of NAV |
| `margin_bps`, `slippage_bps`, `max_turnover_usd`, `turnover_window_*` | Keeper limits (display only) |
| `deposits_open` | False: deposits refused (the keeper's funding guard, or the admin) |

### `Leg`

```ts
const legKeys = [...Array(vault.legCount).keys()].map((i) => legAddress(vaultKey, i));
const legs = (await program.account.leg.fetchMultiple(legKeys)).map((leg, i) => ({ address: legKeys[i], ...leg! }));
```

| Field | Use |
|---|---|
| `index`, `kind` | `{ nativeSol: {} }` (SOL held as Phoenix collateral) or `{ spot: {} }` (tokens) |
| `asset_mint`, `asset_decimals`, `asset_token_program` | The asset; Token-2022 for xStocks |
| `asset_account` | The vault's token account for a spot leg (unset for SOL) |
| `asset_id` | Phoenix market id; its symbol is in Phoenix's markets API (§8) |
| `orderbook`, `spline_collection`, `base_lot_decimals`, `tick_size` | The Phoenix market |
| `active` | False: no new deploys (it may still be unwound) |
| `retired` | True: out of the vault for good; takes one account and is valued at 0 |
| `max_notional_usd`, `max_asset_units`, `max_oi_share_bps`, `max_nav_share_bps` | Caps |
| `after_hours_trading` | Whether the keeper may deploy into a stock market after hours |

### `WithdrawRequest`

| Field | Use |
|---|---|
| `vault`, `user`, `id`, `shares`, `created_at` | The request |
| `returned` | True: the keeper gave up on paying it (the user's USDC account was closed, frozen or given away). The shares sit in escrow until the user takes them back with `cancel_withdraw`, at no fee |

A user's requests:

```ts
// WithdrawRequest layout: 8-byte discriminator, vault (8..40), user (40..72), ...
const mine = await program.account.withdrawRequest.all([
  { memcmp: { offset: 8, bytes: vaultKey.toBase58() } },
  { memcmp: { offset: 40, bytes: user.toBase58() } },
]);
```

If your RPC refuses `getProgramAccounts`, scan ids instead. Request PDAs `0..next_request_id`
exist only while open, so `getMultipleAccountsInfo` in batches of 100 works; keep track of the
lowest id still open.

---

## 4. Valuing the vault

You need NAV and supply for:

- the share price;
- a user's position value;
- whether an instant withdrawal is possible.

There are two ways to get them.

### 4.1 Exact: simulate the user's transaction (recommended for amounts shown before signing)

Build the real `deposit` or `withdraw` transaction and run `simulateTransaction`. Then read the
event the program emits (§7). The result is exactly what the program will do, multiplier windows
and fees included.

### 4.2 Live NAV for display: Phoenix views plus account reads

This is the program's own formula. The program reads Phoenix through Hawkeye, a read-only view
program. You run its views in a simulation (no signature, no fee) and read the return data.

```ts
import { Connection, PublicKey, TransactionInstruction, TransactionMessage, VersionedTransaction } from "@solana/web3.js";

const VIEW_MARGIN = Buffer.from("b20a7cadecd27506", "hex");
const VIEW_MARGIN_FOR_ASSET = Buffer.from("2012affea1ed4d9b", "hex");

function hawkeyeIx(data: Buffer, trader: PublicKey, phoenix: PhoenixKeys, perpAssetMap: PublicKey) {
  const keys = [PHOENIX_PROGRAM_ID, PHOENIX_GLOBAL_CONFIG, ...phoenix.globalTraderIndex,
    ...phoenix.activeTraderBuffer, perpAssetMap, trader]
    .map((pubkey) => ({ pubkey, isSigner: false, isWritable: false }));
  return new TransactionInstruction({ programId: HAWKEYE_PROGRAM_ID, keys, data });
}

/** Runs a view in a simulation. `payer` must be any funded account (the user, or Config.keeper). */
async function simulateView(connection: Connection, payer: PublicKey, ix: TransactionInstruction, size: number) {
  const { blockhash } = await connection.getLatestBlockhash();
  const msg = new TransactionMessage({ payerKey: payer, recentBlockhash: blockhash, instructions: [ix] });
  const sim = await connection.simulateTransaction(new VersionedTransaction(msg.compileToV0Message()),
    { sigVerify: false, replaceRecentBlockhash: true });
  const data = sim.value.returnData?.data?.[0];
  if (sim.value.err || !data) throw new Error(`Hawkeye view failed: ${JSON.stringify(sim.value.err)}`);
  const out = Buffer.alloc(size); // Solana trims trailing zero bytes from return data
  Buffer.from(data, "base64").copy(out);
  return out;
}

/** Phoenix equity of the vault's trader, USD 6 dp: collateral + unrealized PnL + unsettled funding. */
async function phoenixEquity(connection, payer, trader, phoenix, perpAssetMap) {
  const b = await simulateView(connection, payer, hawkeyeIx(VIEW_MARGIN, trader, phoenix, perpAssetMap), 112);
  return b.readBigInt64LE(16) /* collateral */ + b.readBigInt64LE(88) /* uPnL */ + b.readBigInt64LE(104) /* funding */;
}

/** The vault's position (base lots, negative = short) and Phoenix's mark (ticks) on one market. */
async function viewAsset(connection, payer, trader, assetId: number, phoenix, perpAssetMap) {
  const params = Buffer.alloc(8); params.writeUInt32LE(assetId);
  const b = await simulateView(connection, payer,
    hawkeyeIx(Buffer.concat([VIEW_MARGIN_FOR_ASSET, params]), trader, phoenix, perpAssetMap), 128);
  return { baseLots: b.readBigInt64LE(24), markTicks: b.readBigUInt64LE(40) };
}
```

Then, for each live (not retired) leg:

```ts
/** A token's dividend multiplier over 1e12 (Token-2022 scaled UI amount, extension type 25); 1e12 otherwise. */
function multiplier(mintData: Buffer, nowSecs: number): bigint {
  let at = 166;
  while (mintData.length >= at + 4) {
    const kind = mintData.readUInt16LE(at), len = mintData.readUInt16LE(at + 2);
    if (kind === 25) {
      const v = mintData.subarray(at + 4, at + 4 + len);
      const m = nowSecs >= Number(v.readBigInt64LE(40)) ? v.readDoubleLE(48) : v.readDoubleLE(32);
      return BigInt(Math.floor(m * 1e12));
    }
    at += 4 + len;
  }
  return 1_000_000_000_000n;
}

// USD (6 dp) per whole token = mark × tick size × 10^base_lot_decimals × multiplier / 1e12
const tokenPrice = (leg, markTicks: bigint, mult: bigint) =>
  (markTicks * BigInt(leg.tickSize.toString()) * 10n ** BigInt(leg.baseLotDecimals) * mult) / 1_000_000_000_000n;

// Asset held, smallest units:
//   spot leg → the amount in leg.asset_account (token account bytes 64..72)
//   SOL leg  → trader account lamports − rent-exempt minimum for its data length
const legValue = (units: bigint, price: bigint, decimals: number) => (units * price) / 10n ** BigInt(decimals);

// Short notional (6 dp) = |base lots| × mark ticks × tick size
const legNotional = (baseLots: bigint, markTicks: bigint, tickSize: bigint) =>
  (baseLots < 0n ? -baseLots : baseLots) * markTicks * tickSize;
```

```ts
nav    = max(0, idleUsdc + Σ legValue + phoenixEquity)   // idle = vault.usdc_account balance
supply = share mint supply                                 // mint data bytes 36..44
sharePrice = (nav + VIRTUAL) / (supply + VIRTUAL)          // what a deposit pays per share, in USD
positionValue(shares) = withdrawalPayout(nav, shares, supply)   // §7.2
```

A vault with 8 live legs needs 1 + 8 simulations and one `getMultipleAccountsInfo` call. Cache
the result for 15 to 30 seconds. For a list page, the share price from the latest event (§8.3) is
enough.

**Multiplier windows.** While a stock token's multiplier changes by more than 0.25% (a split or a
special dividend), from a day before to three days after, the program prices that token at:

- the **higher** of its two multipliers for deposits;
- the **lower** for withdrawals.

The code above uses the current multiplier, which suits display. Simulate (§4.1) for exact
amounts.

---

## 5. Building a transaction

Every instruction that values the vault (`deposit`, `withdraw`, `fulfill_withdraw`, and the
keeper's) takes the same **remaining accounts**, in this order:

1. Phoenix's global trader index accounts (writable), then its active trader buffer accounts
   (writable). The instruction's `gti` and `atb` arguments are how many of each.
2. **Every leg, in index order**, each followed by:
   - its asset account (writable), for spot legs;
   - its asset mint (read-only), for Token-2022 legs.

   A **retired** leg is passed alone. A missing or reordered leg fails with `InvalidLegAccounts`
   (6012).

```ts
import { AccountMeta, PublicKey } from "@solana/web3.js";
import { TOKEN_2022_PROGRAM_ID } from "@solana/spl-token";

function legMetas(legs): AccountMeta[] {
  const metas: AccountMeta[] = [];
  for (const leg of legs) {
    metas.push({ pubkey: leg.address, isSigner: false, isWritable: false });
    if (leg.retired) continue;
    if ("spot" in leg.kind) {
      metas.push({ pubkey: leg.assetAccount, isSigner: false, isWritable: true });
      if (leg.assetTokenProgram.equals(TOKEN_2022_PROGRAM_ID)) {
        metas.push({ pubkey: leg.assetMint, isSigner: false, isWritable: false });
      }
    }
  }
  return metas;
}

function remainingAccounts(phoenix: PhoenixKeys, legs): AccountMeta[] {
  const w = (pubkey: PublicKey) => ({ pubkey, isSigner: false, isWritable: true });
  return [...phoenix.globalTraderIndex.map(w), ...phoenix.activeTraderBuffer.map(w), ...legMetas(legs)];
}

/** The `core` accounts every valuing instruction names (Anchor nests them). */
function core(vaultKey: PublicKey, vault, config) {
  return {
    config: configAddress(), vault: vaultKey, authority: vault.authority, shareMint: vault.shareMint,
    usdcMint: config.usdcMint, vaultUsdc: vault.usdcAccount, vaultPhoenix: vault.phoenixAccount,
    escrowShares: vault.escrowShares, traderAccount: vault.traderAccount, perpAssetMap: config.perpAssetMap,
    phoenix: {
      phoenixProgram: PHOENIX_PROGRAM_ID, logAuthority: PHOENIX_LOG_AUTHORITY,
      globalConfig: PHOENIX_GLOBAL_CONFIG, hawkeyeProgram: HAWKEYE_PROGRAM_ID,
    },
  };
}
```

**Always send a v0 transaction with the vault's lookup table.**
- With 5 legs a deposit names about 35 accounts (about 336 bytes with the table). That doesn't fit
  Solana's 1,232 bytes as a legacy transaction.
- With 8 legs: about 41 accounts.

```ts
import { AddressLookupTableAccount, ComputeBudgetProgram, Connection, PublicKey,
  TransactionInstruction, TransactionMessage, VersionedTransaction } from "@solana/web3.js";

export async function buildTx(connection: Connection, payer: PublicKey, ixs: TransactionInstruction[],
  lookupTable: PublicKey, computeUnits: number, microLamports = 0): Promise<VersionedTransaction> {
  const table = (await connection.getAddressLookupTable(lookupTable)).value;
  if (!table) throw new Error("vault lookup table not found");
  const budget = [ComputeBudgetProgram.setComputeUnitLimit({ units: computeUnits })];
  if (microLamports > 0) budget.push(ComputeBudgetProgram.setComputeUnitPrice({ microLamports }));
  const { blockhash } = await connection.getLatestBlockhash();
  const message = new TransactionMessage({ payerKey: payer, recentBlockhash: blockhash,
    instructions: [...budget, ...ixs] }).compileToV0Message([table as AddressLookupTableAccount]);
  return new VersionedTransaction(message);
}
```

**Compute units** (measured with real Phoenix in tests; leave headroom):

| Instruction | 5 legs | 8 legs | Set the limit to |
|---|---|---|---|
| `deposit` | 163k | 215k | 400,000 |
| `withdraw` | 249k | 424k | 600,000 |
| `fulfill_withdraw` | ≈ withdraw | ≈ withdraw | 600,000 |
| `request_withdraw`, `cancel_withdraw` | < 50k | < 50k | 100,000 |

---

## 6. User flows

### 6.1 Deposit

`deposit(gti: u8, atb: u8, usdc_amount: u64, min_shares: u64)`

| Account | Notes |
|---|---|
| `user` (signer, writable) | Pays the share account's rent on the first deposit (≈0.002 SOL) |
| `core` | §5 |
| `user_usdc` (writable) | A USDC token account owned by the user (their ATA) |
| `user_shares` (writable) | The user's share ATA, `getAssociatedTokenAddressSync(vault.shareMint, user)`; the program creates it if missing |
| `token_program`, `associated_token_program`, `system_program` | Standard |

```ts
const ix = await program.methods
  .deposit(phoenix.globalTraderIndex.length, phoenix.activeTraderBuffer.length,
    new BN(usdcAmount.toString()), new BN(minShares.toString()))
  .accountsStrict({
    user,
    core: core(vaultKey, vault, config),
    userUsdc: getAssociatedTokenAddressSync(config.usdcMint, user),
    userShares: getAssociatedTokenAddressSync(vault.shareMint, user),
    tokenProgram: TOKEN_PROGRAM_ID,
    associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  })
  .remainingAccounts(remainingAccounts(phoenix, legs))
  .instruction();
const tx = await buildTx(connection, user, [ix], VAULT_LOOKUP_TABLE, 400_000);
```

**Shares minted** = `value × (supply + 1e6) / (nav + 1e6)`, rounded down, where
`value = usdc × (10,000 − entry_fee_bps) / 10,000`.

**Slippage.** Set `min_shares` to the simulated shares less a small margin (for example 0.3%): the
NAV moves between preview and execution.

**Refused when:**

| Condition | Error |
|---|---|
| `config.paused` | `Paused` (6001) |
| `!vault.deposits_open` | `DepositsClosed` (6002) |
| Amount over `max_deposit_usd` | `DepositTooLarge` (6022) |
| NAV + amount over `max_tvl_usd` | `MaxTvlReached` (6021) |
| The vault owes more than it holds | `VaultInsolvent` (6025) |
| Shares come in under `min_shares` | `SharesTooLow` (6030) |

Disable the button for the first four (§9).

### 6.2 Instant withdrawal

`withdraw(gti: u8, atb: u8, shares: u64, min_usdc_out: u64)`

| Account | Notes |
|---|---|
| `user` (signer) | |
| `core` | §5 |
| `user_shares` (writable) | The user's share ATA |
| `user_usdc` (writable) | A USDC token account owned by the user |
| `token_program` | |

```ts
const ix = await program.methods
  .withdraw(gti, atb, new BN(shares.toString()), new BN(minUsdcOut.toString()))
  .accountsStrict({ user, core: core(vaultKey, vault, config),
    userShares: getAssociatedTokenAddressSync(vault.shareMint, user),
    userUsdc: getAssociatedTokenAddressSync(config.usdcMint, user), tokenProgram: TOKEN_PROGRAM_ID })
  .remainingAccounts(remainingAccounts(phoenix, legs))
  .instruction();
const tx = await buildTx(connection, user, [ix], VAULT_LOOKUP_TABLE, 600_000);
```

Create the user's USDC ATA first, in the same transaction, if they don't have one
(`createAssociatedTokenAccountIdempotentInstruction`).

**Payout** = `withdrawalPayout(nav, shares, supply)` less `ceil(payout × exit_fee_bps / 10,000)`
(§7.2). It is paid only from the buffer **above what pending requests are worth**:

```
available = idle USDC − withdrawalPayout(nav, vault.pending_shares, supply)
```

If `payout > available`, the instruction fails with `NotEnoughLiquidity` (6034). Offer a request
instead (§6.3).

### 6.3 Request a withdrawal (when the buffer can't pay)

`request_withdraw(shares: u64)` takes no remaining accounts and does no valuation, so it is cheap.

| Account | Notes |
|---|---|
| `user` (signer, writable) | Pays the request's rent (≈0.0016 SOL, refunded when it closes) |
| `config`, `vault` (writable) | |
| `escrow_shares` (writable) | `vault.escrowShares` |
| `user_shares` (writable) | The user's share ATA |
| `request` (writable) | `requestAddress(vaultKey, vault.nextRequestId)` |
| `token_program`, `system_program` | |

```ts
const vault = await program.account.vault.fetch(vaultKey); // fresh: for next_request_id
const request = requestAddress(vaultKey, BigInt(vault.nextRequestId.toString()));
const ix = await program.methods.requestWithdraw(new BN(shares.toString()))
  .accountsStrict({ user, config: configAddress(), vault: vaultKey, escrowShares: vault.escrowShares,
    userShares: getAssociatedTokenAddressSync(vault.shareMint, user), request,
    tokenProgram: TOKEN_PROGRAM_ID, systemProgram: SystemProgram.programId })
  .instruction();
```

- **Race:** if another user's request lands first, `next_request_id` has moved and yours fails
  with a seeds constraint error (Anchor 2006). Re-read the vault and rebuild; retry
  automatically once or twice.
- **What happens next:** the keeper bot pays requests **oldest first**:
  - From the buffer at once if it can.
  - Otherwise it takes margin back from Phoenix and sells part of the lowest-paying legs, then
    pays.
  - Typical wait: one to a few keeper ticks (30 s each). The daily turnover limit can delay very
    large requests.
- **Price:** the request is paid at the NAV at payment time, not request time. The escrowed
  shares keep earning (or losing) with everyone's until then.
- **Split offer:** show "Withdraw X now, request the rest". Find the largest instant amount by
  bisecting on shares until `pay(shares) <= available`.

### 6.4 Track a request

| Request account | `returned` | Show |
|---|---|---|
| Exists | false | **Pending**: position in queue = number of open requests with a lower id |
| Exists | true | **Returned**: "We couldn't pay into your USDC account. Reclaim your shares (no fee)". Button → `cancel_withdraw` |
| Gone | n/a | **Done**: find the `Withdrawn` event (paid, has `request_id`) or `WithdrawCancelled` (cancelled) in the user's history |

A request is returned when the user's USDC ATA:

- was closed;
- was frozen;
- was given to another owner;

or when its payout rounds to zero (a dust request). Tell users to keep their USDC account open
while a request is pending.

### 6.5 Cancel a request

`cancel_withdraw()`: only the request's user may cancel.

| Account | Notes |
|---|---|
| `user` (signer, writable) | Must be `request.user` |
| `vault` (writable), `authority` | |
| `share_mint` (writable), `escrow_shares` (writable) | The exit fee's shares are burned from escrow |
| `user_shares` (writable) | The user's share ATA (created if they closed it, at their cost) |
| `request` (writable) | Closed; its rent goes back to the user |
| `token_program`, `associated_token_program`, `system_program` | |

```ts
const ix = await program.methods.cancelWithdraw()
  .accountsStrict({ user, vault: vaultKey, authority: vault.authority, shareMint: vault.shareMint,
    escrowShares: vault.escrowShares, userShares: getAssociatedTokenAddressSync(vault.shareMint, user),
    request: requestKey, tokenProgram: TOKEN_PROGRAM_ID,
    associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID, systemProgram: SystemProgram.programId })
  .instruction();
```

**Fee.** Cancelling costs `ceil(shares × exit_fee_bps / 10,000)` shares, the same as leaving: the
keeper may already have traded to raise the USDC. Show it before the user confirms. A **returned**
request comes back whole.

### 6.6 Pay your own request (optional)

`fulfill_withdraw(gti, atb)` can be called by anyone. The keeper does it normally. A user (or
anyone other than the keeper) may pay a request only when the buffer holds this request's payout
**and** what every other pending request is worth. While a stock token is in a multiplier window,
only the request's user or the keeper may pay it.

| Account | Notes |
|---|---|
| `cranker` (signer, writable) | Whoever pays the fee |
| `core` | §5 |
| `request` (writable), `user` (writable) | The request and its user (gets the rent back) |
| `user_usdc` (writable) | **The user's USDC ATA, which must exist and be theirs.** It is never created here |
| `token_program` | |

A "Claim now" button is useful only when `idle >= pay + value of the other pending requests`.
Otherwise leave it to the keeper.

---

## 7. Previews and quotes

### 7.1 Simulate and read the event

```ts
import { EventParser } from "@anchor-lang/core";

async function simulateEvents(connection: Connection, tx: VersionedTransaction) {
  const sim = await connection.simulateTransaction(tx, { sigVerify: false, replaceRecentBlockhash: true });
  if (sim.value.err) return { err: sim.value.err, logs: sim.value.logs ?? [], events: [] };
  const parser = new EventParser(program.programId, program.coder);
  return { err: null, logs: sim.value.logs ?? [], events: [...parser.parseLogs(sim.value.logs ?? [])] };
}

// Deposit preview:  events.find(e => e.name === "deposited").data.shares
// Withdraw preview: events.find(e => e.name === "withdrawn").data.usdcOut
```

The simulation needs the user to have the USDC or shares. When they don't (an amount typed
before connecting a wallet), use the math below. To turn the error in `sim.value.err` /
`sim.value.logs` into a message, use §11.

### 7.2 The math (BigInt, 6 dp)

These are the program's exact roundings:

```ts
const mulDiv = (a: bigint, b: bigint, d: bigint, up = false) => (up ? (a * b + d - 1n) / d : (a * b) / d);

/** Shares a deposit of `usdc` mints. */
export function depositShares(usdc: bigint, nav: bigint, supply: bigint, entryFeeBps: bigint): bigint {
  const value = mulDiv(usdc, BPS - entryFeeBps, BPS);
  return mulDiv(value, supply + VIRTUAL, nav + VIRTUAL);
}

/** What `shares` are worth to their holder before the exit fee. */
export function withdrawalPayout(nav: bigint, shares: bigint, supply: bigint): bigint {
  let retained = mulDiv(nav + VIRTUAL, supply - shares + VIRTUAL, supply + VIRTUAL, true) - VIRTUAL;
  if (retained < 0n) retained = 0n;
  return nav > retained ? nav - retained : 0n;
}

/** USDC a withdrawal or fulfilled request pays. */
export const withdrawalPay = (nav: bigint, shares: bigint, supply: bigint, exitFeeBps: bigint) => {
  const gross = withdrawalPayout(nav, shares, supply);
  return gross - mulDiv(gross, exitFeeBps, BPS, true);
};

/** USDC an instant withdrawal can use: the buffer above what pending requests are worth. */
export const instantAvailable = (idle: bigint, nav: bigint, pendingShares: bigint, supply: bigint) => {
  const pending = pendingShares > 0n ? withdrawalPayout(nav, pendingShares, supply) : 0n;
  return idle > pending ? idle - pending : 0n;
};

/** Shares burned when a (non-returned) request is cancelled. */
export const cancelFeeShares = (shares: bigint, exitFeeBps: bigint) => mulDiv(shares, exitFeeBps, BPS, true);
```

---

## 8. Yield, composition and history

### 8.1 Composition

Show each live leg and the idle USDC as shares of the NAV:

- **Leg:** symbol, asset value, short notional, and percent of NAV.
- **Buffer:** idle USDC.
- **Phoenix margin:** Phoenix equity.

Get the symbol from Phoenix's markets list, matching by `assetId`:

```
GET https://perp-api.phoenix.trade/v1/view/exchange/markets
→ [{ "symbol": "SOL", "assetId": 0, ... }, ...]
```

### 8.2 Estimated APR

Funding is paid hourly, from longs to shorts when positive.

```
GET https://perp-api.phoenix.trade/v1/funding/{symbol}/rates?startTime={ms}&limit=1000
→ { "rates": [{ "timestamp": 1791961200, "fundingRatePercentage": "0.001585" }, ...] }
```

Each rate is a percent per hour, so a market's funding APR is the mean of its rates times 8,760.
Use 7-day and 30-day windows; the keeper ranks markets by their mean.

```
vault funding APR ≈ Σ legs (short notional × funding APR of its market) / NAV
```

Label it an estimate: funding changes every hour, and trades cost a little. Realized yield comes
from the share price.

### 8.3 Realized yield and history

Every user and keeper instruction emits an event carrying `nav_usd` and `supply` (§10). So a
history of the share price, `(nav_usd + 1e6) / (supply + 1e6)`, comes straight from the program's
logs:

```ts
const sigs = await connection.getSignaturesForAddress(vaultKey, { limit: 1000 });
// for each signature: getTransaction(sig, { maxSupportedTransactionVersion: 0 }),
// parse meta.logMessages with EventParser, keep events with a navUsd field.
```

`realized APY = (price_now / price_then)^(365 / days) − 1`, over 7 or 30 days.

**A user's history:** the `Deposited`, `Withdrawn`, `WithdrawRequested`, `WithdrawCancelled` and
`WithdrawReturned` events whose `user` is them. An indexer, or a Helius webhook on the vault
address, saves re-reading logs.

---

## 9. Vault states to show

| Condition | Read from | UI |
|---|---|---|
| `config.paused` | Config | Deposits disabled: "Paused". Withdrawals still work |
| `!vault.deposits_open` | Vault | Deposits disabled. The keeper closes deposits when no active market with room pays at least 3% on capital ("Vault full / yield too low right now"); it reopens them by itself |
| `nav ≥ max_tvl_usd` | NAV + Vault | "Vault full". Cap the amount input at `max_tvl_usd − nav` and `max_deposit_usd` |
| Buffer can't pay | `instantAvailable` | Offer instant + request split (§6.3) |
| Request returned | WithdrawRequest | "Reclaim your shares" (§6.4) |
| A stock leg in a multiplier window | Mint scaled-UI extension, `starts` within −1 day / +3 days | Info: "TSLAx is adjusting for a corporate action; withdrawals price it conservatively" |
| Equity < 0 | NAV calculation | Warning: deposits are refused (`VaultInsolvent`) |

---

## 10. Events

Anchor events, base64 in `Program data:` log lines; parse them with `EventParser`. USD amounts and
shares use 6 decimals.

| Event | Fields | Emitted by |
|---|---|---|
| `Deposited` | vault, user, usdc_amount, shares, value_usd, nav_before_usd, nav_usd, supply | `deposit` |
| `Withdrawn` | vault, user, shares, payout_usd, fee_usd, usdc_out, request_id (`Option<u64>`), nav_usd, supply | `withdraw`, `fulfill_withdraw` |
| `WithdrawRequested` | vault, user, request, request_id, shares | `request_withdraw` |
| `WithdrawCancelled` | vault, user, request_id, shares (returned), fee_shares | `cancel_withdraw` |
| `WithdrawReturned` | vault, user, request_id, shares, by | `return_withdraw` (keeper) |
| `DepositsSet` | vault, open, by | `set_deposits_open` |
| `Deployed` | vault, leg_index, usdc_spent, asset_bought, position_lots, nav_before_usd, nav_usd, supply | keeper |
| `Unwound` | vault, leg_index, asset_sold, usdc_received, position_lots, nav_before_usd, nav_usd, supply | keeper |
| `MarginMoved` | vault, to_phoenix, amount, nav_usd, supply | keeper |
| `Rehedged` | vault, leg_index, position_lots, nav_before_usd, nav_usd, supply | keeper |
| `Swapped` | vault, leg_index, sold_usdc, spent, received | keeper (inside deploy/unwind) |
| `VaultCreated`, `LegAdded`, `LegRetired` | see IDL | admin |

---

## 11. Errors

Custom errors are `6000 + n`. Anchor's own constraint errors are 2000–2999. For example, 2006
`ConstraintSeeds` means a request id race (§6.3), and 3012 `AccountNotInitialized` means a missing
token account.

**Errors a user can hit:**

| Code | Name | Show the user |
|---|---|---|
| 6000 | Unauthorized | "Only the request's owner can do this" |
| 6001 | Paused | "The vault is paused: deposits are off, withdrawals still work" |
| 6002 | DepositsClosed | "Deposits are closed right now" |
| 6003 | ZeroAmount | "Enter an amount" |
| 6012 | InvalidLegAccounts | App bug: reload the vault's legs (one was added or retired) |
| 6021 | MaxTvlReached | "The vault is full" |
| 6022 | DepositTooLarge | "Max per deposit is $X" |
| 6025 | VaultInsolvent | "Deposits are paused while the vault recovers" |
| 6026 | MultiplierChanging | "A stock token is adjusting; only you or the keeper can pay this request right now" |
| 6030 | SharesTooLow | "The price moved: review and try again" (raise slippage) |
| 6031 | PayoutTooLow | Same as 6030, for withdrawals |
| 6032 / 6033 | NothingDeposited / NothingToWithdraw | "Amount too small" |
| 6034 | NotEnoughLiquidity | "Not enough instant liquidity: request the withdrawal" (§6.3) |
| 6035 | NotEnoughShares | "You don't have that many shares" |
| 6044 | RequestReturned | "This request was returned: reclaim your shares" |

**Keeper and admin errors** (shown in an admin or ops UI only):

- 6004 MathOverflow
- 6005 DivisionByZero
- 6006 InvalidMint
- 6007 InvalidTokenAccount
- 6008 InvalidPhoenixAccount
- 6009 MissingPhoenixAccounts
- 6010 PhoenixInstruction
- 6011 InvalidPrice
- 6013 TooManyLegs
- 6014 DuplicateLeg
- 6015 LegInactive
- 6016 LegRetired
- 6017 LegNotFlat
- 6018 MaxPositionReached
- 6019 OpenInterestShareReached
- 6020 LegShareReached
- 6023 MarketAfterHours
- 6024 UnsupportedMint
- 6027 MarginTooThin
- 6028 BufferTooThin
- 6029 TurnoverLimitReached
- 6036 NotEnoughAsset
- 6037 WrongLegKind
- 6038 InvalidSwapAccounts
- 6039 UnsupportedRoute
- 6040 SwapAmountOff
- 6041 SwapPriceTooLow
- 6042 SwapMovedVaultFunds
- 6043 InvalidParameter
- 6045 RetireActiveLeg

Messages for all of them are in the IDL's `errors`.

---

## 12. Checklist

**Reading:**
- [ ] Read `Config`, `Vault` and every `Leg` (0..leg_count). Re-read the legs when a `LegAdded` or `LegRetired` event appears, or on any 6012.
- [ ] Fetch Phoenix's index accounts from its API, cached for 10 minutes, with a fallback to the last known.

**Sending:**
- [ ] Use v0 transactions with the vault's lookup table, and compute limits from §5.
- [ ] Simulate before signing; show shares or USDC from the event; set `min_shares` / `min_usdc_out` from it.
- [ ] Create the user's USDC ATA if missing (withdraw); the program creates the share ATA on deposit.

**Withdrawals:**
- [ ] Decide instant vs request with `instantAvailable`; offer a split.
- [ ] Requests: rebuild on id races; show pending, returned or done; show the cancel fee; warn users to keep their USDC account open.

**Display:**
- [ ] States from §9; errors from §11.
- [ ] Use BigInt for every on-chain amount (6 dp); never JavaScript floats for money.
