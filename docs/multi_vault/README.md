# Vanna multi-strategy basis vault on Solana (Phoenix)

`vanna_phoenix_multi_vault` is an Anchor program: **one USDC vault spread over many Phoenix
perp markets**. Users deposit and withdraw USDC only and hold the vault's shares. A keeper watches
every market's funding and open interest and moves the vault's USDC to where shorts earn the
most: each **leg** holds an asset and shorts the same amount on Phoenix, so the vault stays flat
in USD while it earns the funding longs pay shorts.

It is built from the single-asset vault in this repository ([`programs/vanna_phoenix_vault`](../../README.md)),
and its **accounting and shares are that vault's, unchanged**:

```
NAV          = idle USDC + Σ legs (asset held × Phoenix mark × token multiplier) + Phoenix equity
share price  = NAV / supply, with the virtual offset (1e6 shares and $1)
deposit      → shares = value × (supply + 1e6) / (NAV + 1e6)
withdrawal   → payout = NAV − ((NAV + 1e6) × (supply − shares + 1e6) / (supply + 1e6) − 1e6)
```

Phoenix equity is the USDC collateral, unrealized PnL and unsettled funding of **one cross-margin
trader account that holds every leg's short**. A deposit and a withdrawal at once move no value,
even in a nearly empty vault.

## Docs

| Doc | For |
|---|---|
| [FRONTEND_INTEGRATION.md](FRONTEND_INTEGRATION.md) | The web app: accounts, valuation, every user instruction with its accounts and code, previews, yield, states, events, errors |
| [KEEPER_OPERATIONS.md](KEEPER_OPERATIONS.md) | Admin setup (initialize, vault, legs, lookup table) and the keeper bot: what it calls and when, config, running, alerts, runbook |
| [AUDIT.md](AUDIT.md) | The 2026-10-16 audit of the program and the bot |
| [client/src/idl/vanna_phoenix_multi_vault.json](../../client/src/idl/vanna_phoenix_multi_vault.json) | The program's IDL and TypeScript types (`anchor idl build`) |

## How it differs from the single-asset vault

| | Single-asset vault | Multi-strategy vault |
|---|---|---|
| Markets | one per vault | up to 8 live legs in one vault |
| Deposit | buys the asset and opens the short in the user's transaction | lands in a USDC buffer; the keeper deploys it later |
| Withdrawal | closes the user's part of the short and sells it in their transaction | paid from the buffer at once, or through a request the keeper raises USDC for |
| Who trades | each user, for their own deposit | the keeper, for the whole vault |
| Trading costs | the user who causes them | the vault; `entry_fee_bps` and `exit_fee_bps` (kept in the vault) can charge them back |
| Margin | per vault | shared: SOL collateral and USDC margin cover every leg's short |

A user's transaction can't trade every leg: each leg's swap is a Jupiter route of about 25
accounts, and Solana allows 64. So users only move USDC, and the keeper trades one leg per
transaction.

## Legs

| Kind | Asset | Where it sits | Margin |
|---|---|---|---|
| `NativeSol` | SOL | the vault's Phoenix trader account, as native collateral | the SOL itself (80% counted, up to 500 SOL per trader) |
| `Spot` | any token with a Phoenix perp: ETH, BTC, xStocks (Token-2022), … | the authority's token account | USDC the keeper posts on Phoenix (`move_margin`) |

The admin adds legs with `add_leg`. Each leg's market is checked against Phoenix's perp asset map
(order book, spline, tick size, lot size), and no two live legs may trade one market or hold one
asset. A leg can be deactivated (`active = false`: no more deploys, and unwinds still work).
Once it is flat, the admin can retire it with `retire_leg(max_units)`. Flat means nothing
short, at most `max_units` held, and no more than $100 of dust at the mark. A retired leg takes
one account in every leg list and no Phoenix call, so its market being delisted or halted, or its
token breaking, can no longer stop deposits and withdrawals. Retiring reads only that leg, so it
still works after the break, which stops every other instruction:

- A leg that no longer prices (multiplier unreadable, mark at zero) must be deactivated first.
- A leg whose market Phoenix removed from its asset map holds no position there.

`add_leg` refuses a token whose multiplier the vault can't read. At most 8 legs can be live, and 16 can ever be added. Each leg has its own caps:
short notional, asset held, share of the market's open interest and share of the vault's NAV.

**Every instruction that values the vault passes every leg**, in order, after Phoenix's index
accounts: the leg, then its asset account (spot legs), then its mint (Token-2022 legs, for the
dividend multiplier). Leaving out or reordering one is refused (`InvalidLegAccounts`): a NAV
without a leg would misprice every share.

## Users

| Instruction | What it does |
|---|---|
| `deposit(usdc, min_shares)` | USDC into the buffer; shares at the NAV before it, less `entry_fee_bps` |
| `withdraw(shares, min_usdc)` | Burns shares and pays their NAV less `exit_fee_bps` from the buffer, but only from USDC above what live requests are worth: requests come first |
| `request_withdraw(shares)` | Shares into escrow when the buffer can't pay; they keep earning with everyone's until fulfilled. A request has no minimum: one it could never meet would hold the buffer forever |
| `fulfill_withdraw(request)` | Pays the request's user at the NAV then, into their own USDC account (never created at the cranker's cost), burns the escrowed shares, closes the request. **The keeper** may pay any request, oldest first. **Anyone else**, the user included, may pay one only from the buffer above what every other live request is worth, so no request can jump the queue. During a multiplier window, only the user or the keeper |
| `return_withdraw(request)` | The keeper gives up on a request the program can't pay: the user's USDC account is closed, given away or frozen, or the payout is nothing. No token moves (a user could block that too). The shares stay in escrow and stop reserving the buffer. The queue moves on |
| `cancel_withdraw(request)` | The user takes their shares back, less `exit_fee_bps` (burned, so the value stays with the holders): the keeper may already have traded to raise the USDC. A returned request comes back whole |

Only shares in live requests are reserved (`Vault::pending_shares`): shares someone sends
straight to the escrow are not a request and hold nothing.

**Stock splits:** while a tokenized stock's multiplier is changing by more than 0.25% (from a
day before to three days after), Phoenix's market and the token may disagree on what one token
is worth. The vault stays open but prices that token for the holders who stay:

- **Deposits** count it at the higher of the two multipliers.
- **Withdrawals** count it at the lower.
- **The keeper** may not deploy into it.

A dust donation can't hold the vault shut, and whoever acts inside the window carries the
uncertainty.

Deposits are refused while the vault owes more than it holds, past `max_deposit_usd` or
`max_tvl_usd`, while closed (`set_deposits_open`, the keeper's funding guard) or paused.
Withdrawals stay open while paused.

## The keeper

Off-chain, the keeper decides where the USDC goes from Phoenix's funding rates and open interest
(`GET /v1/funding/{symbol}/rates`, `/v1/view/market/{symbol}`). On-chain, every move it makes is
held to the vault's limits:

| Instruction | What it does | Held to |
|---|---|---|
| `deploy(leg, usdc, route)` | Buys the leg's asset with buffer USDC through Jupiter, then shorts what arrived. SOL goes into Phoenix as collateral | `buffer_bps` of NAV plus pending requests stay in the buffer; margin ≥ `margin_bps` of every short together; the leg's caps; the market not after hours (unless allowed); no multiplier change in progress |
| `unwind(leg, amount, route)` | Closes the same share of the leg's short, sells `amount` of its asset to buffer USDC. SOL leaves Phoenix first | margin not thinner than before, or at target; in a multiplier window the sale must fetch (and counts against turnover at) the higher multiplier's value |
| `move_margin(to_phoenix, amount)` | USDC between the buffer and Phoenix collateral | in: the buffer reserve. Exception: the reserve may cover what the vault owes Phoenix plus 0.5% of its shorts (closing fees), because Phoenix holds native SOL until that is paid. Out: the margin floor |
| `rehedge(leg)` | Brings the short back to the asset after a liquidation or a donation | margin not thinner than before, or at target; not in a multiplier window |
| `set_deposits_open(open)` | The funding guard | |

Every swap is a Jupiter v6 route that the vault resizes to the exact amount
(`swap::resize_route`). It must fill within `slippage_bps` of Phoenix's mark (`check_fill`), and
it may move nothing but its two sides: not another leg's asset, the escrowed shares, the PhUSD
account, the authority's lamports, the trader account's SOL, or the share supply and mint
authority. Every order fills within `slippage_bps` of the mark. Deploys and unwinds together are
limited to `max_turnover_usd` a day.

**Bound:** a stolen keeper key can't take value, but it can trade against its own orders.
Per dollar of turnover it can make the vault lose up to twice the band (a swap and an order, each
filled at the edge of `slippage_bps`). The band is capped at 2%. The turnover window is fixed, so
up to twice `max_turnover_usd` can pass at a window boundary. Keep the band near 0.2–0.5% and the
turnover to what allocation needs. The key can also stop deploying, or allocate badly, which costs
the vault funding it could have earned.

**Fees:** deposits and withdrawals don't trade, so with both fees at 0 a round trip is free.
Someone could then step around the keeper's rotation costs or a small, known multiplier step.
Set `entry_fee_bps` and `exit_fee_bps` to about what deploying and unwinding cost (5–10 bps
each).

### A keeper policy that pays

From the Phoenix scan behind this design (2026-10-07: 94 markets, $25.9M total open interest):

- **Only hedgeable markets:** a leg needs a Solana token with Jupiter liquidity (SOL, cbBTC,
  Wormhole ETH, xStocks, Solana tokens such as JTO). A short with no spot behind it isn't
  neutral.
- **Size by open interest:** stay under ~25% of a market's shorts. Above that, the vault pushes
  funding down on itself. Today that's about SOL $2.1M, BTC $0.85M, ETH $0.32M, TSLA $0.29M and
  META $0.20M.
- **Rotate rarely:** a move between legs costs ~0.2–0.6% (two Phoenix taker fees plus Jupiter on
  both legs). A 10% funding gap pays that back in 1–3 weeks, so act on 7-day and 30-day averages
  with hysteresis, not on the hourly rate.
- **Keep the buffer:** keep the buffer near expected withdrawals. Unwind for requests oldest
  first, and fulfill them.
- **SOL:** the SOL leg earns funding on its whole value (it is its own margin). Spot legs at 1x
  earn on half of theirs.

### The keeper bot (`keeper`)

A deterministic Rust bot that applies the policy above. It doesn't use an LLM. Every 30 seconds it
reads the vault with the program's own valuation code (`nav::Snapshot`, Hawkeye's views and the
perp asset map) and refreshes funding every 15 minutes. It then does the most urgent move, reads
the vault again, and repeats. It stops after 8 moves or when nothing is left to do:

1. **Margin alarm:** margin within 10% of `margin_bps`. The keeper posts free buffer, or sells
   part of the largest spot leg.
2. **Re-hedge** a leg whose short is off its asset by more than 0.5% (liquidation, donation,
   dividend multiplier).
3. **Withdrawal requests,** oldest first: fulfill from the buffer. Otherwise take margin above
   target back from Phoenix, then sell the lowest-paying leg.
4. **Buffer refill** after instant withdrawals: under the reserve, margin over target comes back
   from Phoenix. Under half the reserve, the keeper also sells the lowest-paying leg.
5. **Margin top-up** from the free buffer when margin drifts 10% under target.
6. **Funding guard:** close deposits when no active leg pays 3% on capital, and reopen at 6%.
7. **Exits:** a leg paying under 2%, or one the admin deactivated, is unwound to USDC in the
   vault. A leg above 35% of its market's open interest is trimmed back to 25%.
8. **Deploy** free buffer (above `buffer_bps` and $1,000 of slack) into the best-paying leg with
   room. The keeper posts margin first.
9. **Rotate** the lowest-paying held leg into one paying at least 8% more on capital, sized to
   the new leg's room including its margin.
10. **Margin release** once Phoenix holds 20% over target.

Rates are **capital APRs**: the mean of 7- and 30-day funding over the capital a leg takes. A spot
leg at 1x needs its asset plus equal USDC margin. The SOL leg is its own margin. All thresholds
are in `keeper.example.toml`.

When nothing matches, the plan is empty and the keeper sends nothing. Legs keep earning, idle
USDC stays in the vault, and no one's funds are moved out. Holders can withdraw at any time.
Unwound legs become USDC in the vault, not payouts.

A move that fails is retried after 10 minutes, and an alert goes to the webhook. Each move is
checked against the program's own limits before it is sent:

- every deploy and unwind is capped by the turnover left;
- every Jupiter quote is checked against `check_fill`'s band;
- a SOL exit after a rally first covers the short's USDC deficit on Phoenix.

```bash
cargo run -p vanna_keeper --bin vanna-keeper -- plan --config keeper.toml   # print the plan, send nothing
cargo run -p vanna_keeper --bin vanna-keeper -- once --config keeper.toml   # one tick
cargo run -p vanna_keeper --bin vanna-keeper -- run  --config keeper.toml   # forever
```

The tests run the same keeper, through a LiteSVM backend, against the real Phoenix programs and
this vault (`keeper/tests`):

- it reads the vault exactly as the program does;
- it deploys $100,000 into the best legs, all hedged;
- when nothing pays, it keeps the USDC and closes deposits, while Alice can still withdraw all of
  hers;
- it unwinds a leg whose funding turns negative and keeps the USDC in the vault;
- it raises USDC for a request and pays it;
- it restores the margin after a 40% rally;
- it takes the SOL leg out after a 30% rally;
- it re-hedges a donation;
- it rotates only past the gap;
- it stays within the daily turnover;
- a dry run sends nothing.

How money moves between legs and out to users is covered in `tests/flows.rs`:

| Scenario | What the keeper did |
|---|---|
| Funding leadership moves SOL → ETH → TSLA ($60k) | $39,600 of SOL into ETH's $20k of room. Then all of SOL (25%, not ETH at 45%) into TSLA's $10k. NAV −0.21% |
| A new best market (META 60%) beside SOL 25%, ETH 20% and TSLA 15% | Sold all of TSLA, then $5,132 of ETH. META $19,939. SOL untouched |
| Bob deposits $20k just as ETH becomes the best leg | His USDC filled ETH first. Only $19,850 of SOL was sold, not $39,600 |
| Bob ($30k) then Alice ($20k) request | Bob paid $29,979.77 from TSLA and its margin. Alice paid $19,984.10 from ETH. SOL untouched |
| 10% buffer: Bob takes $8k instantly | Paid with no trade. The keeper sold $3,638 of ETH and took $3,556 of margin back: buffer $10,194 |
| Bob, Carol and Dave request; Dave cancels | Bob paid, then Carol. Dave skipped, shares his again |
| Alice in through a rotation, Bob after it; both leave | Alice $59,848.76 of $60k (paid for the rotation), Bob $39,949.38 of $40k. The vault ends at $0.00 |
| Every leg's funding turns negative, then returns | All three legs to USDC, $34,561 of margin back, deposits closed. Later reopened and redeployed. The round trip cost $110 (0.11%) |
| ETH open interest 8 → 28 → 10 ETH (default 25% share) | ETH leg $8.7k → $32.3k. Trimmed $15.4k when its share hit 48% |
| TSLA pays most but is after hours | Funds went to SOL. When the session opened, $39,600 of SOL went into TSLA |

The live backend (Solana RPC, Phoenix's API, Jupiter's `/quote` and `/swap-instructions`) needs a
deployed vault to run against. Its Jupiter path hasn't run against mainnet yet.

## Phoenix and Solana limits

- **Native SOL: 500 SOL counted per trader account** (cap the SOL leg at 450 SOL with
  `max_asset_units`). SOL beyond it adds no margin.
- **Shared margin:** a rally in one leg is paid from margin that backs every leg. The keeper
  restores it (`unwind` the rallied asset, `move_margin` in): ETH +40% took margin from 99.9% to
  66.6% and back in `a_rally_thins_the_shared_margin_and_the_keeper_restores_it`.
- **Accounts:** with all eight legs, a deposit names 41 accounts and a deploy 47 (with the tests'
  10-account mock route). A direct Jupiter route is about 15 accounts more, so 62 of Solana's 64:
  eight live legs is the ceiling (`a_vault_with_all_eight_legs_fits_solanas_limits`). The keeper
  asks Jupiter for routes with `maxAccounts` set to what is left, and multi-hop routes may need
  fewer live legs. Every vault transaction needs an address lookup table.
- **Phoenix onboarding is permissioned:** `create_vault` registers the trader, and Phoenix grants
  it trading.

## Pieces

| Account | What it is |
|---|---|
| `Config` | Admin, keeper, swap venue (Jupiter or off), pause, USDC and PhUSD mints, Phoenix's perp asset map, withdraw queue and token vault |
| `Vault` (`["vault", id]`) | Share mint, authority, buffer, escrow, trader, leg count, caps, fees, buffer, margin, band, turnover window |
| `Leg` (`["leg", vault, index]`) | Kind, asset, market, caps, active |
| `WithdrawRequest` (`["withdraw_request", vault, id]`) | User, shares, minimum, time |
| Authority (`["authority", vault, nonce]`) | Data-less PDA: the Phoenix trader, owner of every token account, the share mint's authority. The admin's random nonce stops anyone registering its Phoenix trader first |

## Tests

The integration tests run the **real mainnet Phoenix, Ember and Hawkeye programs** in Phoenix's
LiteSVM fixture (`phoenix-rise-litesvm-test`), with mainnet's native SOL limits. The vault has
five legs: SOL (native), Wormhole ETH, cbBTC, TSLAx and METAx (the real Token-2022 mints, with
their extensions and multiplier). Swaps go through a mock Jupiter
([`programs/vanna_phoenix_vault/tests/mock_jupiter`](../../programs/vanna_phoenix_vault/tests/mock_jupiter), shared with the single-asset vault) at Jupiter's
address. It takes the amount the vault resized the route to and pays the route's quote, which the
tests set at Phoenix's mark, so prices can move freely.

| File | Covers |
|---|---|
| `vault.rs` | First deposit at $1 a share; the NAV across five legs through price moves, a second depositor and funding, and both leaving with their part; buffer vs requests (reserved, fulfilled by anyone, cancelled by the user or returned by the keeper); shares sent to the escrow reserving nothing; fees staying in the vault; caps, closed deposits and pause; every leg must be passed; a round trip in a tiny vault; an insolvent vault; shares as plain tokens |
| `strategy.rs` | Five hedged legs; rotating ETH into TSLA as funding moves; funding raising the share price and being reinvested; negative funding; a rally thinning the shared margin and the keeper restoring it; the SOL leg leaving Phoenix; re-hedging a donation; transaction sizes and compute; all eight legs (three more stock markets added to the fixture) within Solana's limits |
| `guards.rs` | Keeper-only moves; inactive legs; the buffer and pending reserve; the margin floor; every leg cap (notional, asset, NAV share, open-interest share); the turnover limit; swaps held to the mark and refused when they touch another leg's asset or the escrow; non-Jupiter routes; swaps off; after hours; a split priced for those who stay with the vault open; a dust donation that can't hold the vault; retiring a flat leg so a broken token no longer matters; pause; legs checked against Phoenix and each other; upgrade-authority `initialize` and parameter bounds |

Results:

| Scenario | Outcome |
|---|---|
| $200,000; keeper deploys SOL 20%, ETH 30%, BTC 20%, TSLA 10%, META 10% | every leg hedged to the lot; costs $93.81 (0.052% of $180,000); margin 92.6% |
| SOL +20%, ETH −14%, BTC +10%, TSLA −8%, META +7% | NAV $99,957.79 → $99,957.72 |
| Bob $50,000 at the same share price, a day of funding on SOL and TSLA, both leave | Alice $99,865.46 of $100,000, Bob $49,953.85 of $50,000, each exactly their shares × the exit share price; nothing left |
| $10,000 buffer, Bob asks for $40,000 | refused at once; requested; buffer reserved for him; keeper unwinds half of every leg; Carol fulfills: Bob gets $39,974.26 |
| Entry 0.2%, exit 0.3% | Bob in and out loses $39.98; Alice's share price 1.002004 → 1.006010 |
| A day of funding at a 1% premium on SOL and TSLA ($80,000 deployed) | share price +0.130%; reinvested into SOL at no NAV change |
| ETH funding negative, TSLA positive | ETH sold ($19,960), TSLA bought with it; ETH's margin backs TSLA's short; NAV change under 0.2% |
| ETH +40% | margin 99.9% → 66.6%, vault flat; keeper sells ETH into margin: 99.9% |

| Operation (5 legs, mock route of 10 accounts) | Accounts | With a lookup table | Compute |
|---|---|---|---|
| deposit | 35 | 336 bytes | 163k CU |
| withdraw | 33 | 332 bytes | 249k CU |
| deploy SOL | 41 | 380 bytes | 480k CU |
| deploy ETH / META | 40 | 379 bytes | 426k / 432k CU |
| unwind SOL | 42 | 382 bytes | 499k CU |

| Operation (8 legs, mock route of 10 accounts) | Accounts | With a lookup table | Compute |
|---|---|---|---|
| deposit | 41 | 348 bytes | 215k CU |
| withdraw | 39 | 344 bytes | 424k CU |
| deploy (the eighth leg) | 46–47 | 391–392 bytes | 379k–616k CU |
| unwind SPY | 46 | 392 bytes | 644k CU |

## Security review (2026-10-07)

An independent review of the program read every instruction against the single-asset vault's
audit. It found no High issue. Its Medium and Low findings and what was done:

| Finding | Severity | Status |
|---|---|---|
| A dust donation of a stock token during a multiplier window froze every deposit and withdrawal | Medium | Fixed: the vault stays open and prices the token low for withdrawals, high for deposits |
| Requests with an unpayable minimum, or shares sent straight to escrow, held the buffer forever | Medium | Fixed: `pending_shares` counter, no request minimum, fulfill creates the user's USDC account, the keeper can return a request it can't pay |
| One leg's market (delisted, halted, broken token) froze every exit; legs were permanent | Medium | Fixed for flat legs: `retire_leg`. A leg still holding assets on a broken market needs the keeper to unwind it before the market goes |
| Eight legs might not fit a transaction with a real route | Medium | Measured: 47 accounts with a 10-account mock route, about 62 with a direct route, 644k CU, heap fine. `maxAccounts` on Jupiter routes |
| The keeper's loss bound is about twice the band, and the band could be 10% | Low | Band capped at 2%; the bound is documented above |
| With fees at 0, round trips are free | Low | Documented: set entry and exit fees near deploy and unwind costs |
| A swap could reach the Phoenix trader account through Phoenix | Info | Fixed: the trader account's data is hashed before and after every swap |

Not changed, as in the single-asset vault: user prices rest on Phoenix checking the trader index
accounts in `sync_native`; the admin transfer is one step; `update_vault` also sets
`deposits_open`; PhUSD or lamports sent to the vault are uncounted.

## Audit (2026-10-16)

A second review covered the program and the keeper bot. Its findings, fixes and regression tests
are in [AUDIT.md](AUDIT.md). There were no Critical issues. Four Medium findings in the program
and three High findings in the bot are fixed:

- a request jumping the queue;
- a request its user could lock forever;
- a broken leg that could not be retired;
- unwinds priced at the low multiplier in a split window;
- the bot's stuck queue;
- its SOL withdrawal deadlock;
- its churn on markets it can't buy.

## Build and test

From the repository root (`basis-trading-solana-phoenix`). The program
(`programs/vanna_phoenix-multi`) and its keeper (`keeper/`) are members of the repository's Cargo
workspace, beside the single-asset vault:

```bash
# Program and the tests' mock Jupiter (platform-tools v1.52)
SBF=~/.local/share/solana/install/releases/3.1.10/solana-release/bin/cargo-build-sbf
$SBF --manifest-path programs/vanna_phoenix-multi/Cargo.toml --sbf-out-dir target/deploy
$SBF --manifest-path programs/vanna_phoenix_vault/tests/mock_jupiter/Cargo.toml --sbf-out-dir target/deploy

# Tests (host toolchain 1.98.1). The first run downloads the mainnet Phoenix, Ember and Hawkeye
# programs into target/deploy/.cache.
cargo test -p vanna_phoenix_multi_vault -- --nocapture
cargo test -p vanna_keeper -- --nocapture

# The same through Anchor's scripts
anchor run test-multi
anchor run test-keeper
```

## Not done yet

- **The keeper on mainnet:** its live backend hasn't run against a deployed vault. Jupiter
  routes and the vault's lookup table need a first `plan` run.
- **Request order among the keeper's payments:** only the keeper may pay a request out of
  order (others must leave every other request's USDC). It pays oldest first, but nothing
  on-chain forces it to.
- **A broken market with holdings:** retiring needs a flat leg, so the keeper has to unwind a leg
  before its market is delisted. A market Phoenix removed from its asset map is assumed to hold
  no position; that path has no test against a real delisting.
- **Native SOL feature dependency:** every instruction calls Phoenix's `sync_native`. If Phoenix
  turned native SOL collateral off, the vault would stop until an upgrade. Checking the index
  accounts against Phoenix's global configuration instead is not built.
- **Exits without the keeper:** if the keeper stops, requests beyond the buffer wait for it. A
  permissionless unwind after a timeout is not built.
- **Real Jupiter tests:** Jupiter routes are built for the vault's authority, so the
  single-asset vault's snapshots don't fit. Re-snapshot them with the repository's
  `scripts/jupiter_fork.py` for this program's authority.
- **TypeScript client package:** the integration guide has the code for every flow
  (`docs/multi_vault/FRONTEND_INTEGRATION.md`), but there is no packaged client with tests yet.
- **Setup scripts:** `initialize`, `create_vault`, `add_leg` and the lookup table are documented
  (`docs/multi_vault/KEEPER_OPERATIONS.md` §4) but not scripted.
- **Phoenix onboarding** of the trader, and an audit.
