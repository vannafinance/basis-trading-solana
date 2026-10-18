# Frontend integration: Vanna Phoenix basis vaults

How a web app reads the vaults and lets users deposit and withdraw. Everything here goes
through the TypeScript client in [`client/`](../client), whose functions this guide documents.
The client is checked byte for byte against the program's own Rust types (`npm test`).

- Program: `vanna_phoenix_vault`, `GtuQoYxzytGKtLqQGkYrVbsaPHTZN5jboP25DrSmKvGr` (Solana mainnet).
- IDL: [`client/src/idl/vanna_phoenix_vault.json`](../client/src/idl/vanna_phoenix_vault.json),
  with TypeScript types in `vanna_phoenix_vault.ts` next to it.
- Design and math: [README.md](../README.md). Security review: [AUDIT.md](../AUDIT.md).

## Contents

1. [What the frontend does](#1-what-the-frontend-does), and
   [the functions that matter](#the-functions-that-matter): instructions, user calls, share price
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

There is one vault per asset: SOL, ETH, BTC, TSLA and META. A user deposits **USDC** and
receives the vault's **shares**, an SPL token with 6 decimals, and withdraws **USDC**. The vault
buys the asset with the deposit through Jupiter and holds it with an equal short on Phoenix
perps, so the share price stays flat in dollars and grows with funding.

| User action | Client function | What happens on-chain, in one transaction |
|---|---|---|
| See a vault | `fetchVault`, `getVaultState` | Nothing; reads accounts and simulates Phoenix's views |
| See their position | `getUserPosition` | Nothing; reads their share balance |
| Deposit | `prepareDeposit` → wallet signs | USDC in; part of it buys the asset through Jupiter; the rest is USDC margin on Phoenix; short opened; shares minted |
| Withdraw | `prepareWithdraw` → wallet signs | Their part of the short closed; their part of the asset sold through Jupiter; shares burned; USDC paid out |

Deposits and withdrawals settle at once: there are no requests, queues or lockups. Each user
pays their own trading costs.

### The functions that matter

The contract has eight instructions, and users only ever call two of them: `deposit` and
`withdraw`. There is no on-chain "get price" instruction. The program prices shares inside
`deposit` and `withdraw`, and the client reads the same price off-chain with the same math.

**On-chain instructions** (`vanna_phoenix_vault`, see the [IDL](../client/src/idl/vanna_phoenix_vault.json)):

| Instruction | Called by | What it does |
|---|---|---|
| `deposit(gti, atb, usdc_amount, min_shares, route)` | **User** | Takes USDC, buys the asset through Jupiter, opens the matching short on Phoenix and mints shares. Fails if it would mint fewer than `min_shares` |
| `withdraw(gti, atb, shares, min_usdc_out, route?)` | **User** | Closes the user's part of the short, sells their part of the asset through Jupiter, burns the shares and pays USDC. Fails if it would pay less than `min_usdc_out` |
| `rebalance(gti, atb, buy_asset, asset_amount, usdc_amount)` | Keeper | Swaps between USDC and the asset with the keeper, within `keeper_band_bps` of the mark, then trades the short to match. Reinvests funding. With both amounts zero it only re-hedges |
| `set_deposits_open(open)` | Keeper or admin | Opens or closes one vault's deposits. The keeper closes them when funding turns poor |
| `initialize(keeper, swap_program)` | The program's upgrade authority, once | Creates the config; the caller becomes the admin |
| `set_config(admin, keeper, swap_program, paused)` | Admin | Changes the admin, the keeper, the swap venue (Jupiter or off) and the protocol-wide pause |
| `create_vault(kind, market, params, authority_nonce)` | Admin, once per asset | Creates a vault with its share mint and token accounts, and registers its Phoenix trader |
| `update_vault(params)` | Admin | Changes a vault's caps, price bands and settings |

**Client functions behind each user action** (`client/src`):

| User action | Function | Gives the UI |
|---|---|---|
| Deposit | `prepareDeposit(ctx, user, usdcAmount, slippageBps)`, then the wallet's `sendTransaction` | The transaction to sign, and the exact `shares` and `valueUsd` from a simulation |
| Withdraw | `prepareWithdraw(ctx, user, shares, slippageBps)`, then the wallet's `sendTransaction` | The transaction to sign, and the exact `usdcOut` and `sharesBurned` |
| Is the deposit button enabled? | `depositStatus(ctx, payer)` | `open` and the `reasons` it isn't |
| Their shares and what they are worth | `getUserPosition(connection, vault, state, user)` | `{ shares, valueUsd }` |
| Their USDC | `getUsdcBalance(connection, vault, user)` | USDC balance (6 dp) |
| Their history and earnings | `getUserSummary(connection, program, vault, state, user)` | Deposits, withdrawals, `earningsUsd` |
| The vault page | `getVaultState(connection, vault, payer, phoenix)`, or `getVaultPage(...)` for every field | NAV, share price, positions, APY, funding (section 4) |

**The share (LP token) price** is `getVaultState(...).sharePrice`: USD per whole share, with 6
decimals (`1_000_000n` is $1.00).

```ts
const state = await getVaultState(connection, vault, wallet.publicKey ?? VANNA.viewPayer, phoenix);
state.sharePrice   // USD per 1 share (6 dp)
state.nav          // the vault's value (USD, 6 dp)
state.supply       // shares outstanding (6 dp)
```

It is computed exactly the way the program prices a withdrawal:

```
NAV        = asset held × Phoenix mark price
           + Phoenix equity (USDC collateral + unrealized PnL + unsettled funding)
sharePrice = 1e6 × (NAV + 1e6) / (supply + 1e6)
```

The `+ 1e6` terms are the program's virtual share and dollar. They make the first share cost $1
and change the price of a vault with real deposits by a negligible amount.

| Need | Function |
|---|---|
| Price of one share | `getVaultState(...).sharePrice` |
| What a user's shares are worth | `getUserPosition(...).valueUsd`, or `shareValue(shares, state.nav, state.supply)` |
| Shares a dollar amount buys | `sharesForValue(value, state.nav, state.supply)`; exact: `prepareDeposit(...).shares` |
| Exact USDC for withdrawing some shares, after costs | `prepareWithdraw(...).usdcOut` |
| Share price at a past deposit | `sharePriceAtDeposit({ valueUsd, shares })`, from a `deposited` event's fields as `bigint` |
| APY from share-price growth | `realizedApy(events, state.sharePrice, days)` |

`sharePrice` is before exit costs. A withdrawal pays slightly less: the user's own Phoenix fee,
spread and Jupiter price impact. Show `prepareWithdraw(...).usdcOut` as the amount they get.

Another program (the Vanna credit layer valuing shares as collateral, say) cannot call a price
function on this contract. It has to read the same accounts and compute the same NAV itself.

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
| `targetMarginBps` | USDC margin a deposit keeps per dollar of asset it buys: `10000` for spot vaults (half and half), `0` for SOL (all of it buys SOL) |
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

### The vault page: every field and its getter

`getVaultPage(connection, program, vault, payer, phoenix)` reads everything below in parallel
(`client/src/views.ts`). Rates are percents (`number`); amounts are `bigint`, USD in 6 decimals.

| UI field | Getter | Source |
|---|---|---|
| TVL | `page.state.nav` | Vault state |
| Cap, room left | `page.capacity.capUsd`, `.roomUsd`, `.binding` | Vault caps; open interest from Phoenix's API. The SOL vault's cap is 450 SOL, not a dollar amount |
| Vault APY | `page.apy` (`apyPct`, `estimated`, `label`) | The realized 7-day APY (`page.apy7d`) once the events hold a deposit 7 days old, else the estimate `page.yield7d.netPct` labelled "Est. APY". Never show `0.00%` for a missing history |
| Vault APY (7D), (30D) | `page.apy7d`, `page.apy30d` | Share price at a deposit that old (events) to now; null without enough history |
| Funding now | `page.funding.nowApr` | Phoenix's last hourly rate × 8,760 |
| Net delta, hedge | `page.positions.netDeltaPct`, `.hedgeRatio` | Asset held vs the short, in market units |
| Long leg | `page.positions.long` (`marketUnits`, `usd`, `heldIn`) | SOL: Phoenix collateral. Others: the vault's token account |
| Short leg | `page.positions.short` (`marketUnits`, `usd`) | Phoenix position |
| Spot / margin split | `page.positions.assetPct`, `.marginPct` | SOL: about 100 / 0. Others: about 50 / 50 |
| Price rise to liquidation | `page.positions.liquidationRisePct`, `.liquidationPrice` | Phoenix's effective collateral and maintenance margin. SOL about +350%, spot vaults about +96% |
| Yield breakdown | `page.yield7d` (`fundingPct`, `rebalancingPct`, `vaultFeePct`, `netPct`) | 7-day funding × `fundingExposure`; keeper rebalance costs from events. No vault fee |
| Negative funding days | `page.funding.negativeDays` / `.days` | Phoenix's hourly funding, summed per UTC day |
| Rebalanced … ago | `page.lastRebalance?.blockTime` | Newest `rebalanced` event read |
| All transactions | `page.events` | `fetchEvents(connection, program, vault.address, { limit, before })` |
| Your position, earnings | `getUserSummary(connection, program, vault, page.state, user)` | Shares × share price; earnings = value + USDC withdrawn − USDC deposited |
| Wallet USDC | `getUsdcBalance(connection, vault, user)` | The user's USDC account |
| You receive (shares) | `estimateDepositShares(page.state, usdc)` as they type; `prepareDeposit` for the exact amount | Share price now, less about 10 bps of entry cost |
| Net APY, est. earnings | `estimateEarnings(amount, apy, days, borrowApr)` | Pass the Vanna credit layer's borrow rate when the USDC is borrowed |

Not from this program: the USDC borrow APY, the margin account balance and the health factor
come from the Vanna credit layer.

## 5. Deposit

Deposits are **USDC only**. Part of the USDC buys the asset at spot through Jupiter and the rest
stays as the short's margin on Phoenix:

| Vault | $1,000 deposit | Client call |
|---|---|---|
| ETH, BTC, TSLA, META | $500 buys the asset, $500 is margin (1x) | `prepareDeposit(ctx, user, 1_000_000_000n)` |
| SOL | all $1,000 buys SOL, which is Phoenix collateral itself | the same |

### `prepareDeposit(ctx, user, usdcAmount, slippageBps = 50)`

It does the following:
1. Works out the USDC that buys the asset (`depositSwapAmount`: a fixed fraction, so exact) and
   fetches a Jupiter route for it, built for the vault authority (`jupiterRoute`).
2. Builds the deposit and simulates it. This gives the exact shares, or the program's error.
3. Rebuilds with `minShares = shares × (1 − slippageBps)` and returns a transaction for the
   wallet.

It returns `{ transaction, shares, valueUsd, assetBought, preview, route }`, or throws
`VaultActionError`, whose `preview.error` holds the program error (section 9).

```ts
import { prepareDeposit, VaultActionError } from "@vanna/phoenix-vault-client";

try {
  const { transaction, shares, valueUsd } = await prepareDeposit(ctx, wallet.publicKey, 1_000n * 1_000_000n);
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
(about 0.002 SOL) once. A SOL-vault deposit also opens and closes a wrapped-SOL account for the
swap; its rent goes back to the user in the same transaction. Leave about 0.01 SOL for fees and
rent.

## 6. Withdraw

Withdrawals pay **USDC only**. The vault closes the user's part of the short, sells their part
of the asset through Jupiter and pays that USDC plus their part of the vault's USDC.

### `prepareWithdraw(ctx, user, shares, slippageBps = 50)`

It does the following:
1. Estimates the asset the withdrawal sells (`withdrawSwapEstimate`): a spot vault's share of
   its tokens, or for the SOL vault the payout in SOL less about 0.1% for closing the short.
2. Fetches a Jupiter route for that amount. The vault decides the exact amount inside the
   transaction (it depends on the user's own trade) and **resizes the route to it**, so the
   estimate only has to be close and nothing is left unsold.
3. Simulates the withdrawal, sets `minUsdcOut` from the result less `slippageBps`, and returns
   the transaction. If Phoenix lets no asset leave now (the SOL vault's SOL held for USDC it
   owes), the program answers `SwapNotNeeded` and the client retries without a route.
4. Prepends an instruction that creates the user's USDC account if it is missing (about
   0.002 SOL of rent the first time).

It returns `{ transaction, usdcOut, assetSold, payoutUsd, sharesBurned, preview, route }`.

```ts
const shares = (await getUserPosition(connection, vault, state, wallet.publicKey)).shares;
const w = await prepareWithdraw(ctx, wallet.publicKey, shares);
// Show: "You receive ${fmt(w.usdcOut, 6)}"
if (w.sharesBurned < shares) {
  // Phoenix let only part of the payout leave now (the SOL vault with USDC owed, or Phoenix's
  // withdraw limits): the user keeps the rest of their shares and can withdraw them later.
}
await wallet.sendTransaction(w.transaction, connection);
```

Withdrawals stay open during pauses, closed deposits, stock after-hours and over caps. They do
need Jupiter: while Jupiter is down, or if Vanna turns swaps off, withdrawals that have asset to
sell fail until it is back.

**Large withdrawals.** Each withdrawal closes its share of the short on Phoenix within 1% of the
mark and sells its asset within 1% of the mark, all at once, or it fails. If the book or the
pool is too thin (a large amount, or a stock on a weekend), split it into several withdrawals.
The preview fails with Phoenix's or the vault's error when the size cannot fill.

## 7. Sending transactions

- **Lookup tables are required.** A deposit or withdrawal names 46–50 accounts with a direct
  Jupiter route, which does not fit Solana's 1,232-byte limit without lookup tables. Pass the
  vault's table (from config) plus the route's Jupiter tables. `prepareDeposit` and
  `prepareWithdraw` already do this, and so does
  `buildTransaction(connection, payer, ixs, tables, options)`.

  | Operation (direct Jupiter route) | Accounts | With lookup tables | Compute used |
  |---|---|---|---|
  | SOL deposit | 46 | 415 bytes | 331k |
  | SOL withdraw | 46 | 416 bytes | 345k |
  | ETH deposit | 46 | 415 bytes | 318k |
  | ETH withdraw | 46 | 416 bytes | 332k |
  | TSLA deposit (Raydium CLMM) | 50 | 425 bytes | 330k |
  | TSLA withdraw (Raydium CLMM) | 50 | 426 bytes | 359k |

- **Compute budget:** the client sets 600,000 units with a swap (every deposit, and every
  withdrawal with asset to sell) and 400,000 without. Add a priority fee with
  `tx.microLamportsPerCu`.
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
| `deposited` | `basisVault, user, usdcAmount, assetBought, shares, valueUsd, navBeforeUsd, positionLots` | Every deposit. `assetBought` is what the USDC bought through Jupiter |
| `withdrawn` | `basisVault, user, shares` (burned), `payoutUsd, assetSold, usdcOut, positionLots` | Every withdrawal. `usdcOut` is what the user actually received; `assetSold` is their part of the asset that was sold |
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
| 6020 | UnsupportedRoute | "Swap failed" | Client bug: the route is not Jupiter's `route` or `shared_accounts_route` |
| 6021 | MarginTooThin | "The vault cannot take this deposit right now" | Retry later; alert ops |
| 6022 | SharesTooLow | "Price moved; try again" | Rebuild with a fresh preview |
| 6023 | PayoutTooLow | "Price moved; try again" | Rebuild with a fresh preview |
| 6024 | NothingDeposited | "Amount too small" | Minimum-amount check |
| 6025 | NothingToWithdraw | "Phoenix lets nothing leave right now; try later" | SOL vault owes USDC; keeper settles |
| 6026 | NotEnoughShares | "Not enough shares" | Validate against the balance |
| 6027 | PriceOutOfBand | "Price moved; try again" | Retry |
| 6029 | InvalidSwapAccounts | "Swap failed" | Rebuild the route |
| 6030 | SwapNotNeeded | — | A withdrawal sent a route but Phoenix lets no asset leave now: resend without one (`prepareWithdraw` does) |
| 6031 | SwapAmountOff | "Price moved; try again" | Rebuild with a fresh route |
| 6032 | SwapPriceTooLow | "Swap price too far from the market; try again or use a smaller amount" | Retry, or split the amount |
| 6033 | SwapMovedVaultFunds | "Swap failed" | Report: bad route |
| others | InvalidMint, InvalidTokenAccount, InvalidPhoenixAccount, MissingPhoenixAccounts, WrongVaultKind, … | "Something went wrong" | Client or config bug: refresh `fetchPhoenixKeys` and the vault |

Codes 6000, 6004, 6005, 6010, 6011, 6016, 6018 and 6034 come from admin or keeper actions or
internal checks, which users do not trigger.

## 10. Asset notes

| Vault | Asset | Notes |
|---|---|---|
| SOL | Native SOL (lamports) | The SOL is the short's margin, so all of a deposit buys SOL. Capped at 450 SOL by Phoenix's 500-SOL limit per trader. A withdrawal takes SOL out first and sells it. If the vault owes USDC after a rally, a payout can be partial (`sharesBurned < shares`) until the keeper settles it |
| ETH | Wormhole ETH `7vfCXTUXx5WJV5JADk17DUJ4ksgau7utNKj4b963voxs` | The vault holds Wormhole ETH, not native ETH; users never touch it, they deposit and withdraw USDC |
| BTC | cbBTC `cbbtcf3aa214zXHbiAZQwf4122FBYbraNdFqgw4iMij` | As ETH |
| TSLA, META | xStocks `XsDoVfqe…`, `Xsa62P5m…` (Token-2022) | The vault holds the tokens; users deposit and withdraw USDC. One METAx is about 1.00285 shares (reinvested dividends); value it with `tokenPrice`. Deposits are closed after hours (weekends, US holidays) and around large dividends or splits. **xStocks are not offered to US persons; geofence accordingly.** The issuer can pause transfers, which also pauses withdrawals |

## 11. Low-level reference

Use these functions instead of the flows if you need full control.

```ts
// Instructions
depositInstruction(program, { vault, user, usdcAmount, minShares, route, phoenix? }): Promise<TransactionInstruction>
withdrawInstruction(program, { vault, user, shares, minUsdcOut, route?, phoenix? }): Promise<TransactionInstruction>
withdrawSetupInstructions(vault, user): TransactionInstruction[]       // create the user's USDC account

// Swaps
depositSwapAmount(usdcAmount, targetMarginBps): bigint                  // the USDC that buys the asset
withdrawSwapEstimate(kind, shares, supply, assetUnits, nav, tokenPrice, decimals): bigint  // about the asset sold
jupiterRoute(vault, sell, amount, { apiBase?, apiKey?, slippageBps?, maxAccounts?, dexes? }): Promise<SwapRoute>

// Transactions
buildTransaction(connection, payer, instructions, lookupTables, { computeUnits?, microLamportsPerCu? }): Promise<VersionedTransaction>
simulate(connection, program, tx): Promise<{ ok, logs, events, error, unitsConsumed }>
parseEvents(program, logs); vaultErrorFromLogs(logs)

// Math (the program's own arithmetic, in bigint)
tokenPrice(markTicks, tickSize, baseLotDecimals, multiplier?); usdValue; tokensForUsd
sharesForValue(value, nav, supply); shareValue(shares, nav, supply); lessBps(amount, bps)

// Addresses
configAddress(); basisVaultAddress(assetMint); shareMintAddress(basisVault)
authorityAddress(basisVault, nonce); traderAccountAddress(authority); userShareAccount(user, shareMint)
```

### The raw instructions

If you build instructions without the client (for example from another language), use the
IDL. The account lists below are exactly what `program.methods.deposit(...)` encodes.

**`deposit(gti: u8, atb: u8, usdc_amount: u64, min_shares: u64, route: SwapRoute)`**

| # | Account | Notes |
|---|---|---|
| 1 | `user` | Signer, writable |
| 2–25 | `core` (24 accounts): `config, basis_vault, authority, share_mint, usdc_mint, phoenix_mint, vault_usdc, vault_phoenix, vault_asset?, asset_mint, asset_token_program, trader_account, [phoenix_program, log_authority, global_config, hawkeye_program, ember_program, ember_state, ember_vault], perp_asset_map, orderbook, spline_collection, withdraw_queue, global_vault` | All from `VaultInfo` and the constants. `vault_asset` is empty (the program id) for SOL |
| 26 | `user_shares` | The user's share ATA (SPL Token); created by the program if missing |
| 27 | `user_usdc` | The user's USDC ATA |
| 28 | `swap_program` | Jupiter v6 |
| 29 | `vault_wsol?` | The authority's wrapped-SOL ATA, for the SOL vault |
| 30–32 | `token_program, associated_token_program, system_program` | |
| remaining | Phoenix's global trader index accounts (`gti` of them) and active trader buffer accounts (`atb`), writable; then the route's accounts (`route.accounts` of them) as Jupiter returns them | |

An absent optional account (`?`) is passed as the program's own id, as Anchor does.

`SwapRoute { accounts: u8, data: Vec<u8> }`: Jupiter's `swapInstruction.data` (a `route` or
`shared_accounts_route` instruction), and the number of its accounts appended to
`remainingAccounts`, with the vault authority's signer flag cleared. The vault rewrites the
route's input amount (and scales its quoted output) to the exact amount it sells.

**`withdraw(gti, atb, shares: u64, min_usdc_out: u64, route: Option<SwapRoute>)`**: the same
accounts as `deposit`; `swap_program` and `vault_wsol` are passed only with a route. A route is
required exactly when the withdrawal has asset to sell.

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
- The program's own test suite (64 tests: real Phoenix, Jupiter, Orca and Raydium programs in
  LiteSVM, and a mock Jupiter where tests move prices) covers the on-chain side:
  `cargo test -p vanna_phoenix_vault`.
