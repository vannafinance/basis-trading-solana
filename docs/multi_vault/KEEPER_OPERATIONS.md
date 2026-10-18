# Keeper and operations: Vanna multi-strategy basis vault

This guide is for whoever sets up the vault and runs its keeper bot (`vanna-keeper`). It covers:

- the one-time admin setup;
- building, configuring and running the bot;
- every instruction the bot calls and when;
- what it alerts on;
- the runbook for incidents.

Frontend integration is in [FRONTEND_INTEGRATION.md](FRONTEND_INTEGRATION.md).

## Contents

1. [What the keeper is](#1-what-the-keeper-is)
2. [What the bot calls, and when](#2-what-the-bot-calls-and-when)
3. [How it decides](#3-how-it-decides)
4. [One-time setup (admin)](#4-one-time-setup-admin)
5. [Build and configure the bot](#5-build-and-configure-the-bot)
6. [Run it](#6-run-it)
7. [Alerts and monitoring](#7-alerts-and-monitoring)
8. [Runbook](#8-runbook)
9. [Security](#9-security)
10. [Testing](#10-testing)
11. [Known limits](#11-known-limits)

---

## 1. What the keeper is

The vault program holds the money and enforces the limits; the keeper decides what to do within
them. The keeper is a key set in `Config.keeper` and a Rust bot (`keeper`) that signs
with it. The bot is deterministic: plain rules on the program's own numbers, no AI model.

**Every tick (30 s by default) the bot:**

1. **Reads the vault** the way the program values it:
   - `Config`, `Vault` and every `Leg`;
   - token balances and the share supply;
   - Phoenix's margin and position views (Hawkeye, simulated);
   - the perp asset map (open interest, after-hours flags, market names);
   - the live withdrawal requests and each one's user USDC account.
2. **Refreshes funding** from Phoenix's API every 15 minutes: 7- and 30-day averages per market.
3. **Plans** every move worth making now, most urgent first, with alternatives.
4. **Acts:** sends the first move, re-reads the vault, and repeats, up to 8 moves a tick.
5. **Retries carefully:** a move that fails cools down, twice as long each time it fails in a
   row, and is retried at half the size.

**What the program enforces whatever the bot does:**

- the buffer reserve (`buffer_bps` of NAV plus pending requests);
- the margin floor (`margin_bps`);
- per-leg caps (notional, asset, open-interest share, NAV share);
- a price band of at most 2% on every swap and order (`slippage_bps`);
- a daily turnover limit (`max_turnover_usd`).

**What a stolen keeper key can do:** at worst, lose about twice the band on the day's turnover,
or stop working. It can't withdraw funds.

---

## 2. What the bot calls, and when

| Instruction | Signed by | The bot calls it when | Typical frequency |
|---|---|---|---|
| `deploy(leg, usdc, route)` | keeper | Free buffer (above reserve + $1k slack) and a leg pays at least 6% on capital with room. Buys the asset through Jupiter, then shorts it | After deposits; after rotations |
| `move_margin(to_phoenix, amount)` | keeper | **In:** before a spot deploy (USDC margin for its short), margin top-up, margin alarm, covering Phoenix's USDC deficit before a SOL sale. **Out:** margin over target back to the buffer, raising USDC for withdrawals | Most common: comes with nearly every spot deploy and unwind |
| `unwind(leg, units, route)` | keeper | Raising USDC for requests or the buffer refill; a leg paying under 2%, deactivated, or above 35% of its market's open interest; rotation; margin alarm | Withdrawals, exits, rotations |
| `fulfill_withdraw(request)` | anyone (bot) | The buffer holds the oldest request's payout | Every request |
| `return_withdraw(request)` | keeper | A request the program can't pay: its user's USDC account is closed, frozen or not theirs; it pays nothing; or it failed three times | Rare |
| `rehedge(leg)` | keeper | A leg's short is off its asset by more than 0.5% (liquidation, donation, a dividend multiplier step). Never in a multiplier window | Rare |
| `set_deposits_open(open)` | keeper | Close when no active leg with room pays at least 3% on capital; reopen at 6% | Rare |

**The bot never calls these** (they are the admin's):

- `initialize`, `set_config`
- `create_vault`, `update_vault`
- `add_leg`, `update_leg`, `retire_leg`
- `cancel_withdraw` (the user's)

**Example log of a tick** (from the end-to-end tests):

```
[1792147737] deploy (deploy): SOL pays 25.00%: deploy $29700.00 of $100000.00 free
[1792147737]   sent 2VbnR4sZ…
[1792147737] move_margin (deploy): margin for $19800.00 into ETH (20.00%): post $19825.24
[1792147737] deploy (deploy): ETH pays 20.00%: deploy $19800.00 of $50474.76 free
[1792147747] unwind (requests): request 0 needs $29013.48 more: sell $14656.81 of TSLA
[1792147747] move_margin (requests): request 0 needs $14352.94 more: $14352.94 of margin back from Phoenix
[1792147747] fulfill_withdraw (requests): request 0 pays $29979.77 from the buffer
```

---

## 3. How it decides

Rates are **capital APRs**: the mean of a market's 7- and 30-day funding, divided by the capital a
dollar of short takes.

- A **spot leg** at 1x margin needs its asset plus an equal amount of USDC margin, so it earns its
  funding on half its capital.
- The **SOL leg** is its own collateral, so it earns on all of it.

**Rules, most urgent first** (`src/policy.rs`; thresholds are the `[policy]` settings, §5.2):

1. **Margin alarm:** margin within 10% of `margin_bps`. Post free buffer (or, while the vault owes
   Phoenix, reserved USDC up to the debt). Otherwise sell spot legs, largest first.
2. **Re-hedge:** a short off its asset by more than 0.5%, outside multiplier windows.
3. **Withdrawal requests, oldest first:**
   - Return an unpayable one.
   - Pay the next one if the buffer holds it.
   - Otherwise raise the USDC: margin over target back from Phoenix first, then sell the
     lowest-paying legs one after another. Legs in multiplier windows or with stale funding go
     last, and SOL goes after spot legs while the vault owes Phoenix.
4. **Buffer refill:**
   - Under the reserve, bring back margin over target.
   - Under half the reserve, also sell the lowest-paying leg.
5. **Margin top-up:** from the free buffer when margin is 10% under target.
6. **Deposit guard:** close or reopen deposits, as in §2.
7. **Exits:**
   - A leg under 2% or deactivated is unwound to USDC in the vault.
   - A leg above 35% of its market's open interest is trimmed back to 25%.
   - Legs in a multiplier window wait.
8. **Deploy:** best capital APR first, with margin posted first for spot legs.
   - The size is the largest the free buffer pays for, capped by the leg's caps, 25% of the
     market's open interest, 50% of NAV per leg, 500 SOL for the SOL leg, $50k per trade, and the
     turnover left.
9. **Rotate:** when another leg pays at least 8% more on capital, sell the lowest-paying leg. It
   sells enough for the new leg's room including its margin ($30k of room in a spot leg = $60k of
   SOL). Before selling, it checks that the new market has a Jupiter route.
10. **Margin release:** Phoenix holds 20% over target.

**When nothing applies, the bot does nothing:**
- Legs keep earning.
- USDC stays in the vault.
- Users can still withdraw.

Unwound legs become USDC in the vault, not payouts.

---

## 4. One-time setup (admin)

Run these with the Anchor TypeScript client or a Rust script. There is no setup script in the
repository yet. Account lists are in the IDL ([`client/src/idl/vanna_phoenix_multi_vault.json`](../../client/src/idl/vanna_phoenix_multi_vault.json)).

### 4.1 Deploy the program

Commands in this guide run from the repository root (`basis-trading-solana-phoenix`).

```bash
SBF=~/.local/share/solana/install/releases/3.1.10/solana-release/bin/cargo-build-sbf
$SBF --manifest-path programs/vanna_phoenix-multi/Cargo.toml --sbf-out-dir target/deploy
solana program deploy target/deploy/vanna_phoenix_multi_vault.so \
  --program-id target/deploy/vanna_phoenix_multi_vault-keypair.json   # upgrade authority = your admin key
```

Put the upgrade authority behind a multisig once the vault holds funds.

### 4.2 `initialize(keeper, swap_program)`

- It must be signed by the **program's upgrade authority**, which becomes `Config.admin`.
- `keeper` is the bot's key.
- `swap_program` is Jupiter v6, `JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4`. Pass the default key to turn swaps off.

**Accounts:**
- admin, the program and its program-data account;
- `config` (PDA `["config"]`);
- the USDC mint and PhUSD (`PhUsd11YkbjSaWjFncfAAmatntsjx3MgDR9B6g1ks3A`);
- Phoenix's perp asset map (`2nHGAaEw3D5dd4hVueaUNoygkQFmoeKqRQWnSPqSMFUC`);
- its withdraw queue (`3c3NTwpg7yW91FxijkHBXwVH1xUifun3Z8TC5eW5Si3K`);
- its global vault (`csZXgw2G58hbiWc9ndxaxrQVYVvqdXgQzYLuznEzHJu`);
- the system program.

### 4.3 `create_vault(id, params, authority_nonce)`

- `id`: a `u16` of your choice; it is part of the vault's address.
- `authority_nonce`: a **random** `u64`. The authority PDA `["authority", vault, nonce]` becomes the vault's Phoenix trader, so nobody can register that trader first.

The instruction creates:
- the share mint;
- the authority's USDC, PhUSD and escrow token accounts;
- the vault's Phoenix trader account (16 position slots).

**Recommended `VaultParams` to start:**

| Field | Meaning | Start with |
|---|---|---|
| `max_tvl_usd` | NAV cap (6 dp) | What the markets can absorb (§11) |
| `max_deposit_usd` | One deposit's cap | 10–20% of TVL |
| `entry_fee_bps`, `exit_fee_bps` | Fees kept in the vault (≤ 100). Cover the trades money moving in or out causes; the exit fee also applies to cancels | 5–10 each |
| `buffer_bps` | Idle USDC kept per dollar of NAV (instant withdrawals) | 500 (5%) |
| `margin_bps` | Phoenix effective collateral floor per dollar of short | 5000 |
| `slippage_bps` | Price band on every swap and order (10–200) | 30–50 |
| `max_turnover_usd` | Deploys and unwinds per 24 h | 20–30% of TVL |
| `deposits_open` | | `false` until the legs and the bot are ready |

### 4.4 Phoenix onboarding

Phoenix's trader capabilities are **permissioned**. Ask Phoenix to enable the vault's trader
account (`Vault.trader_account`). Until they do, deploys fail.

### 4.5 `add_leg(kind, market, params)`: one per market

`kind` is `NativeSol` (the SOL leg: native SOL as Phoenix collateral) or `Spot` (SPL or
Token-2022 tokens).

**`market`** must match Phoenix's listing; the program checks it against the perp asset map. Take
the values from `GET https://perp-api.phoenix.trade/v1/view/exchange/markets`:

| Field | From the API |
|---|---|
| `asset_id` | `assetId` |
| `base_lot_decimals` | `baseLotsDecimals` |
| `tick_size` | `tickSize` |
| orderbook, spline collection (accounts) | `marketPubkey`, `splinePubkey` |

**Accounts:**
- admin, config, the vault;
- the leg PDA `["leg", vault, [leg_count]]`;
- the authority and the asset mint;
- `vault_asset`: the authority's ATA for spot legs (created here), none for SOL;
- the orderbook, spline collection and perp asset map;
- the asset's token program;
- remaining accounts: every existing leg, in order.

**Rules the program enforces:**
- At most 8 live legs (16 ever).
- No two live legs on one market or one mint.
- Token-2022 mints with transfer fees, transfer hooks, interest or non-transferable flags are refused.
- A multiplier the vault can't read is refused.

**Recommended `LegParams`:**

| Field | Start with |
|---|---|
| `active` | `true` |
| `max_notional_usd` | Your per-leg limit |
| `max_asset_units` | SOL leg: **450 SOL** (Phoenix counts at most 500 SOL per trader as collateral). Others: no limit |
| `max_oi_share_bps` | 2500 (25% of the market's open interest) |
| `max_nav_share_bps` | 5000 |
| `after_hours_trading` | `false` for stocks (no deploys outside the session) |

**Which markets** (from the 2026-10-16 scan of Phoenix's 99 markets): a leg needs a Solana token
with Jupiter liquidity, a cross-margin market and real open interest. The ones that qualified:

- **Good funding and capacity:** SOL, BTC, NEAR, META, TSLA, SKHY, MU, HYPE.
- **Hedgeable but low funding that day:** ETH, ZEC.

Re-scan before adding.

### 4.6 Address lookup table

Every vault transaction needs a lookup table, users' included: with 5 legs a deposit names 35
accounts. Create one and add:

- **Programs:** the vault program, Phoenix, the Phoenix log authority, the Phoenix global config,
  Hawkeye, Ember, SPL Token, Token-2022, Associated Token, System, Jupiter.
- **Accounts:**
  - Config, the vault, its authority, share mint, USDC, PhUSD and escrow accounts, its trader account;
  - the USDC and PhUSD mints;
  - the perp asset map, the withdraw queue and the global vault;
  - Ember's state and vault;
  - Phoenix's global trader index and active trader buffer accounts;
  - the native mint and the authority's wrapped-SOL ATA.
- **For each leg:** the leg, its asset account, its mint, its orderbook and its spline collection.

```bash
solana address-lookup-table create --keypair admin.json
solana address-lookup-table extend <TABLE> --keypair admin.json --addresses <A1>,<A2>,…   # up to ~20 per call
```

Put the table in the bot's `lookup_tables` and the frontend's `VAULT_LOOKUP_TABLE`. **Extend it
whenever a leg is added**, and when Phoenix's index accounts change.

### 4.7 Open deposits

Do this once the bot runs and `vanna-keeper plan` looks right: `update_vault` with
`deposits_open = true`. From then on the bot's deposit guard manages it (unless
`manage_deposits = false`).

---

## 5. Build and configure the bot

### 5.1 Build

The Rust toolchain is pinned to 1.98.1 in `rust-toolchain.toml`.

```bash
cargo build --release -p vanna_keeper
# binary: target/release/vanna-keeper
```

### 5.2 Configuration (`keeper.toml`)

Copy [`keeper/keeper.example.toml`](../../keeper/keeper.example.toml).

**Deployment:**

| Key | Meaning | Default |
|---|---|---|
| `rpc_url` | Mainnet RPC: needs `getMultipleAccounts` with data slices and `minContextSlot`, `simulateTransaction` with return data. Use a paid one | required |
| `keypair_path` | The keeper's keypair JSON (`Config.keeper`) | required |
| `vault_id` | The vault's `id` | required |
| `lookup_tables` | The vault's table(s) (§4.6) | `[]` |
| `phoenix_api` | Funding and index accounts | `https://perp-api.phoenix.trade` |
| `jupiter_api`, `jupiter_api_key` | Routes (`/quote`, `/swap-instructions`); add a key for `api.jup.ag` | `https://lite-api.jup.ag/swap/v1` |
| `jupiter_max_accounts` | Upper bound on a route's accounts; the bot lowers it to what the transaction leaves | 20 |
| `[symbols]` | Phoenix symbols by asset id, where the perp map's name isn't the API's (`12 = "TSLA"`) | none |
| `tick_secs` | Seconds between ticks | 30 |
| `funding_refresh_secs` | Seconds between funding reads | 900 |
| `dry_run` | Plan and log, send nothing | false |
| `alert_webhook` | Slack or Discord webhook (the body has `text` and `content`) | none |
| `priority_fee_micro_lamports` | Priority fee per compute unit | 0 |

**`[policy]`** (capital APRs in percent; USD in whole dollars; bps):

| Key | Meaning | Default |
|---|---|---|
| `entry_apr` | Enter a leg at or above this (and positive 7-day funding) | 6.0 |
| `exit_apr` | Unwind a leg below this | 2.0 |
| `rotate_gap_apr` | Rotate only for at least this much more | 8.0 |
| `manage_deposits` | Let the bot open and close deposits | true |
| `close_deposits_apr`, `reopen_deposits_apr` | Deposit guard | 3.0, 6.0 |
| `max_oi_share_bps`, `trim_oi_share_bps` | Fill to 25% of a market's open interest; trim above 35% | 2500, 3500 |
| `max_leg_nav_share_bps` | The bot's own cap per leg | 5000 |
| `spot_margin_bps` | USDC margin per dollar of a spot short (1x) | 10000 |
| `sol_margin_bps`, `sol_collateral_weight_bps` | SOL leg's margin target and Phoenix's weight for SOL collateral | 8000, 8000 |
| `margin_headroom_bps` | Target never below `margin_bps` + this | 2000 |
| `margin_alarm_bps`, `margin_topup_bps`, `margin_release_bps` | Alarm within this of the floor; top up this far under target; release this far over | 1000, 1000, 2000 |
| `hedge_tolerance_bps` | Re-hedge beyond this | 50 |
| `min_trade_usd`, `max_trade_usd` | Trade size bounds | 500, 50000 |
| `buffer_slack_usd` | Idle USDC kept above the vault's reserve | 1000 |
| `buffer_refill_bps` | Sell to refill once idle is under this share of the reserve | 5000 |
| `max_actions_per_tick` | Moves per tick | 8 |
| `failure_cooldown_secs`, `urgent_cooldown_secs` | Cooldown after a failure, doubling each time, up to a day (urgent: alarm, re-hedge, requests, refill) | 600, 60 |
| `max_funding_age_secs` | Funding older than this is not acted on | 10800 |
| `request_alert_secs` | Alert when a request has waited this long | 21600 |

---

## 6. Run it

```bash
vanna-keeper plan --config keeper.toml   # read the vault, print every move it would make, send nothing
vanna-keeper once --config keeper.toml   # one tick, then exit
vanna-keeper run  --config keeper.toml   # tick forever, tick_secs apart
```

**The first time, run `plan` until its output makes sense.** Then `once`, then start `run` with
small caps (`max_tvl_usd`, `max_trade_usd`).

The bot keeps no state on disk: everything it needs is on-chain, and it is safe to restart at any
time. Cooldowns and alert history reset on restart.

### As a service (systemd)

```ini
# /etc/systemd/system/vanna-keeper.service
[Unit]
Description=Vanna multi-vault keeper
After=network-online.target

[Service]
ExecStart=/opt/vanna/vanna-keeper run --config /etc/vanna/keeper.toml
Restart=always
RestartSec=10
User=vanna
# The keypair file: readable by this user only (chmod 600)

[Install]
WantedBy=multi-user.target
```

```bash
systemctl enable --now vanna-keeper
journalctl -u vanna-keeper -f
```

Run **one** keeper per vault. Two would race each other: harmless to funds (the program checks
everything) but wasteful.

**Keep the keeper's SOL** above 0.1 SOL for fees and wrapped-SOL account rent; it alerts under
0.05 SOL.

---

## 7. Alerts and monitoring

Alerts go to stderr and to `alert_webhook`. Each one is sent once an hour at most, keyed by kind
not text. URLs in alerts are cut to their host.

| Alert | Meaning | What to do |
|---|---|---|
| `the protocol is paused…` | `Config.paused` | Expected if you paused it |
| `the vault owes Phoenix more than it holds` | Equity below 0 | Investigate at once (§8) |
| `margin X% of every short, floor Y%` | Margin alarm | The bot is posting or selling. If it keeps firing, add USDC margin or reduce legs |
| `withdrawal request N has waited Nh` | A request is stuck | Check turnover, routes and the bot's failures |
| `…the daily turnover limit is reached` | Requests wait for tomorrow's window | Raise `max_turnover_usd` (`update_vault`) if justified |
| `…swaps are off` | `Config.swap_program` is the default key | Set Jupiter again with `set_config` |
| `no fresh funding for X…` | Phoenix's funding API is unreachable for that market | The bot stops entering or exiting on stale data; check the API |
| `the SOL leg may hold more than the 500 SOL…` | SOL leg cap too high | `update_leg` with `max_asset_units` at most 450 SOL |
| `the keeper has N SOL left` | Fees at risk | Top up the keeper |
| `<move> failed (…)` | A move failed and is cooling down | Read the log line; see §8 |
| `tick failed: …` | The whole tick failed (RPC, account read) | Check the RPC |

**Worth graphing** (from the log summary line each tick, or from events):

- NAV and share price;
- idle versus reserved;
- the margin ratio;
- pending requests and their age;
- per-leg value and short;
- moves sent versus failed.

---

## 8. Runbook

| Situation | What happens | Action |
|---|---|---|
| A Jupiter route fails for a market | Its deploys and rotations cool down and halve. Requests are paid from other legs | Check Jupiter liquidity. Deactivate the leg (`update_leg active=false`) if it lasts |
| Phoenix's API is down | Index accounts fall back to the last ones; funding goes stale after 3 h (no entries or exits) | Wait. Margin defence and requests keep running |
| RPC down | Ticks fail and alert | Switch `rpc_url`, restart |
| Margin alarm after a rally | The bot posts buffer USDC or sells the rallied spot leg | If the buffer is empty and nothing sells: add USDC, or `update_vault` to a lower `margin_bps` temporarily |
| Request stuck | It is either returned (the user reclaims it) or waits for turnover or routes | Raise turnover; check failures |
| A user's USDC account is closed or frozen | The request is returned; the user's shares wait in escrow | Tell the user to reopen the account and cancel (no fee) |
| A stock split or special dividend (multiplier window) | The bot leaves that leg alone (no re-hedge, exit or rotation; sold last for requests); deploys into it wait | Nothing. It resumes after the window (3 days after the change) |
| A market is to be delisted | | Deactivate the leg; the bot unwinds it (exits). Then `retire_leg(max_units)` once flat |
| A token or market already broke (every instruction fails) | | `update_leg active=false`, then `retire_leg(max_units ≥ dust held)`. Retiring reads only that leg |
| Need to stop everything | | `set_config(paused=true)`: deposits and deploys stop; withdrawals, unwinds and re-hedges still work. Or stop the bot |
| Rotate the keeper key | | `set_config` with the new `keeper`, update `keypair_path`, restart |
| Program upgrade | | Stop the bot; deploy; regenerate the IDL; run `plan`; start |

---

## 9. Security

**Keeper key:**
- Keep it in an HSM or KMS, or at least in a 600-mode file on a hardened host.
- It can't withdraw funds, but it can trade within the band and turnover limits, or stop working.

**Admin key:** behind a multisig. It can:
- pause the vault;
- change every limit;
- retire legs;
- set the keeper and swap program.

**Program bounds:**
- band at most 2%, fees at most 1%;
- turnover limited per 24 h;
- every swap checked against Phoenix's mark;
- a swap may move nothing but its two sides.

**Secrets in logs:** RPC and webhook URLs are cut to their host. Keep `keeper.toml` out of the
repository.

**Dry runs:** `plan` and `dry_run = true` never send.

---

## 10. Testing

```bash
# Program and the tests' mock Jupiter
SBF=~/.local/share/solana/install/releases/3.1.10/solana-release/bin/cargo-build-sbf
$SBF --manifest-path programs/vanna_phoenix-multi/Cargo.toml --sbf-out-dir target/deploy
$SBF --manifest-path programs/vanna_phoenix_vault/tests/mock_jupiter/Cargo.toml --sbf-out-dir target/deploy

cargo test -p vanna_phoenix_multi_vault        # 55 tests: real Phoenix, Ember and Hawkeye in LiteSVM
cargo test -p vanna_keeper -- --nocapture      # 41 tests: the bot end to end, with its logs
```

**What the bot's tests run** (`keeper/tests`):

- `scenarios.rs`, `flows.rs`, `audit.rs` drive the real keeper against the real program and Phoenix:
  - deposits into the best legs;
  - nothing pays;
  - negative funding;
  - withdrawals from the lowest-paying leg;
  - buffer refill;
  - oldest-first requests;
  - wind-down to zero;
  - rotations;
  - rallies;
  - multiplier windows;
  - open interest moves;
  - after hours;
  - unpayable requests;
  - failing routes;
  - alert dedupe.
- `sim.rs` is the backend: LiteSVM accounts and views, mock Jupiter routes, funding the test sets.

---

## 11. Known limits

- **Not yet run on mainnet:** the live backend (RPC, Jupiter's `/swap-instructions` for a PDA authority, lookup tables) has not run against a deployed vault. Start with `plan` and small caps.
- **Capacity:** at 25% of Phoenix's open interest, the 2026-10-16 scan put total room at about $2M of short notional, about $3.9M of TVL. Size `max_tvl_usd` to that.
- **Native SOL:** Phoenix counts at most 500 SOL per trader account as collateral, and every instruction calls Phoenix's `sync_native`. If Phoenix turned native SOL off, the vault would stop until an upgrade.
- **Phoenix's withdrawal throttle:** a $2M exchange-wide budget, refilling at $450 a slot. A very large margin withdrawal fails and is retried.
- **Exits without the keeper:**
  - instant withdrawals keep working from the buffer;
  - anyone may pay a request the buffer fully covers;
  - beyond that, requests wait for the keeper.
- **Request order:** only the keeper may pay requests out of order. It pays oldest first, but nothing on-chain forces it to.
