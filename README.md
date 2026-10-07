# Vanna basis vaults on Solana (Phoenix)

`vanna_phoenix_vault` is an Anchor program with **one vault per asset** (SOL, ETH, BTC, and
tokenized stocks: TSLA, META). Each vault holds the asset and a short of the same size on
**Phoenix perpetuals**, so its value stays flat in USD while it earns the funding that longs
pay shorts. There are no fees, no stablecoin, and nothing is shared between vaults. Every vault
has **caps** (below) that keep it a minority of its market's shorts.

- Users deposit and withdraw **USDC only**. The vault buys the asset with the deposit through
  Jupiter and sells the leaver's part back to USDC on the way out.
- A user deposits and gets that vault's **shares**. The share price is **NAV / supply**.
- Phoenix fills market orders in the same instruction, so deposits and withdrawals settle at
  once. There are no requests waiting for a keeper, unlike the GMTrade version.
- Every deposit and withdrawal is priced from the NAV before and after its own trade, so each
  user pays their own spread and taker fee and nobody else's.
- Funding raises the NAV as soon as it accrues. The keeper **reinvests** it into more of the
  asset with a bigger short, so later funding is earned on it too.

## SOL vault and ETH/BTC/stock vaults work differently

Phoenix takes only USDC (as PhUSD, through Ember) and native SOL as collateral.

| | SOL vault (`NativeSol`) | ETH, BTC, TSLA, META vaults (`Spot`) |
|---|---|---|
| Where the asset sits | in the vault's Phoenix trader account, as collateral | in the vault's own token account (Token-2022 for stocks) |
| Short | same size as the SOL held | same size as the asset held (in shares, for stocks) |
| A $1,000 deposit | all of it buys SOL | half buys the asset, half stays as margin |
| Margin | the SOL itself (80% counted) | USDC on Phoenix, 1x by default |
| Payout (USDC) | SOL leaves first and is sold; USDC margin only once the short is gone | the leaver's part of the tokens, sold, plus their part of the USDC |

## Stock vaults (TSLA, META)

A stock vault holds Backed's **xStocks** (TSLAx `XsDoVfqe…`, METAx `Xsa62P5m…`) and shorts
Phoenix's TSLA or META perp. It is a `Spot` vault like ETH; what differs:

- **Token-2022.** xStocks are Token-2022 mints. The vault stores each asset's token program,
  moves the asset with `transfer_checked`, and accepts the xStock extensions (permanent
  delegate, pausable, default account state, metadata, confidential transfer mint, a transfer
  hook with no program). `create_vault` refuses mints with transfer fees, interest,
  non-transferable tokens, a frozen default state or an active transfer hook.
- **The dividend multiplier.** Backed reinvests dividends by raising the token's scaled UI
  multiplier: one METAx is 1.0028515 META shares today (TSLA pays none: 1.0). The vault reads
  the multiplier from the mint (the new one once its start time passes) and uses it everywhere:
  a token is priced at `mark × multiplier`, and the short is `tokens × multiplier` shares. 10
  METAx is a 10.029 META short, not 10 (`a_meta_token_is_worth_more_than_a_share_and_the_short_follows`).
- **After hours.** Phoenix trades stock perps 24/5 (Sunday 8pm to Friday 8pm New York); on
  weekends and holidays it runs the market "after hours" (price held within 5% of the last
  close, thin book) and then reopens it. The vault reads Phoenix's flag from the perp asset map;
  with `after_hours_deposits = false` deposits wait for the session (`MarketAfterHours`).
  Withdrawals are always open (`stock_deposits_wait_for_the_session_but_withdrawals_do_not`).
- **Issuer risk.** Backed can pause TSLAx transfers, freeze accounts and move tokens with its
  permanent delegate. While paused, nothing that moves tokens works: withdrawals revert (and
  burn nothing) until Backed unpauses (`a_paused_xstock_stops_token_moves_until_backed_unpauses`).
  xStocks are not offered to US persons.

## Caps

Shorts earn funding only while longs outweigh them. A vault that is most of a market's short
side pushes funding down on itself, so every vault is capped against its market. The program
enforces, on every deposit and every keeper reinvest that grows the short or the holding:

| Cap | Parameter | Error |
|---|---|---|
| The short is at most a share of the market's open interest (read on-chain from Phoenix's perp asset map) | `max_oi_share_bps` | `OpenInterestShareReached` |
| The short's notional | `max_position_usd` | `MaxPositionReached` |
| The asset held (the SOL vault's native SOL limit) | `max_asset_units` | `MaxPositionReached` |
| The value one deposit adds | `max_deposit_usd` | `DepositTooLarge` |

Caps never stop withdrawals, and a deposit that only adds USDC margin passes even over a cap.
The keeper (or the admin) opens and closes deposits with `set_deposits_open`: the **funding
guard**. It closes a vault's deposits when its market's 7-day funding falls below about 8% a
year, and reopens them when it recovers.

Launch caps, from Phoenix and Jupiter on 2026-10-06 (`mainnet_params` in the tests):

| Vault | Phoenix open interest | Book within 0.5% (sell / buy) | Jupiter $100k vs mark (buy / sell) | Funding, 30 d / 7 d | Short cap | One deposit | Other |
|---|---|---|---|---|---|---|---|
| SOL | $7.9M | $2.2M / $2.1M | −0.07% / −0.09% | 20.5% / 16.2% | 25% of OI, $100k | $100k | **450 SOL** |
| ETH | $1.07M | $1.5M / $1.2M | +0.05% / −0.02% | 6.9% / −0.2% | 25% of OI, $300k | $250k | funding guard closes it today |
| BTC | $2.4M | $2.2M / $1.9M | −0.08% / −0.10% | 8.2% / 12.5% | 25% of OI, $750k | $250k | |
| TSLA | $1.18M | $353k / $294k | +0.17% / −0.28% | 28.6% / 21.2% | 25% of OI, $300k | $100k | deposits close after hours |
| META | $0.80M | $461k / $419k | +1.6% / −0.9% | 19.4% / 13.4% | 25% of OI, $200k | $50k | deposits close after hours |

- **Open interest sets the size.** At 25%, a vault can be a third of everyone else's shorts:
  about $390k for TSLA, $270k META, $360k ETH and $810k BTC today. The position caps sit a
  little under that; if open interest falls, the open-interest cap stops deposits first, and
  when it grows the admin can raise the position caps.
- **SOL is capped by Phoenix itself.** Phoenix counts at most 500 SOL of native collateral per
  trader; SOL beyond that sits in the account but adds no margin (500 → 600 SOL took the
  vault's margin from 79.9% to 66.6% of its short, `phoenix_counts_only_500_sol_per_trader_so_the_sol_vault_stops_at_450`).
  The SOL vault stops at 450 SOL (about $54k). To grow past it, the SOL vault needs more trader
  accounts or USDC margin like ETH.
- **One deposit** is sized to what the Phoenix book and Jupiter take near the mark; bigger
  amounts come in several transactions. META's xStock pools are
  the thinnest: a $50k USDC deposit swaps $25k into METAx, between Jupiter's +0.53% at $10k
  and +0.90% at $50k.
- **ETH funding** was negative over the last 7 days and negative over 14% of 3-day windows in
  the last 30: the funding guard keeps ETH deposits closed until it recovers.

## USDC in, USDC out

Users bring **USDC only** and get **USDC only** back. A deposit buys the asset at spot through
Jupiter: the vault keeps `target_margin_bps` of USDC per dollar of asset as margin and buys the
asset with the rest (`swap::deposit_swap_amount`), checks the fill against Phoenix's mark price,
and opens the short on what it ends up holding.

| User deposits ($35,000, ETH at $3,500, SOL at $150) | Vault swaps | Vault ends with |
|---|---|---|
| ETH vault | $17,500 USDC → ~5 ETH | ~5 ETH held, ~5 ETH short, $17,500 margin |
| BTC, TSLA, META vaults | the same, half and half | asset held = asset short, 1x margin |
| SOL vault | **all** of it → ~233 SOL | SOL held = SOL short, no USDC needed |

Why not keep the SOL vault's USDC as Phoenix collateral and just short? Phoenix does take
USDC, but USDC plus a short is a naked short: it loses when SOL rises. Delta neutral needs the
SOL itself, and since SOL is its own margin on Phoenix, all of the USDC buys SOL and all of it
earns funding.

A withdrawal closes the leaver's part of the short, sells their part of the asset through
Jupiter and pays them that USDC plus their part of the vault's USDC. The vault decides the
amount it sells inside the transaction (it depends on the leaver's own trade), so the app
builds the route for about that amount and the vault **resizes it to the exact amount**
(`swap::resize_route`): it sets the route's input amount and scales Jupiter's quoted output with
it. Nothing is left unsold, so nothing is paid in the asset. The leaver carries the swap's cost
and `min_usdc_out` guards what they receive.

Every swap goes through `Config.swap_program`, signed by the vault authority. It can only be
Jupiter v6 (or the default key, which turns swaps off, and with them deposits and every
withdrawal that has asset to sell): the authority is the share mint's mint authority and the
Phoenix trader, so no other program gets its signature. The vault only resizes Jupiter's own
`route` and `shared_accounts_route` instructions (`UnsupportedRoute` otherwise). After the
route the vault checks that:
- the side it sold went down by exactly the planned amount (`SwapAmountOff`),
- the side it bought went up by at least the mark-price value less `slippage_bps` (`SwapPriceTooLow`),
- its Phoenix tokens, the authority's lamports, the SOL in its trader account and the share
  supply did not move, the share mint authority is unchanged, and its token accounts are still
  the authority's with no delegate or close authority (`SwapMovedVaultFunds`).

Native SOL goes through wrapped SOL, as Jupiter does: the vault opens its wrapped-SOL account,
swaps, closes it (the user gets the account's rent back) and posts the SOL to Phoenix (deposit)
or pays the USDC (withdrawal).

### Jupiter

On mainnet `Config.swap_program` is Jupiter v6, `JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4`.
The vault books **what the route actually delivered**, never the quote: the asset it holds is
its token balance after the route, the short is sized on that balance, the margin is the USDC
that arrived, and shares are priced on the NAV after the swap and the short. Jupiter's fee and
price impact land on the depositor, like the Phoenix trade.

The app builds the route like this:

1. The amount: what the vault will sell. A deposit sells
   `usdc × 10,000 / (10,000 + target_margin_bps)` of its USDC: half at 1x, all of it for the SOL
   vault (`swap::deposit_swap_amount`). A withdrawal sells about the leaver's part of the asset:
   their share of a spot vault's tokens, or for the SOL vault their payout in SOL less about
   0.1% for closing their short (the client's `withdrawSwapEstimate`). The vault resizes the
   route to the exact amount either way.
2. `GET /swap/v1/quote` for that amount.
3. `POST /swap/v1/swap-instructions` with `userPublicKey` = the vault authority PDA,
   `wrapAndUnwrapSol: false` (the vault wraps SOL itself), `useSharedAccounts: true`,
   `destinationTokenAccount` = the authority's account for the output mint (its wrapped-SOL
   account for the SOL vault) and `skipUserAccountsRpcCalls: true`.
4. Pass `swapInstruction.data` as the route data and its accounts, with the authority's signer
   flag cleared (the vault signs for it), after Phoenix's index accounts. Leave out Jupiter's
   setup and cleanup instructions: they would need the PDA's signature, and the vault opens and
   closes its own accounts.
5. Send a v0 transaction with Jupiter's lookup tables plus the vault's.

`scripts/jupiter_fork.py` does steps 2 and 3 for the tests.

**Jupiter is now on every path.** If Jupiter is down, or the admin turns swaps off, nobody can
deposit and holders whose withdrawal has asset to sell cannot leave until it is back (a SOL
vault holder whose SOL is held for the vault's USDC debt still gets the USDC Phoenix releases).

## Pieces

| Piece | What it is |
|---|---|
| `Config` | Admin, keeper, swap venue, pause, USDC and PhUSD mints, Phoenix's perp asset map, withdraw queue and token vault |
| `BasisVault` (one per asset) | Kind, market (asset id, orderbook, spline, lot and tick sizes), token accounts, parameters |
| Authority PDA (one per vault) | Data-less, at `["authority", basis_vault, nonce]` with the admin's random nonce. The Phoenix trader, owner of the vault's token accounts, the share mint's mint authority |
| Share mint (one per vault) | The vault's LP token. 6 decimals: the first dollar buys one share. A plain SPL token with **no freeze authority and no lockup**: holders can move shares to anyone at any time, and whoever holds them can withdraw |

Vault parameters: the caps (`max_position_usd`, `max_asset_units`, `max_oi_share_bps`,
`max_deposit_usd`), `slippage_bps` (price band around the mark for the vault's orders and
swaps, 10 to 1,000 bps), `keeper_band_bps` and `max_rebalance_usd` (the keeper's own band and
daily volume), `margin_bps` (Phoenix effective collateral the vault keeps per dollar of short, a
floor), `target_margin_bps` (USDC a deposit keeps as margin per dollar of asset it buys: 10,000
for ETH, BTC and stocks, 0 for SOL), `deposits_open`, `after_hours_deposits`.

## Instructions

| Instruction | Who | What |
|---|---|---|
| `initialize` | upgrade authority | Config; only the program's upgrade authority can call it |
| `set_config` | admin | Admin, keeper, swap venue, pause |
| `create_vault(kind, market, params, authority_nonce)` | admin | Vault, share mint, token accounts, registers the Phoenix trader. The market must match Phoenix's listing; the authority's address takes a random nonce, so nobody can register its trader first |
| `update_vault` | admin | Parameters and caps |
| `set_deposits_open(open)` | keeper or admin | The funding guard: closes or reopens a vault's deposits |
| `deposit(usdc, min_shares, route)` | user | Brings USDC, buys the asset with the part that is not margin through Jupiter, trades the short to match, mints shares |
| `withdraw(shares, min_usdc, route)` | user | Shrinks the short, sells the leaver's part of the asset through Jupiter, pays USDC, burns shares |
| `rebalance(buy_asset, asset, usdc)` | keeper | Swaps asset and USDC with the keeper within `keeper_band_bps` of the mark, up to `max_rebalance_usd` a day, then re-hedges; with both amounts zero, re-hedges only |

`deposit`, `withdraw` and `rebalance` take Phoenix's global trader index and active trader
buffer accounts at the front of `remaining_accounts` (`gti`, `atb` say how many), followed by
the route's accounts when there is one.

## Value

```
NAV = asset held × Phoenix mark price × multiplier + USDC collateral + unrealized PnL + unsettled funding
```

The multiplier is 1 except for a stock token that reinvested dividends.

Everything is read from Phoenix through its Hawkeye view program, and the asset and the short
are always valued at the same mark price. Phoenix's mark can move with the vault's own trade,
so the NAV after a trade is read again afterwards. A hedged vault's NAV does not move with the
price (`sol_moving_does_not_move_the_vault_and_everyone_exits_fairly`).

Shares for a deposit: `(NAV_after − NAV_before) × (supply + 1e6) / (NAV_before + 1e6)`.
Payout for a withdrawal: `NAV_after − ((NAV_before + 1e6) × (supply − shares + 1e6) / (supply + 1e6) − 1e6)`:
the part that stays is priced exactly like a deposit, so depositing and withdrawing at once
never moves value between holders.

## Rebalancing and reinvesting funding

The 50/50 split happens at deposit. `rebalance` is the keeper's upkeep of the whole vault,
for three reasons:

1. **Reinvesting funding.** Phoenix pays funding into the vault's USDC collateral; the NAV
   counts it at once. `rebalance(buy_asset = true)` turns that USDC into more of the asset and
   a bigger short, so later funding is earned on it too.
2. **Restoring margin after a move.** When the asset rallies, the short loses USDC while the
   held asset gains: `buy_asset = false` sells some of the vault's asset for USDC margin (ETH
   +30%: margin 54% → back to 100% in `eth_rallies_the_keeper_moves_value_to_margin_and_carol_exits_whole`).
   After a drop the margin is above target and `buy_asset = true` puts the extra to work.
3. **The SOL vault's USDC.** It pays down USDC the SOL vault owes after a rally, and keeps a
   USDC buffer above the short's initial margin so funding can be taken out and reinvested
   (SOL never backs a USDC withdrawal on Phoenix).

Today the keeper is the counterparty: it hands the vault the asset (or USDC) and takes the
other side, within its own `keeper_band_bps` of the mark (20 bps at launch) and at most
`max_rebalance_usd` a day ($50,000 at launch), so a stolen keeper key can cost a vault at most
about $100 a day. Reinvesting may not take the vault's margin below `margin_bps` or the vault
past its caps. The keeper runs it whenever funding has built up and after big moves, and
re-hedges (`rebalance` with no amounts) what withdrawals leave: a withdrawal only closes its
own share of the short, so donated asset, or a short Phoenix liquidated, waits for the keeper.

The full security review is in [AUDIT.md](AUDIT.md). Frontend integration (a TypeScript client in
`client/` and its guide) is in [docs/FRONTEND_INTEGRATION.md](docs/FRONTEND_INTEGRATION.md).

## What Phoenix imposes (mainnet values read on 2026-10-05)

- **Native SOL collateral: 500 SOL per trader account** (50,000 SOL across Phoenix, 7,914 used
  on 2026-10-06), counted at 80%. Past that, extra SOL does not count as margin, so the SOL
  vault is capped at 450 SOL (`max_asset_units`). More capacity needs more trader accounts
  (subaccounts) or USDC margin.
- **SOL never backs a USDC withdrawal.** USDC leaves the SOL vault only from collateral above
  the short's initial margin (5%). So the keeper keeps a USDC buffer in the SOL vault before it
  can reinvest funding (`funding_raises_the_share_price_and_the_keeper_reinvests_it`).
  Phoenix's own `SwapNative` (USDC ↔ SOL inside the trader account through Jupiter) is the
  production alternative; it is not wired in yet.
- **USDC the SOL vault owes must stay covered by SOL at 80%.** After a rally, closing the short
  realizes its loss in USDC. A withdrawal then pays what Phoenix lets leave and burns shares
  only for that; the holder keeps the rest until the keeper's `rebalance` pays the debt
  (`after_a_rally_the_vault_owes_usdc_and_the_keeper_settles_it`).
- **Deposit cooldown: 0 slots on mainnet.** If Phoenix turns it on, USDC withdrawals wait it out
  after any USDC deposit into the vault (`a_phoenix_deposit_cooldown_would_delay_usdc_withdrawals`).
  Native SOL withdrawals are not held back.
- **Onboarding is permissioned.** `create_vault` registers the vault's trader; Phoenix must then
  grant it trading capabilities before it can trade.

## Tests

The integration tests run the **real mainnet Phoenix, Ember and Hawkeye programs** in Phoenix's
own LiteSVM fixture (`phoenix-rise-litesvm-test`), with mainnet's native SOL limits and
cooldown, on the mainnet USDC, Wormhole ETH and cbBTC addresses and the mainnet TSLAx and
METAx mints (their real Token-2022 data, extensions and multiplier). Phoenix's fixture lists
BTC, ETH and SOL; the tests add TSLA and META markets by repeating its SOL market setup with
mainnet's stock lot and tick sizes. Fixture prices: SOL 150, ETH 3,500, BTC 100,000, TSLA 382,
META 746. The fixture maker quotes 0.05% around the price.

Every deposit and withdrawal swaps, so every test swaps too, in one of two ways:

- **Real Jupiter** (`jupiter.rs`, the TSLA and META Raydium tests, transaction sizes): a real
  Jupiter v6 route over a real pool (Orca Whirlpool for SOL, ETH and BTC; Raydium CLMM, where
  xStocks trade deepest, for TSLAx and METAx). The tests load the Jupiter, Whirlpool and Raydium
  programs and every account the routes touch, snapshotted from mainnet by
  `scripts/jupiter_fork.py` (routes and accounts in `tests/fixtures/jupiter_routes.json`,
  programs cached under `target/deploy/.cache/mainnet`), with the clock at the snapshot's time
  and Phoenix's marks between the routes' buy and sell prices. There is one route each way per
  vault, and the vault resizes it to the amount it sells, as it does on mainnet. The routes are
  pinned to one AMM each: Jupiter's best routes often go through proprietary market makers
  whose quotes expire within slots, which a snapshot cannot replay. On mainnet the vault takes
  whatever route Jupiter gives.
- **A mock Jupiter** (`tests/mock_jupiter`, loaded at Jupiter's address) everywhere else. The
  snapshotted pools only trade at mainnet's prices, and most tests move Phoenix's price (SOL to
  $180, ETH to $4,550). The mock takes the amount the vault resized its route to and pays the
  route's quote, which the test sets at Phoenix's mark, from a reserve it holds of every token.

| File | Covers |
|---|---|
| `sol_vault.rs` | $1,500 → 10 SOL bought, 10 SOL short; price moves; fair entry and exit for two users in USDC; partial withdrawal; cap and price band; USDC debt after a rally |
| `spot_vaults.rs` | ETH: $35,000 → 5 ETH held and short, $17,500 margin; a worse fill is the depositor's and the short follows what arrived; rally + keeper margin top-up + exit in USDC; BTC two users and a drop; deposit cooldown |
| `reinvest.rs` | Funding accrues and is reinvested (SOL and ETH); negative funding; keeper role and price band |
| `jupiter.rs` | Through real Jupiter: USDC deposits into the ETH, BTC and SOL vaults; USDC payouts that sell exactly the leaver's ETH and SOL; routes resized; bad routes refused |
| `stock_vaults.rs` | TSLA and META: $10,000 and $5,000 in and out in USDC through Jupiter (Raydium); the multiplier sizes the short and the NAV holds when META moves; after hours; a paused xStock |
| `caps.rs` | TSLA at launch caps: open-interest share, one-deposit size, position cap, withdrawals over the cap; Phoenix's 500 SOL limit and the 450 SOL cap; the keeper's funding guard |
| `audit.rs` | One test per audit finding (AUDIT.md): Jupiter-only venue, keeper limits, upgrade-authority `initialize`, pre-registered traders, market checks, fake Phoenix index accounts, donations and withdrawals, multiplier changes, price-band floor, every deposit buying at spot, insolvent vaults, symmetric share pricing, the first-deposit attack |
| `guards.rs` | Shares are plain tokens (no freeze authority; withdraw in the same slot; transferred shares redeemed by their new holder); wrong market, fake Phoenix accounts, closed deposits, transaction sizes and compute |

Results:

| Scenario | Outcome |
|---|---|
| Alice $1,500 at $150 | 10 SOL bought, held and short, NAV $1,498.73 (her own $1.27 spread and fee) |
| SOL to $180, Bob $1,800, SOL to $120 | NAV flat; Alice out with $1,497.45 USDC (12.48 SOL sold); Bob out with $1,797.70 USDC (7.52 SOL sold, plus the short's profit) |
| SOL to $200, Alice withdraws all | the vault owes USDC: $1,328.79 now, the rest after the keeper settles the debt: $1,497.02 in all |
| Carol $35,000 | 5 ETH held, 5 ETH short, $17,500 margin; ETH +30%: margin 54% → keeper restores 1x → Carol out with $34,965.81 USDC |
| One day at a 1% perp premium (SOL vault, $15k) | +$45 funding, share price +0.30%; reinvested into 0.298 SOL |
| Three days at 1% premium (ETH vault, $70k) | +$360 funding, reinvested into 0.102 ETH |
| A day of negative funding | share price −0.33%: the strategy's risk |
| $7,000 USDC into the ETH vault (Jupiter) | $3,500 → 1.28537734 ETH, exactly the quote; vault holds and shorts what arrived; $6,982.86 of value after the short, 6,982.86 shares |
| $20,000, then $10,000 into the BTC vault (Jupiter) | $10,000 → 0.11619306 cbBTC; held = short, margin 100%, same share price for both |
| $1,000 USDC into the SOL vault (Jupiter) | all of it → 8.30811436 SOL, unwrapped into Phoenix collateral; short 8.31 SOL; no USDC margin |
| Carol out of the ETH vault in two halves (Jupiter) | Jupiter sold exactly her 0.64268867 ETH each time; $6,990.59 USDC back of $7,000 after two swaps each way and two trades |
| Alice takes half of $1,500 out of the SOL vault (Jupiter) | Jupiter sold exactly 6.2205 SOL; $748.26 USDC; no SOL comes back, only the fee is spent |
| $10,000 USDC into the TSLA vault and out again (Jupiter, Raydium) | $5,000 → 13.137694 TSLAx; short 13.138 TSLA; out: $9,981.52 USDC (0.18% for two swaps and two trades), no TSLAx left over |
| $15,000 into the META vault | $7,500 → 10.025 METAx, short 10.054 META (multiplier 1.0028515); META +10%: NAV $14,993.56 → $14,993.53; out: $14,986.52 USDC |
| TSLA vault at launch caps | refused while it would be all of the shorts; after others short 1,200 TSLA: $114,600 at once refused, three $91,680 deposits (9.1%, 16.7%, 23.1% of open interest), a fourth refused (28.6%) |
| Routes (Jupiter) | a route built for a third of the plan is resized and buys with exactly the plan; output paid elsewhere, the vault's own ETH sold, another venue, another Jupiter instruction, Phoenix marking ETH 2.5% under Jupiter's price: all refused |

| Operation (real Jupiter) | Accounts | Legacy tx | With a lookup table | Compute |
|---|---|---|---|---|
| SOL deposit (Whirlpool) | 46 | 1,712 bytes (too big) | 415 bytes | 331k CU |
| SOL withdraw (Whirlpool) | 46 | 1,713 bytes (too big) | 416 bytes | 345k CU |
| ETH deposit (Whirlpool) | 46 | 1,712 bytes (too big) | 415 bytes | 318k CU |
| ETH withdraw (Whirlpool) | 46 | 1,713 bytes (too big) | 416 bytes | 332k CU |
| TSLA deposit (Raydium CLMM) | 50 | 1,846 bytes (too big) | 425 bytes | 330k CU |
| TSLA withdraw (Raydium CLMM) | 50 | 1,847 bytes (too big) | 426 bytes | 359k CU |

Every deposit and withdrawal needs an address lookup table. A Jupiter route adds its own
accounts (25 for a direct Whirlpool route, 32 for Raydium CLMM; multi-hop routes more) and
compute: a transaction stays under Solana's 64-account and 1.4M CU limits if the route is asked
for with `maxAccounts` around 25.

## Build and test

```bash
# Program and the tests' mock Jupiter (platform-tools v1.52)
SBF=~/.local/share/solana/install/releases/3.1.10/solana-release/bin/cargo-build-sbf
$SBF --manifest-path programs/vanna_phoenix_vault/Cargo.toml --sbf-out-dir target/deploy
$SBF --manifest-path programs/vanna_phoenix_vault/tests/mock_jupiter/Cargo.toml --sbf-out-dir target/deploy

# Jupiter, Orca and Raydium programs for the swap tests (once). Without the flag the script also takes
# a fresh snapshot of the routes and accounts.
python3 scripts/jupiter_fork.py --programs-only

# Tests (host toolchain 1.98.1 from rust-toolchain.toml). The first run downloads the mainnet
# Phoenix, Ember and Hawkeye programs and caches them under target/deploy/.cache.
cargo test -p vanna_phoenix_vault -- --nocapture
```


## Not done yet

- Phoenix onboarding approval for the vault traders on mainnet.
- Open audit items (AUDIT.md): partial payouts when Phoenix's withdraw throttle or cooldown
  holds USDC back (M-10), a two-step admin transfer (L-07), recovering stray token balances
  (L-09).
- `SwapNative` with a Jupiter route for SOL-vault reinvesting without a keeper counterparty.
- More than one trader account per SOL vault, past Phoenix's 500 SOL cap.
- The keeper's funding guard itself (watching 7-day funding and calling `set_deposits_open`).
- Stock vaults: an xStock transfer hook, if Backed ever sets one (transfers would need its extra
  accounts); deposits close after hours but nothing in the program follows Phoenix's holiday
  calendar beyond its flag.
- The app: fetching Jupiter routes for the vault authority (the steps above) and sending v0
  transactions with lookup tables.
- Rebalancing through a Jupiter route instead of the keeper's own inventory.
- An exit that does not need Jupiter: with USDC-only payouts, holders cannot leave while Jupiter
  is down or swaps are turned off (an in-kind fallback for that case is not built).
- An audit.
