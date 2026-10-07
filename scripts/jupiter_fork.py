#!/usr/bin/env python3
"""Snapshots real Jupiter routes for the vault tests.

For each swap a test makes, this asks Jupiter for a route with the vault authority PDA as
the trader, then downloads every mainnet account the route touches (pools, pool vaults, tick
arrays, mints, Jupiter's own accounts) and the Jupiter and AMM program binaries. The tests
load all of it into LiteSVM next to Phoenix, so the vault runs the real Jupiter program
against real pool state.

Routes are pinned to direct routes over one AMM (Orca Whirlpool for SOL, ETH and BTC; Raydium
CLMM, where the tokenized stocks trade deepest, for TSLAx and METAx): Jupiter's best routes
often go through proprietary market makers whose quotes expire within slots, which a snapshot
cannot replay.

Usage (from basis-trading-solana-phoenix/):
  python3 scripts/jupiter_fork.py --programs-only   downloads the programs the committed
                                                    snapshot needs (run once)
  python3 scripts/jupiter_fork.py                   takes a fresh snapshot of routes and accounts
Writes programs/vanna_phoenix_vault/tests/fixtures/jupiter_routes.json and
target/deploy/.cache/mainnet/<program>.so.
"""
import base64
import json
import os
import subprocess
import time
import urllib.parse
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "programs/vanna_phoenix_vault/tests/fixtures/jupiter_routes.json")
CACHE = os.path.join(ROOT, "target/deploy/.cache/mainnet")
SOLANA = os.path.expanduser("~/.local/share/solana/install/releases/3.1.10/solana-release/bin/solana")
RPC = "https://api.mainnet-beta.solana.com"
JUP = "https://lite-api.jup.ag/swap/v1"
UA = {"User-Agent": "crate-source-reader"}

VAULT_PROGRAM = "GtuQoYxzytGKtLqQGkYrVbsaPHTZN5jboP25DrSmKvGr"
# The authority nonce the tests create every vault with.
AUTHORITY_NONCE = 0
JUPITER = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4"
USDC = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
WETH = "7vfCXTUXx5WJV5JADk17DUJ4ksgau7utNKj4b963voxs"
CBBTC = "cbbtcf3aa214zXHbiAZQwf4122FBYbraNdFqgw4iMij"
# Backed's tokenized stocks: Token-2022 mints.
TSLAX = "XsDoVfqeBukxuZHWhdvWHBhgEHjGNst4MLodqsJHzoB"
METAX = "Xsa62P5mvPszXL1krVUnU5ar38bBSVcWAB6fmPCo5Zu"
WSOL = "So11111111111111111111111111111111111111112"
TOKEN = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
TOKEN_2022 = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"
ATA = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
SKIP = {
    TOKEN, TOKEN_2022, ATA,
    "11111111111111111111111111111111",
    "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",
    "Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo",
    "SysvarRent111111111111111111111111111111111",
    "SysvarC1ock11111111111111111111111111111111",
    "Sysvar1nstructions1111111111111111111111111",
}

# (name, vault asset mint, input mint, output mint, amount in, AMM): one route each way per
# vault, sized like the tests' swaps. Tests resize a route to the exact amount the vault plans
# (its data ends with the input amount and the quote), the way an app re-quotes the same route.
SWAPS = [
    ("eth_buy", WETH, USDC, WETH, 3_500_000_000, "Whirlpool"),
    ("eth_sell", WETH, WETH, USDC, 100_000_000, "Whirlpool"),
    ("btc_buy", CBBTC, USDC, CBBTC, 10_000_000_000, "Whirlpool"),
    ("btc_sell", CBBTC, CBBTC, USDC, 10_000_000, "Whirlpool"),
    ("sol_buy", WSOL, USDC, WSOL, 1_000_000_000, "Whirlpool"),
    ("sol_sell", WSOL, WSOL, USDC, 5_000_000_000, "Whirlpool"),
    ("tsla_buy", TSLAX, USDC, TSLAX, 5_000_000_000, "Raydium CLMM"),
    ("tsla_sell", TSLAX, TSLAX, USDC, 1_000_000_000, "Raydium CLMM"),
    ("meta_buy", METAX, USDC, METAX, 5_000_000_000, "Raydium CLMM"),
    ("meta_sell", METAX, METAX, USDC, 500_000_000, "Raydium CLMM"),
]
# Token program of each mint that is not a classic SPL token.
TOKEN_PROGRAM_OF = {TSLAX: TOKEN_2022, METAX: TOKEN_2022}
# Jupiter's own floor on a resized route is the scaled quote less this; the vault's check
# against Phoenix's mark price (1%) is the one that matters.
SLIPPAGE_BPS = 300


def http_json(url, body=None):
    data = json.dumps(body).encode() if body is not None else None
    headers = dict(UA)
    if body is not None:
        headers["Content-Type"] = "application/json"
    for attempt in range(5):
        try:
            return json.load(urllib.request.urlopen(urllib.request.Request(url, data, headers), timeout=60))
        except Exception as e:
            err = e
            time.sleep(2 * (attempt + 1))
    raise err


def rpc(method, params):
    return http_json(RPC, {"jsonrpc": "2.0", "id": 1, "method": method, "params": params})["result"]


def pda(*seeds):
    out = subprocess.check_output([SOLANA, "find-program-derived-address", VAULT_PROGRAM, *seeds], text=True)
    return out.split()[0]


def ata(owner, mint):
    token = TOKEN_PROGRAM_OF.get(mint, TOKEN)
    out = subprocess.check_output(
        [SOLANA, "find-program-derived-address", ATA, f"pubkey:{owner}", f"pubkey:{token}", f"pubkey:{mint}"],
        text=True,
    )
    return out.split()[0]


def fetch_accounts(keys):
    accounts = {}
    keys = sorted(keys)
    for i in range(0, len(keys), 100):
        chunk = keys[i : i + 100]
        values = rpc("getMultipleAccounts", [chunk, {"encoding": "base64"}])["value"]
        for key, value in zip(chunk, values):
            if value is None:
                # An uninitialized tick array the route may pass through: absent in the test too.
                print("absent on mainnet:", key)
                continue
            accounts[key] = {
                "owner": value["owner"],
                "lamports": value["lamports"],
                "executable": value["executable"],
                "data": value["data"][0],
            }
    return accounts


def fetch_program(program_id):
    os.makedirs(CACHE, exist_ok=True)
    path = os.path.join(CACHE, f"{program_id}.so")
    if os.path.exists(path):
        return path
    account = rpc("getAccountInfo", [program_id, {"encoding": "base64"}])["value"]
    raw = base64.b64decode(account["data"][0])
    # Upgradeable loader: the program account points at its program data account.
    programdata = b58encode(raw[4:36])
    data = rpc("getAccountInfo", [programdata, {"encoding": "base64"}])["value"]
    elf = base64.b64decode(data["data"][0])[45:]
    with open(path, "wb") as f:
        f.write(elf)
    return path


ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58encode(data):
    n = int.from_bytes(data, "big")
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = ALPHABET[r] + out
    return "1" * (len(data) - len(data.lstrip(b"\0"))) + out


def main():
    routes, wanted, programs = {}, set(), set()
    for name, asset_mint, input_mint, output_mint, amount, amm in SWAPS:
        basis_vault = pda("string:basis_vault", f"pubkey:{asset_mint}")
        authority = pda("string:authority", f"pubkey:{basis_vault}", f"u64le:{AUTHORITY_NONCE}")
        query = urllib.parse.urlencode({
            "inputMint": input_mint,
            "outputMint": output_mint,
            "amount": amount,
            "slippageBps": SLIPPAGE_BPS,
            "onlyDirectRoutes": "true",
            "dexes": amm,
        })
        quote = http_json(f"{JUP}/quote?{query}")
        if "routePlan" not in quote:
            raise SystemExit(f"{name}: {quote}")
        swap = http_json(f"{JUP}/swap-instructions", {
            "quoteResponse": quote,
            "userPublicKey": authority,
            "wrapAndUnwrapSol": False,
            "useSharedAccounts": True,
            "skipUserAccountsRpcCalls": True,
            "destinationTokenAccount": ata(authority, output_mint),
        })
        ix = swap["swapInstruction"]
        assert ix["programId"] == JUPITER, ix["programId"]
        own = {authority, ata(authority, input_mint), ata(authority, output_mint)}
        for meta in ix["accounts"]:
            if meta["pubkey"] not in own | SKIP | {JUPITER}:
                wanted.add(meta["pubkey"])
        # The vault's asset mint, which the tests recreate with their own mint authority.
        wanted.add(asset_mint)
        programs.add(JUPITER)
        routes[name] = {
            "authority": authority,
            "input_mint": input_mint,
            "output_mint": output_mint,
            "in_amount": int(quote["inAmount"]),
            "quoted_out_amount": int(quote["outAmount"]),
            "amm": [r["swapInfo"]["label"] for r in quote["routePlan"]],
            "accounts": ix["accounts"],
            "data": ix["data"],
            "lookup_tables": swap.get("addressLookupTableAddresses", []),
        }
        print(f"{name}: {quote['inAmount']} -> {quote['outAmount']} via {routes[name]['amm']} ({len(ix['accounts'])} accounts)")
        time.sleep(1)

    accounts = fetch_accounts(wanted)
    for key, account in list(accounts.items()):
        if account["executable"]:
            programs.add(key)
            del accounts[key]
    for program in sorted(programs):
        print("program", program, fetch_program(program))
    slot = rpc("getSlot", [])
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w") as f:
        json.dump({
            "fetched_at": int(time.time()),
            "slot": slot,
            "programs": sorted(programs),
            "routes": routes,
            "accounts": accounts,
        }, f, indent=1)
    print(f"wrote {OUT}: {len(routes)} routes, {len(accounts)} accounts at slot {slot}")


if __name__ == "__main__":
    import sys

    if "--programs-only" in sys.argv:
        for program in json.load(open(OUT))["programs"]:
            print("program", program, fetch_program(program))
    else:
        main()
