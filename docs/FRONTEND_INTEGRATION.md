# Frontend integration: Vanna Phoenix basis vaults

How a web app reads the vaults and lets users deposit and withdraw. Everything here goes
through the TypeScript client in [`client/`](../client), whose functions this guide documents.
The client is checked byte for byte against the program's own Rust types (`npm test`).

- Program: `vanna_phoenix_vault`, `GtuQoYxzytGKtLqQGkYrVbsaPHTZN5jboP25DrSmKvGr` (Solana mainnet).
- IDL: [`client/src/idl/vanna_phoenix_vault.json`](../client/src/idl/vanna_phoenix_vault.json),
  with TypeScript types in `vanna_phoenix_vault.ts` next to it.
- Design and math: [README.md](../README.md). Security review: [AUDIT.md](../AUDIT.md).

## Contents

1. [What the frontend does](#1-what-the-frontend-does)
2. [Setup](#2-setup)
3. [Units and conventions](#3-units-and-conventions)
4. [Reading a vault](#4-reading-a-vault)
5. [Deposit](#5-deposit)
6. [Withdraw](#6-withdraw)
7. [Sending transactions](#7-sending-transactions)
8. [Events and history](#8-events-and-history)
9. [Errors](#9-errors)
10. [Asset notes](#10-asset-notes)
11. [Low-level reference](#11-low-level-reference)
12. [One-time setup by Vanna (ops)](#12-one-time-setup-by-vanna-ops)
13. [Testing the integration](#13-testing-the-integration)

## 1. What the frontend does

There is one vault per asset: SOL, ETH, BTC, TSLA and META. A user deposits and receives the
vault's **shares**, an SPL token with 6 decimals. The vault holds the asset and an equal short on
Phoenix perps, so the share price stays flat in dollars and grows with funding.

| User action | Client function | What happens on-chain, in one transaction |
|---|---|---|
| See a vault | `fetchVault`, `getVaultState` | Nothing; reads accounts and simulates Phoenix's views |
| See their position | `getUserPosition` | Nothing; reads their share balance |
| Deposit | `prepareDeposit` → wallet signs | Funds in; swap through Jupiter if needed; USDC margin to Phoenix; short opened; shares minted |
| Withdraw | `prepareWithdraw` → wallet signs | Their part of the short closed; shares burned; asset and/or USDC paid out |

Deposits and withdrawals settle at once: there are no requests, queues or lockups. Each user
pays their own trading costs.

## 2. Setup

```bash
cd client && npm install && npm run build      # or depend on it from your app's workspace
```

Dependencies: `@anchor-lang/core` 1.2 (Anchor's TypeScript client), `@solana/web3.js` 1.x,
`@solana/spl-token` 0.4.

Frontend config. Vanna provides these values once the vaults exist (section 12):

```ts
import { PublicKey } from "@solana/web3.js";
import { ASSETS } from "@vanna/phoenix-vault-client";

export const VANNA = {
  rpc: "https://<your-rpc>",                       // a paid RPC: simulations are frequent
  vaults: {
    SOL:  { mint: ASSETS.SOL.mint,  lookupTable: new PublicKey("<from ops>") },
    ETH:  { mint: ASSETS.ETH.mint,  lookupTable: new PublicKey("<from ops>") },
    BTC:  { mint: ASSETS.BTC.mint,  lookupTable: new PublicKey("<from ops>") },
    TSLA: { mint: ASSETS.TSLA.mint, lookupTable: new PublicKey("<from ops>") },
    META: { mint: ASSETS.META.mint, lookupTable: new PublicKey("<from ops>") },
  },
  // Any funded system account, used as the fee payer of read-only simulations when no wallet
  // is connected. It signs nothing and pays nothing.
  viewPayer: new PublicKey("<any funded address>"),
  jupiter: { apiBase: "https://api.jup.ag/swap/v1", apiKey: "<key>" },  // lite-api.jup.ag without a key
};
```

The basic objects:

```ts
import { Connection } from "@solana/web3.js";
import { vaultProgram, fetchVault, fetchPhoenixKeys, FlowContext } from "@vanna/phoenix-vault-client";

const connection = new Connection(VANNA.rpc, "confirmed");
const program = vaultProgram(connection);          // no wallet needed: the wallet signs later
const phoenix = await fetchPhoenixKeys();          // Phoenix's index accounts; cache for minutes

const vault = await fetchVault(program, VANNA.vaults.ETH.mint);
const ctx: FlowContext = {
  connection, program, vault, phoenix,
  vaultLookupTables: [VANNA.vaults.ETH.lookupTable],
  jupiter: VANNA.jupiter,
  tx: { microLamportsPerCu: 50_000 },              // priority fee; see section 7
};
```

`fetchPhoenixKeys()` reads Phoenix's current trader index accounts from its API. They grow as
Phoenix grows, so do not hard-code them; `PHOENIX_MAINNET` is only a fallback.

## 3. Units and conventions

- **All amounts are `bigint` in the token's smallest unit.** The client never takes floats.

  | Token | Decimals | 1 token is |
  |---|---|---|
  | SOL | 9 | `1_000_000_000n` lamports |
  | USDC | 6 | `1_000_000n` |
  | ETH (Wormhole ETH `7vfCX…`), BTC (cbBTC `cbbtcf…`) | 8 | `100_000_000n` |
  | TSLAx, METAx (Token-2022) | 8 | `100_000_000n` |
  | Vault shares | 6 | `1_000_000n` |

- **USD values have 6 decimals:** `nav`, `valueUsd`, `payoutUsd`, `sharePrice` and caps.
  `1_000_000n` is $1.
- **Share price** is quoted per whole share in USD with 6 decimals. It starts at $1.
- **Prices** come from Phoenix's mark price. `tokenPrice` is per whole token, including a stock
  token's dividend multiplier.
- Every write is a **v0 transaction with address lookup tables** (section 7).

Formatting helper:

```ts
const fmt = (v: bigint, decimals: number, dp = 2) =>
  (Number(v) / 10 ** decimals).toLocaleString(undefined, { maximumFractionDigits: dp });
```

## 4. Reading a vault

### `fetchVault(program, assetMint): Promise<VaultInfo>`

Reads the vault account and the protocol config. Every address the vault's instructions need
is in it, so nothing else has to be derived. Fields the UI uses:

| Field | Meaning |
|---|---|
| `kind` | `"nativeSol"` (SOL vault) or `"spot"` (ETH, BTC, TSLA, META) |
| `assetMint`, `assetDecimals`, `assetTokenProgram` | The asset token. Token-2022 for TSLA and META |
| `shareMint` | The vault's share token |
| `depositsOpen`, `paused` | Deposits allowed. `paused` is protocol-wide; withdrawals are never paused |
| `afterHoursDeposits` | Whether deposits stay open while a stock market is after hours (`false` for stocks) |
| `maxDepositUsd` | The most value one deposit may add (USD, 6 dp) |
| `maxPositionUsd`, `maxOiShareBps`, `maxAssetUnits` | The vault's caps (the program enforces them) |
| `targetMarginBps` | USDC per dollar of asset in the vault's mix: `10000` for spot vaults (1:1), `0` for SOL |
| `slippageBps` | The vault's price band for its own trades (100 = 1%) |

### `getVaultState(connection, vault, payer, phoenix?): Promise<VaultState>`

Values the vault exactly as the program does. It simulates Phoenix's Hawkeye views for the
vault's margin and position and reads its balances. `payer` is the connected wallet, or
`VANNA.viewPayer`; it only pays for the simulation, which costs nothing.

```ts
const state = await getVaultState(connection, vault, wallet.publicKey ?? VANNA.viewPayer, phoenix);
`TVL $${fmt(state.nav, 6)} · share $${fmt(state.sharePrice, 6, 6)} · mark $${fmt(state.markPrice, 6)}`
```

| Field | Meaning |
|---|---|
| `nav` | The vault's value: asset × mark + Phoenix equity (USD, 6 dp) |
| `supply` | Shares outstanding |
| `sharePrice` | USD per whole share, at the price withdrawals pay |
| `markPrice`, `tokenPrice` | Phoenix mark, and per token after the stock multiplier |
| `assetUnits`, `positionLots` | Asset held and the short (negative base lots) |
| `shortNotional`, `marginBps` | The short's size, and Phoenix effective collateral per dollar of it |
| `multiplier` | Shares per stock token over `1e12` (METAx ≈ 1.00285e12) |
| `margin` | Phoenix's raw margin view (collateral, uPnL, funding, withdrawable, liquidatable) |

The APY is not on-chain. Compute it from `sharePrice` sampled over time, or from Phoenix's
funding history (`fetchPhoenixMarket`, or `GET /v1/funding/{symbol}/rates` on Phoenix's API).

### `getUserPosition(connection, vault, state, user)`

Returns `{ shares, valueUsd }`: the user's share balance and what it is worth at the
withdrawal price, before the costs of leaving. For exact numbers, preview the withdrawal
(section 6).

### `depositStatus(ctx, payer)`

Returns whether the deposit button should be enabled:
`{ open, reasons[], positionRoomUsd, maxDepositUsd, state }`.

| Check | Where | When it applies |
|---|---|---|
| Protocol paused, deposits closed | `vault.paused`, `vault.depositsOpen` | Any time. The keeper closes deposits when funding is poor |
| Stock market after hours | `fetchPhoenixMarket("TSLA")?.commodityMetadata?.status`, or simulate (`MarketAfterHours`) | TSLA and META on weekends and holidays (Sun 8pm to Fri 8pm New York is open) |
| One deposit too large | `valueUsd ≤ vault.maxDepositUsd` | Large users: split into several deposits |
| Vault full | `positionRoomUsd` | Near the cap |
| Open-interest share | Simulate (`OpenInterestShareReached`) | Phoenix's market is small |

The simulation in `prepareDeposit` catches every one of these with its exact program error, so
the UI can rely on it and use the fields above only to explain why.

## 5. Deposit

### What users can bring

| User brings | How the vault takes it | Client call |
|---|---|---|
| USDC only | Swaps part to the asset through Jupiter: half for spot vaults (1x mix), all of it for SOL | `prepareDeposit(ctx, user, 0n, usdc)` |
| The asset only | Spot vaults swap half to USDC margin; SOL needs no swap | `prepareDeposit(ctx, user, asset, 0n)` |
| Both, at the vault's mix | Taken as they come, no swap | `prepareDeposit(ctx, user, asset, mixUsdc(...))` |

A deposit of both must be within 5% of the vault's mix, or the program refuses it
(`DepositOffMix`): off the mix, the vault would trade its hedged asset at Phoenix's mark instead
of the spot price. Use `mixUsdc` to fill in the second amount:

```ts
const usdc = mixUsdc(assetAmount, state.tokenPrice, vault.assetDecimals, vault.targetMarginBps);
```

### `prepareDeposit(ctx, user, assetAmount, usdcAmount, slippageBps = 50)`

It does the following:
1. For a single asset, plans the swap amount (`planDepositSwap`) and fetches a Jupiter route
   built for the vault authority (`jupiterRoute`).
2. Builds the deposit and simulates it. This gives the exact shares, or the program's error.
3. Rebuilds with `minShares = shares × (1 − slippageBps)` and returns a transaction for the
   wallet.

It returns `{ transaction, shares, valueUsd, preview, route }`, or throws `VaultActionError`,
whose `preview.error` holds the program error (section 9).

```ts
import { prepareDeposit, VaultActionError } from "@vanna/phoenix-vault-client";

try {
  const { transaction, shares, valueUsd } = await prepareDeposit(ctx, wallet.publicKey, 0n, 1_000n * 1_000_000n);
  // Show: "You deposit $1,000.00 → about {fmt(shares,6)} shares worth ${fmt(valueUsd,6)}"
  const signature = await wallet.sendTransaction(transaction, connection);
  await connection.confirmTransaction(signature, "confirmed");
} catch (e) {
  if (e instanceof VaultActionError) showError(e.preview.error);   // see section 9
  else throw e;
}
```

`valueUsd` is what the deposit added to the vault after its own swap and trade, so
`amount − valueUsd` is the user's entry cost (spread, Phoenix fee, Jupiter price impact). Show
it.

The program creates the user's share token account if needed; the user pays its rent
(about 0.002 SOL) once. A SOL deposit takes lamports straight from the wallet, so leave about
0.01 SOL for fees and rent.

## 6. Withdraw

### Payout options (`receive`)

| `receive` | User gets | Swap |
|---|---|---|
| `"both"` | Their part of the asset plus USDC. Spot vaults pay about half and half; the SOL vault pays SOL first, then USDC | None: cheapest |
| `"usdc"` | Everything in USDC | The asset part is sold through Jupiter |
| `"asset"` | Everything in the asset | The USDC part is bought through Jupiter |

### `prepareWithdraw(ctx, user, shares, receive, slippageBps = 50)`

It does the following:
1. Simulates a `"both"` withdrawal to learn exactly what the vault will pay: `assetOut`,
   `usdcOut`.
2. For `"usdc"` or `"asset"`, fetches a Jupiter route for slightly less than the amount to swap
   (99.95% for a spot vault's asset, 99.6% otherwise). The vault decides the amount inside the
   transaction, and the route may not spend more. Whatever the route leaves unsold is paid as
   it is, usually a few cents of the other token.
3. Simulates again, sets `minAssetOut` and `minUsdcOut` from the result less `slippageBps`, and
   returns the transaction.
4. Prepends instructions that create the user's USDC and asset token accounts if they are
   missing. Each costs the user about 0.002 SOL of rent the first time.

It returns `{ transaction, assetOut, usdcOut, payoutUsd, sharesBurned, preview, route }`.

```ts
const shares = (await getUserPosition(connection, vault, state, wallet.publicKey)).shares;
const w = await prepareWithdraw(ctx, wallet.publicKey, shares, "usdc");
// Show: "You receive ${fmt(w.usdcOut, 6)}" (+ fmt(w.assetOut, vault.assetDecimals) of the asset if any)
if (w.sharesBurned < shares) {
  // Phoenix let only part of the payout leave now (the SOL vault with USDC owed, or Phoenix's
  // withdraw limits): the user keeps the rest of their shares and can withdraw them later.
}
await wallet.sendTransaction(w.transaction, connection);
```

Withdrawals are always open: during pauses, closed deposits, stock after-hours and over caps.

**Large withdrawals.** Each withdrawal closes its share of the short on Phoenix within 1% of the
mark, all at once, or it fails. If the book is too thin (a large amount, or a stock on a
weekend), split it into several withdrawals, or use `"both"` to skip the swap. The preview
fails with Phoenix's error when the size cannot fill.

## 7. Sending transactions

- **Lookup tables are required.** A deposit or withdrawal names 31–51 accounts, which does not
  fit Solana's 1,232-byte limit without lookup tables. Pass the vault's table (from config) plus
  the route's Jupiter tables. `prepareDeposit` and `prepareWithdraw` already do this, and so does
  `buildTransaction(connection, payer, ixs, tables, options)`.

  | Operation | Accounts | With lookup tables | Compute used |
  |---|---|---|---|
  | SOL deposit | 31 | 343 bytes | 241k |
  | SOL withdraw | 32 | 345 bytes | 246k |
  | ETH deposit (ETH + USDC) | 34 | 346 bytes | 246k |
  | ETH withdraw | 34 | 347 bytes | 272k |
  | ETH deposit, USDC only (Jupiter) | 47 | 426 bytes | 319k |
  | TSLA deposit, USDC only (Jupiter) | 51 | 436 bytes | 331k |

- **Compute budget:** the client sets 400,000 units without a swap and 600,000 with one. Add a
  priority fee with `tx.microLamportsPerCu`.
- **Freshness:** a Jupiter route's pools move quickly. Prepare the transaction when the user
  clicks confirm, not when the page loads, and rebuild it if the blockhash expires (about 60
  seconds). A stale route fails with `SwapAmountOff` or `SwapPriceTooLow`; rebuild and retry.
- **Wallets:** use your wallet adapter's `sendTransaction(tx, connection)`, or `signTransaction`
  followed by `connection.sendRawTransaction`. The transactions are v0, which all major Solana
  wallets support.
- **Only the user signs.** The vault signs for itself inside the program.

## 8. Events and history

The program emits Anchor events in its transaction logs. `parseEvents(program, logs)` decodes
them, with field names in camelCase:

| Event | Fields | When |
|---|---|---|
| `deposited` | `basisVault, user, assetAmount, usdcAmount, shares, valueUsd, navBeforeUsd, positionLots` | Every deposit |
| `withdrawn` | `basisVault, user, shares` (burned), `payoutUsd, assetOut, usdcOut, positionLots` | Every withdrawal. `assetOut` and `usdcOut` are what the user actually received |
| `swapped` | `basisVault, soldUsdc, spent, received` | Each Jupiter swap inside a deposit or withdrawal |
| `rebalanced`, `depositsSet` | (keeper) | Funding reinvested, or deposits opened or closed |

A user's history: `connection.getSignaturesForAddress(userShareAccount(user, vault.shareMint))`,
then `getTransaction` and `parseEvents`. For a vault-wide feed, use the basis vault address.
Index these with your backend or a webhook service rather than in the browser.

## 9. Errors

`VaultActionError.preview.error` (or `vaultErrorFromLogs(logs)` on a failed transaction) is
`{ code, name, message }`. If it is `null`, the failure came from another program: Phoenix
(its order could not fill within the band), Jupiter, or the token program (balance too low).

| Code | Name | Show the user | Frontend action |
|---|---|---|---|
| 6001 | Paused | "Vault paused for maintenance" | Disable deposits; withdrawals still work |
| 6002 | DepositsClosed | "Deposits are closed for now" | Disable deposits |
| 6003 | ZeroAmount | "Enter an amount" | Validate input |
| 6012 | MaxPositionReached | "The vault is full" | Show remaining room; suggest a smaller amount |
| 6013 | OpenInterestShareReached | "The vault is at its share of the market" | Smaller amount, or later |
| 6014 | DepositTooLarge | "Max ${maxDepositUsd} per deposit" | Split the deposit |
| 6015 | MarketAfterHours | "The TSLA/META market is closed; deposits reopen Sunday 8pm ET" | Disable deposits for stocks |
| 6017 | VaultInsolvent | "Deposits unavailable" | Disable deposits; alert ops |
| 6019 | MultiplierChanging | "Deposits pause around a stock dividend or split" | Disable deposits for a few days |
| 6020 | DepositOffMix | "Use a single asset, or the suggested split" | Use `mixUsdc`, or one asset |
| 6021 | MarginTooThin | "The vault cannot take this deposit right now" | Retry later; alert ops |
| 6022 | SharesTooLow | "Price moved; try again" | Rebuild with a fresh preview |
| 6023 | PayoutTooLow | "Price moved; try again" | Rebuild with a fresh preview |
| 6024 | NothingDeposited | "Amount too small" | Minimum-amount check |
| 6025 | NothingToWithdraw | "Phoenix lets nothing leave right now; try later" | SOL vault owes USDC; keeper settles |
| 6026 | NotEnoughShares | "Not enough shares" | Validate against the balance |
| 6027 | PriceOutOfBand | "Price moved; try again" | Retry |
| 6029 | InvalidSwapAccounts | "Swap failed" | Rebuild the route |
| 6030 | SwapNotNeeded | — | Client bug: a route was sent for a deposit at the mix |
| 6031 | SwapAmountOff | "Price moved; try again" | Rebuild with a fresh route |
| 6032 | SwapPriceTooLow | "Swap price too far from the market; try again or use another payout" | Retry, or `"both"` |
| 6033 | SwapMovedVaultFunds | "Swap failed" | Report: bad route |
| others | InvalidMint, InvalidTokenAccount, InvalidPhoenixAccount, MissingPhoenixAccounts, WrongVaultKind, … | "Something went wrong" | Client or config bug: refresh `fetchPhoenixKeys` and the vault |

Codes 6000, 6004, 6005, 6010, 6011, 6016, 6018 and 6034 come from admin or keeper actions or
internal checks, which users do not trigger.

## 10. Asset notes

| Vault | Asset | Notes |
|---|---|---|
| SOL | Native SOL (lamports) | The SOL is the short's margin, so a SOL-only deposit needs no swap and USDC-only swaps all of it to SOL. Capped at 450 SOL by Phoenix's 500-SOL limit per trader. Withdrawals pay SOL first. If the vault owes USDC after a rally, a payout can be partial (`sharesBurned < shares`) until the keeper settles it |
| ETH | Wormhole ETH `7vfCXTUXx5WJV5JADk17DUJ4ksgau7utNKj4b963voxs` | Not native ETH: users bridge or buy it on Solana. USDC-only deposits avoid this |
| BTC | cbBTC `cbbtcf3aa214zXHbiAZQwf4122FBYbraNdFqgw4iMij` | As ETH |
| TSLA, META | xStocks `XsDoVfqe…`, `Xsa62P5m…` (Token-2022) | Use the Token-2022 program for token accounts (the client does). One METAx is about 1.00285 shares (reinvested dividends); show the token, value it with `tokenPrice`. Deposits are closed after hours (weekends, US holidays) and around large dividends or splits. **xStocks are not offered to US persons; geofence accordingly.** The issuer can pause transfers, which also pauses withdrawals |

## 11. Low-level reference

Use these functions instead of the flows if you need full control.

```ts
// Instructions
depositInstruction(program, { vault, user, assetAmount, usdcAmount, minShares, route?, phoenix? }): Promise<TransactionInstruction>
withdrawInstruction(program, { vault, user, shares, minAssetOut, minUsdcOut, receive, route?, phoenix? }): Promise<TransactionInstruction>
withdrawSetupInstructions(vault, user): TransactionInstruction[]       // create the payout token accounts

// Swaps
planDepositSwap(price, decimals, targetMarginBps, assetAmount, usdcAmount): { sell: "usdc" | "asset"; amount } | null
jupiterRoute(vault, sell, amount, { apiBase?, apiKey?, slippageBps?, maxAccounts?, dexes? }): Promise<SwapRoute>

// Transactions
buildTransaction(connection, payer, instructions, lookupTables, { computeUnits?, microLamportsPerCu? }): Promise<VersionedTransaction>
simulate(connection, program, tx): Promise<{ ok, logs, events, error, unitsConsumed }>
parseEvents(program, logs); vaultErrorFromLogs(logs)

// Math (the program's own arithmetic, in bigint)
tokenPrice(markTicks, tickSize, baseLotDecimals, multiplier?); usdValue; tokensForUsd
mixUsdc(assetAmount, price, decimals, targetMarginBps); onMix(...)
sharesForValue(value, nav, supply); shareValue(shares, nav, supply); lessBps(amount, bps)

// Addresses
configAddress(); basisVaultAddress(assetMint); shareMintAddress(basisVault)
authorityAddress(basisVault, nonce); traderAccountAddress(authority); userShareAccount(user, shareMint)
```

### The raw instructions

If you build instructions without the client (for example from another language), use the
IDL. The account lists below are exactly what `program.methods.deposit(...)` encodes.

**`deposit(gti: u8, atb: u8, asset_amount: u64, usdc_amount: u64, min_shares: u64, route: Option<SwapRoute>)`**

| # | Account | Notes |
|---|---|---|
| 1 | `user` | Signer, writable |
| 2–25 | `core` (24 accounts): `config, basis_vault, authority, share_mint, usdc_mint, phoenix_mint, vault_usdc, vault_phoenix, vault_asset?, asset_mint, asset_token_program, trader_account, [phoenix_program, log_authority, global_config, hawkeye_program, ember_program, ember_state, ember_vault], perp_asset_map, orderbook, spline_collection, withdraw_queue, global_vault` | All from `VaultInfo` and the constants. `vault_asset` is empty (the program id) for SOL |
| 26 | `user_shares` | The user's share ATA (SPL Token); created by the program if missing |
| 27 | `user_asset?` | The user's asset ATA under the asset's token program, when `asset_amount > 0` (spot vaults) |
| 28 | `user_usdc?` | The user's USDC ATA, when `usdc_amount > 0` |
| 29 | `swap_program?` | Jupiter v6, with a route |
| 30 | `vault_wsol?` | The authority's wrapped-SOL ATA, for SOL-vault routes |
| 31–33 | `token_program, associated_token_program, system_program` | |
| remaining | Phoenix's global trader index accounts (`gti` of them) and active trader buffer accounts (`atb`), writable; then the route's accounts (`route.accounts` of them) as Jupiter returns them | |

An absent optional account (`?`) is passed as the program's own id, as Anchor does.

`SwapRoute { accounts: u8, data: Vec<u8> }`: Jupiter's `swapInstruction.data`, and the number
of its accounts appended to `remainingAccounts`, with the vault authority's signer flag
cleared.

**`withdraw(gti, atb, shares: u64, min_asset_out: u64, min_usdc_out: u64, receive: Receive, route: Option<SwapRoute>)`**:
the same accounts as `deposit`; `user_usdc` is always passed. `Receive` is
`{ both: {} } | { asset: {} } | { usdc: {} }`.

**Keeper and admin instructions** (not for the user frontend): `rebalance`,
`set_deposits_open`, `update_vault`, `create_vault`, `set_config`, `initialize`. See the README.

## 12. One-time setup by Vanna (ops)

1. **Create the vaults:** the admin runs `create_vault` with a random `authority_nonce`, and
   Phoenix onboards each vault's trader.
2. **Create one address lookup table per vault:**

   ```ts
   const { table, transactions } = await createVaultLookupTable(connection, opsWallet.publicKey, vault, phoenix);
   for (const ixs of transactions) await sendAndConfirm(ixs);   // create + extend, then extend
   // Publish `table` as VANNA.vaults.<ASSET>.lookupTable
   ```

   If Phoenix adds trader index accounts later, extend the tables with the new ones
   (`vaultTableAddresses` lists what belongs in them).
3. **Publish the frontend config** (section 2): table addresses, the view payer, and the Jupiter
   API key.

## 13. Testing the integration

- `cd client && npm test` checks the client's instructions, PDAs, Hawkeye views, events and
  account decoding against bytes the program's Rust types produced
  (`programs/vanna_phoenix_vault/tests/client_vectors.rs`). After any program change, rebuild
  the IDL and rerun:

  ```bash
  ~/.avm/bin/anchor-1.2.0 idl build -p vanna_phoenix_vault -o target/idl/vanna_phoenix_vault.json -t target/types/vanna_phoenix_vault.ts
  cp target/idl/vanna_phoenix_vault.json target/types/vanna_phoenix_vault.ts client/src/idl/
  cargo test -p vanna_phoenix_vault --test vanna_phoenix_vault client_vectors -- --ignored
  cd client && npm test
  ```

- The Hawkeye reads (`viewMargin`, `viewAsset`) were run against mainnet Phoenix on a live
  trader account and decode correctly.
- Phoenix runs only on mainnet, so there is no devnet. Integrate against mainnet with
  `prepareDeposit` and `prepareWithdraw`: they simulate without sending, so a page can run
  every check with no funds at risk. Then send small real amounts.
- The program's own test suite (65 tests: real Phoenix, Jupiter, Orca and Raydium programs in
  LiteSVM) covers the on-chain side: `cargo test -p vanna_phoenix_vault`.
