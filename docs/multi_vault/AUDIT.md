# Audit: multi-strategy basis vault and keeper bot (2026-10-16)

**Scope:**
- `programs/vanna_phoenix-multi`: the Anchor program, all instructions.
- `keeper`: the off-chain keeper bot (observe, policy, actions, loop, live backend).

The earlier program review (README, "Security review (2026-10-07)") was the starting point.

**Method:**
- Three independent line-by-line reviews:
  1. user money paths and valuation;
  2. keeper and admin instructions, swaps and the Phoenix CPI;
  3. the bot against the program.
- Every finding was re-checked against the code before it was accepted.
- Mainnet Phoenix state was read where a finding depended on it (the withdraw queue's fee and throttle).
- Each accepted finding has a regression test that runs the scenario end to end, against the real Phoenix, Ember and Hawkeye programs in LiteSVM.
- Tools: `cargo clippy` (0 warnings) and `cargo audit` (0 vulnerabilities; 5 transitive crates marked unmaintained: `ansi_term`, `bincode`, `derivative`, `libsecp256k1`, `paste`, all from the Solana SDK).

**Result:**
- No Critical findings.
- The program had 6 Medium, 4 Low and 3 Informational findings. Every Medium is fixed except P-6, which is documented.
- The bot had 3 High, 5 Medium and 4 Low findings. All are fixed.

| ID | Severity | Finding | Status | Regression test |
|---|---|---|---|---|
| P-1 | Medium | A later request can take USDC raised for an older one | Fixed | `audit::a_request_cannot_jump_the_queue_for_usdc_raised_for_an_older_one` |
| P-2 | Medium | A user can lock their own request (and its reserve) forever | Fixed | `audit::a_request_whose_user_gave_their_accounts_away_is_returned` |
| P-3 | Medium | A flat leg whose token or market broke cannot be retired; everything stops | Fixed | `audit::flat_legs_whose_tokens_broke_can_still_be_retired`, `audit::a_token_with_an_unreadable_multiplier_cannot_be_added` |
| P-4 | Medium | In a multiplier window, unwinds are checked at the current multiplier | Fixed | `audit::a_sale_in_a_multiplier_window_must_fetch_the_higher_value` |
| P-5 | Medium | Reserved USDC cannot repay Phoenix, so a SOL vault deadlocks after a rally | Fixed | `audit::reserved_usdc_may_cover_what_the_vault_owes_phoenix` |
| P-6 | Medium | Every instruction depends on Phoenix's native SOL feature | Documented | n/a |
| P-7 | Low | A $1 donation blocks a retirement | Fixed | `audit::a_small_donation_cannot_stop_a_retirement` |
| P-8 | Low | Anyone can fulfill someone else's request in a multiplier window | Fixed | `audit::only_the_user_or_the_keeper_fulfills_in_a_multiplier_window` |
| P-9 | Low | Closing and re-creating a USDC account drains the cranker's SOL | Fixed | `audit::a_cranker_never_pays_for_its_users_accounts` |
| P-10 | Low | Free cancels let anyone make the keeper churn | Fixed | `audit::a_cancel_costs_the_exit_fee_and_a_returned_request_does_not` |
| P-11 | Info | Phoenix's withdrawal throttle and fee fields | Verified, documented | n/a |
| P-12 | Info | Hawkeye return data version not checked | Fixed | (existing suite) |
| P-13 | Info | A swap could reassign the authority account | Fixed (hardening) | n/a |
| K-1 | High | An unpayable oldest request stalls every request and the buffer refill | Fixed | keeper `audit::a_request_the_program_cannot_pay_is_returned_and_the_queue_moves_on` |
| K-2 | High | A SOL-heavy vault cannot pay a full withdrawal after a rally | Fixed | keeper `audit::a_sol_vault_pays_a_full_withdrawal_after_a_rally` |
| K-3 | High | A market that can't be bought churns the vault | Fixed | keeper `audit::a_market_that_cannot_be_bought_does_not_churn_the_vault` |
| K-4 | Medium | Raising USDC tries one leg only; cooldowns shared across rules | Fixed | keeper `audit::if_the_cheapest_leg_cannot_be_sold_the_next_one_pays` |
| K-5 | Medium | Margin posts of exactly the free buffer race the program's reserve | Fixed | (policy unit tests) |
| K-6 | Medium | A Phoenix API or getProgramAccounts outage stops the keeper | Fixed | n/a (live backend) |
| K-7 | Medium | The 500-SOL Phoenix collateral cap is not modeled | Fixed | `policy::tests::the_sol_leg_takes_the_whole_budget_as_its_own_margin` |
| K-8 | Medium | A stock leg in a multiplier window is traded like any other | Fixed | keeper `audit::a_stock_in_a_multiplier_window_is_left_alone` |
| K-9 | Low | Alert spam | Fixed | keeper `audit::an_alert_goes_out_once_however_its_numbers_move` |
| K-10 | Low | API keys in logs and alerts | Fixed | `keeper::tests::urls_lose_their_keys` |
| K-11 | Low | Reads not tied to the last send; 1.6 MB perp map per read; tables cached forever | Fixed | n/a (live backend) |
| K-12 | Low | Fixed route size, unchecked route type, guard blind to capacity, keeper SOL unwatched | Fixed | flow tests updated |

---

## Program

### P-1 (Medium): a later request could take USDC raised for an older one

**What was wrong.** `withdraw` pays only from USDC above what live requests are worth. But
`fulfill_withdraw` (anyone may call it) checked only `pay <= idle`. A user refused an instant
withdrawal could `request_withdraw` and `fulfill_withdraw` their own request in the same
transaction. That took the USDC the keeper had just raised for an older request, and sent the
keeper back to unwind again at everyone's cost.

**Fix** (`user.rs::fulfill_withdraw`):
- The keeper (or admin) may pay any request, and pays oldest first.
- Anyone else, the user included, must leave every other live request's value in the buffer: `pay + others <= idle`. `others` is what the other requests are worth once this one has left, so the last two holders out are not blocked by rounding.

### P-2 (Medium): a user could lock their own request forever

**What was wrong.** Fulfill and cancel paid into the request user's associated token accounts,
creating them if missing. Classic SPL token accounts can change owner (`SetAuthority`). A user
who gave their USDC and share ATAs to another key made the ATA program's idempotent create fail.
That made both `fulfill_withdraw` and `cancel_withdraw` revert for everyone. The request's value
stayed reserved, at no cost to them, and the keeper bot kept raising USDC it could never pay out.

**Fix:**
- `return_withdraw` is a new keeper/admin instruction. It moves no tokens: it marks the request `returned` and drops its shares from `pending_shares`.
- The shares stay in escrow, earning as before, until the user takes them back with `cancel_withdraw` (no fee for a returned request).
- `fulfill_withdraw` now requires the user's existing USDC ATA, still owned by them. It no longer creates one (see P-9).
- `fulfill_withdraw` refuses a returned request.

### P-3 (Medium): a flat leg whose token or market broke could not be retired

**What was wrong.** `retire_leg` valued every leg, the one being retired included. Once a leg's
token multiplier became unreadable, or its mark went to zero, every snapshot reverted. That
stopped deposits, withdrawals, unwinds and `retire_leg` itself, the one way out, so recovery
needed a program upgrade. `add_leg` also accepted a token whose multiplier the vault could not
read, which bricked the vault the moment the leg was added.

**Fix** (`admin.rs::retire_leg(max_units)`):
- Only the target leg is read: its position (Hawkeye) and its raw units.
- If it can be priced, its value must be at most `RETIRE_DUST_USD`.
- If it can't be priced (multiplier unreadable, mark at zero), it must have been deactivated first and hold at most `max_units`.
- If Phoenix removed its market from the asset map, it holds no position there.
- Several broken legs retire one by one.
- `add_leg` runs `multiplier_range` on Token-2022 mints.

### P-4 (Medium): unwinds in a multiplier window were checked at the current multiplier

**What was wrong.** In a split or special-dividend window, the token's multiplier and Phoenix's
mark may not have caught up with each other. Withdrawals price low and deposits high, but
`unwind` checked its sale at the current multiplier. Take a 3:1 split where Phoenix's mark moved
before the token's multiplier did: a keeper could sell tokens about to be worth three times as
much at a third of their value, and count only that third against the turnover limit. That
breaks the stolen-key bound. `rehedge` in the same window would size the short to a multiplier
Phoenix might not agree with yet.

**Fix** (`keeper.rs`):
- In a window, the sale must fetch the higher multiplier's value, and counts that value against turnover.
- `rehedge` is refused in a window.

### P-5 (Medium): reserved USDC could not repay Phoenix, so a SOL-heavy vault could deadlock

**What was wrong.** After a SOL rally the SOL leg's short owes USDC on Phoenix, and Phoenix will
not release the native SOL until that is covered. When a withdrawal request reserves every idle
dollar, `move_margin` refuses to post any of it. So the SOL can't be sold, and the request can't
be paid.

**Fix** (`keeper.rs::move_margin`): the reserve may cover what the vault owes Phoenix plus
`PHOENIX_COST_CUSHION_BPS` (0.5%) of its shorts, which pays the closing fees. The NAV does not
move.

### P-6 (Medium, documented): every instruction depends on Phoenix's native SOL feature

Every snapshot starts with Phoenix's `sync_native`, which validates the index accounts that
Hawkeye only reads. If Phoenix turned native SOL collateral off, every instruction would revert
until an upgrade. The suggested follow-up is to validate the index accounts against
`GlobalConfig`'s header keys instead. That needs Phoenix's layout to be confirmed, so it is left
as a known dependency in the README.

### P-7 (Low): a $1 donation blocked a retirement

The retire dust limit was $1, so a griefer could front-run each attempt. It is now $100 at the
mark, and the admin names the most units it expects (`max_units`).

### P-8 (Low): anyone could fulfill someone else's request in a multiplier window

Withdrawals are priced low in a window, so a third party could choose a bad moment for someone
else's exit. Now only the user or the keeper may fulfill while any leg is in a window.

### P-9 (Low): ATA creation drained the cranker

Fulfill and keeper-cancel created the user's ATA at the cranker's cost. A user could close it and
repeat, keeping about 0.002 SOL a round. Fulfill no longer creates accounts, cancel is the
user's alone (at their cost), and the keeper returns unpayable requests (P-2).

### P-10 (Low): free cancels let anyone make the keeper churn

Request → keeper unwinds → cancel cost the requester nothing. Every holder paid for the round
trip. Cancelling now costs `exit_fee_bps` of the shares, burned so the value stays with the
holders. A returned request comes back whole.

### P-11 (Info): Phoenix's withdrawal throttle and fee

**What mainnet's withdraw queue shows** (read on 2026-10-16):
- `withdrawal_fee` and `enqueueing_fee` are both $0.50.
- The withdrawal budget is $2M exchange-wide, refilling at $450 a slot.

**What the real Phoenix program does** (in LiteSVM): a direct `withdraw_funds` was not charged
the fee. A withdrawal above the remaining budget is rejected outright (`RequestTooLarge`) and
nothing is queued.

**Effect:** a large `move_margin` out fails and is retried. Liveness only; no funds are at risk.

### P-12, P-13 (Info): hardening

- **P-12:** Hawkeye's return version is checked on both views.
- **P-13:** after a swap, the authority must still be a system-owned account with no data. A route signed by the authority could otherwise reassign it and break every later lamport transfer.

### Program: checked and found correct

- **Share math:** the virtual offset stops first-deposit and donation inflation, and every rounding favours the vault.
- **Arithmetic:** no overflow at realistic sizes, and `overflow-checks` is on.
- **Leg list:** ownership, type, vault and index are checked for every leg, and duplicates can't be passed.
- **Account binding:** every `VaultCore` account is pinned. Requests are tied to their vault.
- **Request lifecycle:** no double fulfill or cancel. `pending_shares` changes only in request, cancel, fulfill and return.
- **Perp asset map:** the offsets match `phoenix-rise-accounts` 0.6.12.
- **Swaps:** `resize_route` and `check_fill` hold, and the "nothing else may move" checks are complete. Account substitution is closed off.
- **Admin bounds:** band 10–200 bps, fees at most 1%, no duplicate markets, listings checked.

---

## Keeper bot

### K-1 (High): an unpayable oldest request stalled the queue

**What was wrong.** The bot looked only at the oldest request. If its user's USDC account was
closed or frozen, or it paid nothing (one share atom at a non-zero exit fee), every later
request waited forever. Buffer refills waited too.

**Fix:**
- The bot reads each request user's USDC account.
- It returns (`return_withdraw`) any request that pays nothing, can't be paid into, or failed three times. The queue then moves on.

### K-2 (High): a SOL-heavy vault could not pay a full withdrawal after a rally

See P-5. The bot now:
- posts the deficit from the reserve;
- sells spot legs before SOL while the vault owes Phoenix;
- keeps the SOL leg's exit cushion on Phoenix instead of releasing it and posting it again, which looped.

The test pays a $50k withdrawal from a vault holding nothing but SOL that is up 30%.

### K-3 (High): a market that can't be bought churned the vault

**What was wrong.** If a target market's deploy kept failing (no route, a thin book), each tick:
- sold the leg it rotated out of;
- failed to buy;
- bought the sold leg back;
- released and re-posted the margin.

**Fix:**
- A rotation first checks the target market's route, and sells nothing without one.
- The planner skips moves that are cooling down.
- Cooldowns double with each failure in a row, up to a day.
- Deploy and rotation sizes into a failing market halve each time.

The test runs six ticks against a market with no route: no SOL is sold and no margin moves.

### K-4 (Medium): raising USDC tried one leg only

**What was wrong.** If the cheapest leg's sale failed, a request waited. Cooldowns were shared
between urgent and optional moves, and a leg with stale funding counted as 0% and was sold first.

**Fix:**
- Every leg is offered in order: lowest-paying first; stale funding, multiplier windows and (while owing Phoenix) SOL last.
- Cooldowns are per rule and move.
- Urgent moves (alarm, re-hedge, requests, refill) cool down for 60 seconds, not 10 minutes.

### K-5 (Medium): margin posts raced the program's reserve

Posting exactly `idle − reserved` failed whenever the NAV ticked up before execution. Posts are
now `free − max($1, 0.2%)`.

### K-6 (Medium): API outages stopped the keeper

**What was wrong.** A Phoenix API outage stopped every tick, including margin defence.
getProgramAccounts (which public RPCs refuse) did the same.

**Fix:**
- Phoenix's index accounts fall back to the last ones read, which Phoenix checks anyway.
- Requests are read by PDA, in batches, from the oldest still open. No getProgramAccounts.

### K-7 (Medium): Phoenix's 500-SOL collateral cap was not modeled

The bot sized SOL deploys past what Phoenix counts as collateral. Capacity now stops at 500 SOL,
and an alert fires if the SOL leg's own cap is set above it.

### K-8 (Medium): stock legs in multiplier windows were traded like any other

In a window, the bot now leaves the leg alone:
- no re-hedges, exits, trims or rotations;
- requests are raised from the other legs first;
- quotes are checked at the higher multiplier, as the program checks them.

### K-9 to K-12 (Low)

- **K-9:** alerts are deduplicated by a stable key, not by their text. Text with live numbers had sent an alert every tick.
- **K-10:** URLs in logs and alerts are cut to their host (RPC and webhook URLs carry keys).
- **K-11:** after a send, reads wait for at least that slot (`min_context_slot`). The perp asset map is read only for the slots in use (about 160 KB, not 1.6 MB). Lookup tables are re-read every 10 minutes and after a failed send.
- **K-12:**
  - Jupiter's `maxAccounts` follows what the instruction leaves of Solana's 64.
  - Routes must be Jupiter's `route` or `shared_accounts_route`.
  - The deposit guard closes deposits when no paying leg has room. A deposit raises the NAV-share room itself, so that cap is not counted here.
  - The keeper's own SOL is watched.
  - Funding values are read as numbers or strings.

---

## Residual risks and what this audit did not cover

- **Live backend:** RPC, Jupiter's `/swap-instructions` for a PDA authority, and lookup tables have not run against a deployed vault. Start with `vanna-keeper plan`, then small amounts.
- **Phoenix dependencies:** native SOL (P-6), Phoenix's API at cold start (its index accounts), and the withdrawal throttle (P-11).
- **Trust in the keeper:**
  - It may pay requests out of order (it pays oldest first).
  - A stolen key can lose up to about twice the band on its daily turnover. Moving USDC to Phoenix is not counted in turnover (Phoenix does not charge direct withdrawals; P-11).
- **Exits without the keeper:** buffer withdrawals keep working, and anyone may fulfill a request the buffer fully covers. Beyond that, requests wait for the keeper.
- **A market Phoenix delists while the vault holds it:** the keeper must unwind it first. A delisted market is assumed to hold no position (P-3).

## Tests after the fixes

| Suite | Result |
|---|---|
| Program: unit | 11 passed |
| Program: integration (LiteSVM, real Phoenix), incl. 10 audit regressions | 44 passed |
| Keeper: unit | 13 passed |
| Keeper: integration (LiteSVM), incl. 6 audit regressions | 28 passed |
| `cargo clippy --all-targets` (program and keeper) | 0 warnings |
| `cargo audit` | 0 vulnerabilities, 5 unmaintained transitive crates |
