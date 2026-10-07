# Vanna basis vaults on Solana (Phoenix)

`vanna_phoenix_vault` is an Anchor program with **one vault per asset** (SOL, ETH, BTC, and
tokenized stocks: TSLA, META). Each vault holds the asset and a short of the same size on
**Phoenix perpetuals**, so its value stays flat in USD while it earns the funding that longs
pay shorts. There are no fees, no stablecoin, and nothing is shared between vaults. Every vault
has **caps** (below) that keep it a minority of its market's shorts.

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
| Margin | the SOL itself (80% counted) | USDC on Phoenix, 1x by default |
| Payout | SOL first; USDC only when SOL runs out | the leaver's part of the tokens, the rest in USDC |

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
- **One deposit** is sized to what the Phoenix book (and, for single-asset deposits, Jupiter)
  takes near the mark; bigger amounts come in several transactions. META's xStock pools are
  the thinnest: a $50k USDC deposit swaps $25k into METAx, between Jupiter's +0.53% at $10k
  and +0.90% at $50k.
- **ETH funding** was negative over the last 7 days and negative over 14% of 3-day windows in
  the last 30: the funding guard keeps ETH deposits closed until it recovers.

## Single-asset deposits and withdrawals

A user brings **one asset**, USDC or the vault's asset. The vault plans the swap to its target
mix (`target_margin_bps` of USDC per dollar of asset), the app builds a Jupiter route for
exactly that amount with the vault authority as the trader, and the vault runs the route,
checks the fill against Phoenix's mark price, and opens the short on what it ends up holding.

| User brings ($35,000 of value, ETH $3,500) | Vault swaps | Vault ends with |
|---|---|---|
| ETH vault: $35,000 USDC | $17,500 USDC → ~5 ETH | ~5 ETH held, ~5 ETH short, $17,500 margin |
| ETH vault: 10 ETH | 5 ETH → ~$17,480 USDC | 5 ETH held, 5 ETH short, ~$17,480 margin |
| BTC vault: USDC or BTC | the same, half and half | BTC held = BTC short, 1x margin |
| SOL vault: USDC | **all** of it → SOL | SOL held = SOL short, no USDC needed |
| SOL vault: SOL | nothing | SOL held = SOL short |

Why not keep the SOL vault's USDC as Phoenix collateral and just short? Phoenix does take
USDC, but USDC plus a short is a naked short: it loses when SOL rises. Delta neutral needs the
SOL itself, and since SOL is its own margin on Phoenix, all of the USDC buys SOL and all of it
earns funding.

`withdraw` takes `receive`: `Both` (no swap), `Asset` or `Usdc`. For a single asset the vault
swaps the other part through the route; whatever the route leaves unsold (it is built a little
under the estimate) is paid as it is.

Every swap goes through `Config.swap_program`, signed by the vault authority. It can only be
Jupiter v6 (or the default key, which turns swaps off): the authority is the share mint's mint
authority and the Phoenix trader, so no other program gets its signature. After the route the
vault checks that:
- the side it sold went down by between 99% and 100% of the planned amount (`SwapAmountOff`),
- the side it bought went up by at least the mark-price value less `slippage_bps` (`SwapPriceTooLow`),
- its Phoenix tokens, the authority's lamports, the SOL in its trader account and the share
  supply did not move, the share mint authority is unchanged, and its token accounts are still
  the authority's with no delegate or close authority (`SwapMovedVaultFunds`).

Native SOL goes through wrapped SOL, as Jupiter does: the vault opens its wrapped-SOL account,
swaps, closes it (the user gets the account's rent back) and posts or pays the SOL.

A deposit without a route must already be at the vault's mix, within 5% of its value (asset
and USDC together; SOL alone for the SOL vault), and must leave the vault at least `margin_bps`
of margin. Off the mix, the vault would trade its hedged asset at Phoenix's mark rather than at
spot; USDC alone or the asset alone goes through a Jupiter route.

### Jupiter

On mainnet `Config.swap_program` is Jupiter v6, `JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4`.
The vault books **what the route actually delivered**, never the quote: the asset it holds is
its token balance after the route, the short is sized on that balance, the margin is the USDC
that arrived, and shares are priced on the NAV after the swap and the short. Jupiter's fee and
price impact land on the depositor, like the Phoenix trade.

The app builds the route like this:

1. The amount: what the vault will sell. USDC alone into a spot vault sells
   `usdc × 10,000 / (10,000 + target_margin_bps)` (half at 1x); the asset alone sells
   `asset × target_margin_bps / (10,000 + target_margin_bps)`; USDC into the SOL vault sells all
   of it (`swap::plan_deposit_swap`).
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
floor), `target_margin_bps` (USDC a single-asset deposit sets aside per dollar of asset: 10,000
for ETH, BTC and stocks, 0 for SOL), `deposits_open`, `after_hours_deposits`.

## Instructions

| Instruction | Who | What |
|---|---|---|
| `initialize` | upgrade authority | Config; only the program's upgrade authority can call it |
| `set_config` | admin | Admin, keeper, swap venue, pause |
| `create_vault(kind, market, params, authority_nonce)` | admin | Vault, share mint, token accounts, registers the Phoenix trader. The market must match Phoenix's listing; the authority's address takes a random nonce, so nobody can register its trader first |
| `update_vault` | admin | Parameters and caps |
| `set_deposits_open(open)` | keeper or admin | The funding guard: closes or reopens a vault's deposits |
| `deposit(asset, usdc, min_shares, route)` | user | Brings the asset and/or USDC, swaps to the target mix if routed, trades the short to match, mints shares |
| `withdraw(shares, min_asset, min_usdc, receive, route)` | user | Shrinks the short, pays out (in one asset if routed), burns shares |
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

Every swap in the tests is a **real Jupiter v6 route over a real pool** (Orca Whirlpool for
SOL, ETH and BTC; Raydium CLMM, where xStocks trade deepest, for TSLAx and METAx): the
tests load the Jupiter, Whirlpool and Raydium programs and every account the routes touch, snapshotted
from mainnet by `scripts/jupiter_fork.py` (routes and accounts in
`tests/fixtures/jupiter_routes.json`, programs cached under `target/deploy/.cache/mainnet`),
with the clock at the snapshot's time and Phoenix's marks between the routes' buy and sell
prices. There is one route each way per vault; a test resizes it to the exact amount the vault
plans by rewriting the input amount and quote at the end of the route's data, as Jupiter's API
would return for that amount. The routes are pinned to one AMM each: Jupiter's best routes
often go through proprietary market makers whose quotes expire within slots, which a snapshot
cannot replay. On mainnet the vault takes whatever route Jupiter gives.

| File | Covers |
|---|---|
| `sol_vault.rs` | 10 SOL → 10 SOL short; price moves; fair entry and exit for two users; partial withdrawal; cap and price band; USDC debt after a rally |
| `spot_vaults.rs` | ETH 50/50 deposit; ETH alone refused; rally + keeper margin top-up + exit; BTC two users and a drop; deposit cooldown |
| `reinvest.rs` | Funding accrues and is reinvested (SOL and ETH); negative funding; keeper role and price band |
| `single_asset.rs` | Through Jupiter: USDC-only and asset-only deposits for ETH, BTC and SOL; USDC-only and ETH-only payouts (ETH and SOL vaults); bad routes refused |
| `stock_vaults.rs` | TSLA: $10,000 USDC in and out through Jupiter (Raydium); META: the multiplier sizes the short, the NAV holds when META moves, METAx paid back; METAx alone sells its margin; after hours; a paused xStock |
| `caps.rs` | TSLA at launch caps: open-interest share, one-deposit size, position cap, withdrawals over the cap; Phoenix's 500 SOL limit and the 450 SOL cap; the keeper's funding guard |
| `audit.rs` | One test per audit finding (AUDIT.md): Jupiter-only venue, keeper limits, upgrade-authority `initialize`, pre-registered traders, market checks, fake Phoenix index accounts, donations and withdrawals, multiplier changes, price-band floor, the deposit mix, insolvent vaults, symmetric share pricing, the first-deposit attack |
| `guards.rs` | Shares are plain tokens (no freeze authority; withdraw in the same slot; transferred shares redeemed by their new holder); wrong market, fake Phoenix accounts, closed deposits, transaction sizes and compute |

Results:

| Scenario | Outcome |
|---|---|
| Alice 10 SOL at $150 | 10 SOL held, 10 SOL short, NAV $1,498.73 (her own $1.27 spread and fee) |
| SOL to $180, Bob 10 SOL, SOL to $120 | NAV flat; Alice out with 12.48 SOL ($1,497.45); Bob out with 7.52 SOL + $895 USDC ($1,797.70) |
| Carol 5 ETH + $17,500 | 5 ETH held, 5 ETH short; ETH +30%: margin 54% → keeper restores 1x → Carol out with $34,966 |
| One day at a 1% perp premium (SOL vault, $15k) | +$46 funding, share price +0.31%; reinvested into 0.305 SOL |
| Three days at 1% premium (ETH vault, $70k) | +$360 funding, reinvested into 0.102 ETH |
| A day of negative funding | share price −0.33%: the strategy's risk |
| $7,000 USDC into the ETH vault (Jupiter) | $3,500 → 1.28503511 ETH (quote 1.28506688); vault holds and shorts what arrived; $6,982.84 of value after the short, 6,982.84 shares |
| 2 ETH into the ETH vault (Jupiter) | 1 ETH → $2,701.65 USDC margin; 1 ETH held and short |
| $20,000 USDC, then 0.2 BTC into the BTC vault (Jupiter) | $10,000 → 0.11599636 cbBTC; 0.1 BTC → $8,614.19; held = short, margin 99.9%, same share price for both |
| $1,000 USDC into the SOL vault (Jupiter) | all of it → 8.31469940 SOL, unwrapped into Phoenix collateral; short 8.31 SOL; no USDC margin |
| Carol out of the ETH vault, half in USDC, half in ETH (Jupiter) | $2,692.24 USDC, then 0.998457 ETH + $6.94: $5,408.28 of her $5,425.22 after four swaps and two trades |
| Alice takes half of 10 SOL in USDC (Jupiter) | Jupiter sold 4.9708 SOL for $597.56; 0.0207 SOL the route left unsold came back as SOL |
| $10,000 USDC into the TSLA vault and out again in USDC (Jupiter, Raydium) | $5,000 → 13.046671 TSLAx, exactly the quote; short 13.047 TSLA; out: $9,979.06 + 0.0065 TSLAx = $9,981.56 (0.18% for two swaps and two trades) |
| 10 METAx + $7,500 into the META vault | short 10.029 META (multiplier 1.0028515); META +10%: NAV $14,974.84 → $14,974.81; out: 10 METAx + $6,744.43 = $14,967.82 |
| TSLA vault at launch caps | refused while it would be all of the shorts; after others short 1,200 TSLA: $114,600 at once refused, three $91,700 deposits (9.1%, 16.7%, 23.1% of open interest), a fourth refused (28.6%) |
| Bad routes (Jupiter) | a third of the plan, output paid elsewhere, the vault's own ETH sold, another venue, a needless route, Phoenix marking ETH 2.5% under Jupiter's price: all refused |

| Operation | Accounts | Legacy tx | With a lookup table | Compute |
|---|---|---|---|---|
| SOL deposit | 31 | 1,175 bytes | 343 bytes | 241k CU |
| SOL withdraw | 32 | 1,208 bytes | 345 bytes | 246k CU |
| ETH deposit (ETH + USDC) | 34 | 1,271 bytes (too big) | 346 bytes | 246k CU |
| ETH withdraw | 34 | 1,272 bytes (too big) | 347 bytes | 272k CU |
| ETH deposit, USDC only (Jupiter + Whirlpool) | 47 | 1,754 bytes (too big) | 426 bytes | 319k CU |
| SOL deposit, USDC only (Jupiter + Whirlpool) | 46 | 1,722 bytes (too big) | 425 bytes | 336k CU |
| TSLA deposit (TSLAx + USDC) | 35 | 1,303 bytes (too big) | 347 bytes | 255k CU |
| TSLA deposit, USDC only (Jupiter + Raydium CLMM) | 51 | 1,888 bytes (too big) | 436 bytes | 331k CU |

Everything but plain SOL deposits needs an address lookup table. A Jupiter route adds its own
accounts (25 for a direct Whirlpool route, 32 for Raydium CLMM; multi-hop routes more) and compute: a routed
deposit stays under Solana's 64-account and 1.4M CU limits if the route is asked for with
`maxAccounts` around 25.

## Build and test

```bash
# Program (platform-tools v1.52)
~/.local/share/solana/install/releases/3.1.10/solana-release/bin/cargo-build-sbf \
  --manifest-path programs/vanna_phoenix_vault/Cargo.toml --sbf-out-dir target/deploy

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
- An audit.
