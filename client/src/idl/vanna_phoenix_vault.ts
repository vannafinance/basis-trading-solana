/**
 * Program IDL in camelCase format in order to be used in JS/TS.
 *
 * Note that this is only a type helper and is not the actual IDL. The original
 * IDL can be found at `target/idl/vanna_phoenix_vault.json`.
 */
export type VannaPhoenixVault = {
  "address": "GtuQoYxzytGKtLqQGkYrVbsaPHTZN5jboP25DrSmKvGr",
  "metadata": {
    "name": "vannaPhoenixVault",
    "version": "0.1.0",
    "spec": "0.1.0",
    "description": "Vanna basis vaults on Solana hedged on Phoenix perpetuals: one share vault per asset (SOL, ETH, BTC, TSLA, META)"
  },
  "instructions": [
    {
      "name": "createVault",
      "discriminator": [
        29,
        237,
        247,
        208,
        193,
        82,
        54,
        135
      ],
      "accounts": [
        {
          "name": "admin",
          "writable": true,
          "signer": true
        },
        {
          "name": "config",
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  99,
                  111,
                  110,
                  102,
                  105,
                  103
                ]
              }
            ]
          }
        },
        {
          "name": "assetMint"
        },
        {
          "name": "basisVault",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  98,
                  97,
                  115,
                  105,
                  115,
                  95,
                  118,
                  97,
                  117,
                  108,
                  116
                ]
              },
              {
                "kind": "account",
                "path": "assetMint"
              }
            ]
          }
        },
        {
          "name": "authority",
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  97,
                  117,
                  116,
                  104,
                  111,
                  114,
                  105,
                  116,
                  121
                ]
              },
              {
                "kind": "account",
                "path": "basisVault"
              },
              {
                "kind": "arg",
                "path": "authorityNonce"
              }
            ]
          }
        },
        {
          "name": "shareMint",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  115,
                  104,
                  97,
                  114,
                  101,
                  95,
                  109,
                  105,
                  110,
                  116
                ]
              },
              {
                "kind": "account",
                "path": "basisVault"
              }
            ]
          }
        },
        {
          "name": "usdcMint"
        },
        {
          "name": "phoenixMint"
        },
        {
          "name": "vaultUsdc",
          "writable": true
        },
        {
          "name": "vaultPhoenix",
          "writable": true
        },
        {
          "name": "vaultAsset",
          "writable": true,
          "optional": true
        },
        {
          "name": "traderAccount",
          "writable": true
        },
        {
          "name": "phoenixProgram",
          "address": "EtrnLzgbS7nMMy5fbD42kXiUzGg8XQzJ972Xtk1cjWih"
        },
        {
          "name": "logAuthority",
          "address": "GdxfTLSsdSY37G6fZoYtdGDSfgFnbT2EmRpuePZxWShS"
        },
        {
          "name": "globalConfig",
          "writable": true,
          "address": "2zskx2iyCvb6Stg7RBZkt1f6MrF4dpYtMG3yMvKwqtUZ"
        },
        {
          "name": "orderbook"
        },
        {
          "name": "splineCollection"
        },
        {
          "name": "perpAssetMap"
        },
        {
          "name": "tokenProgram",
          "address": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        },
        {
          "name": "assetTokenProgram",
          "docs": [
            "Token for the SOL vault."
          ]
        },
        {
          "name": "associatedTokenProgram",
          "address": "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
        },
        {
          "name": "systemProgram",
          "address": "11111111111111111111111111111111"
        }
      ],
      "args": [
        {
          "name": "kind",
          "type": {
            "defined": {
              "name": "vaultKind"
            }
          }
        },
        {
          "name": "market",
          "type": {
            "defined": {
              "name": "marketParams"
            }
          }
        },
        {
          "name": "params",
          "type": {
            "defined": {
              "name": "vaultParams"
            }
          }
        },
        {
          "name": "authorityNonce",
          "type": "u64"
        }
      ]
    },
    {
      "name": "deposit",
      "discriminator": [
        242,
        35,
        198,
        137,
        82,
        225,
        242,
        182
      ],
      "accounts": [
        {
          "name": "user",
          "writable": true,
          "signer": true
        },
        {
          "name": "core",
          "accounts": [
            {
              "name": "config",
              "pda": {
                "seeds": [
                  {
                    "kind": "const",
                    "value": [
                      99,
                      111,
                      110,
                      102,
                      105,
                      103
                    ]
                  }
                ]
              }
            },
            {
              "name": "basisVault",
              "docs": [
                "A vault of this program: only `create_vault` makes one, at its asset's PDA, so the",
                "owner and type checks are enough (re-deriving the PDA here would overflow the stack).",
                "Writable for the keeper's daily rebalancing count."
              ],
              "writable": true
            },
            {
              "name": "authority",
              "writable": true
            },
            {
              "name": "shareMint",
              "writable": true
            },
            {
              "name": "usdcMint"
            },
            {
              "name": "phoenixMint",
              "writable": true
            },
            {
              "name": "vaultUsdc",
              "writable": true
            },
            {
              "name": "vaultPhoenix",
              "writable": true
            },
            {
              "name": "vaultAsset",
              "writable": true,
              "optional": true
            },
            {
              "name": "assetMint",
              "docs": [
                "is read from it."
              ]
            },
            {
              "name": "assetTokenProgram"
            },
            {
              "name": "traderAccount",
              "writable": true
            },
            {
              "name": "phoenix",
              "accounts": [
                {
                  "name": "phoenixProgram",
                  "address": "EtrnLzgbS7nMMy5fbD42kXiUzGg8XQzJ972Xtk1cjWih"
                },
                {
                  "name": "logAuthority",
                  "address": "GdxfTLSsdSY37G6fZoYtdGDSfgFnbT2EmRpuePZxWShS"
                },
                {
                  "name": "globalConfig",
                  "writable": true,
                  "address": "2zskx2iyCvb6Stg7RBZkt1f6MrF4dpYtMG3yMvKwqtUZ"
                },
                {
                  "name": "hawkeyeProgram",
                  "address": "RiSeVw3ZjNfsaXPRb4mgaqYaEEt41pNNJoDvVh7pgQj"
                },
                {
                  "name": "emberProgram",
                  "address": "EMBERpYNE6ehWmXymZZS2skiFmCa9V5dp14e1iduM5qy"
                },
                {
                  "name": "emberState"
                },
                {
                  "name": "emberVault",
                  "writable": true
                }
              ]
            },
            {
              "name": "perpAssetMap",
              "writable": true
            },
            {
              "name": "orderbook",
              "writable": true
            },
            {
              "name": "splineCollection",
              "writable": true
            },
            {
              "name": "withdrawQueue",
              "writable": true
            },
            {
              "name": "globalVault",
              "writable": true
            }
          ]
        },
        {
          "name": "userShares",
          "writable": true
        },
        {
          "name": "userAsset",
          "docs": [
            "The user's asset tokens (spot vaults). The SOL vault takes lamports from `user`."
          ],
          "writable": true,
          "optional": true
        },
        {
          "name": "userUsdc",
          "docs": [
            "The user's USDC, when the deposit brings margin."
          ],
          "writable": true,
          "optional": true
        },
        {
          "name": "swapProgram",
          "optional": true
        },
        {
          "name": "vaultWsol",
          "writable": true,
          "optional": true
        },
        {
          "name": "tokenProgram",
          "address": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        },
        {
          "name": "associatedTokenProgram",
          "address": "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
        },
        {
          "name": "systemProgram",
          "address": "11111111111111111111111111111111"
        }
      ],
      "args": [
        {
          "name": "gti",
          "type": "u8"
        },
        {
          "name": "atb",
          "type": "u8"
        },
        {
          "name": "assetAmount",
          "type": "u64"
        },
        {
          "name": "usdcAmount",
          "type": "u64"
        },
        {
          "name": "minShares",
          "type": "u64"
        },
        {
          "name": "route",
          "type": {
            "option": {
              "defined": {
                "name": "swapRoute"
              }
            }
          }
        }
      ]
    },
    {
      "name": "initialize",
      "discriminator": [
        175,
        175,
        109,
        31,
        13,
        152,
        155,
        237
      ],
      "accounts": [
        {
          "name": "admin",
          "docs": [
            "The program's upgrade authority: whoever deployed it, so nobody can take the admin role",
            "by initializing a fresh deployment first."
          ],
          "writable": true,
          "signer": true
        },
        {
          "name": "program",
          "address": "GtuQoYxzytGKtLqQGkYrVbsaPHTZN5jboP25DrSmKvGr"
        },
        {
          "name": "programData"
        },
        {
          "name": "config",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  99,
                  111,
                  110,
                  102,
                  105,
                  103
                ]
              }
            ]
          }
        },
        {
          "name": "usdcMint"
        },
        {
          "name": "phoenixMint",
          "docs": [
            "Phoenix's collateral token (PhUSD)."
          ]
        },
        {
          "name": "perpAssetMap"
        },
        {
          "name": "withdrawQueue"
        },
        {
          "name": "globalVault"
        },
        {
          "name": "systemProgram",
          "address": "11111111111111111111111111111111"
        }
      ],
      "args": [
        {
          "name": "keeper",
          "type": "pubkey"
        },
        {
          "name": "swapProgram",
          "type": "pubkey"
        }
      ]
    },
    {
      "name": "rebalance",
      "discriminator": [
        108,
        158,
        77,
        9,
        210,
        52,
        88,
        62
      ],
      "accounts": [
        {
          "name": "keeper",
          "writable": true,
          "signer": true
        },
        {
          "name": "core",
          "accounts": [
            {
              "name": "config",
              "pda": {
                "seeds": [
                  {
                    "kind": "const",
                    "value": [
                      99,
                      111,
                      110,
                      102,
                      105,
                      103
                    ]
                  }
                ]
              }
            },
            {
              "name": "basisVault",
              "docs": [
                "A vault of this program: only `create_vault` makes one, at its asset's PDA, so the",
                "owner and type checks are enough (re-deriving the PDA here would overflow the stack).",
                "Writable for the keeper's daily rebalancing count."
              ],
              "writable": true
            },
            {
              "name": "authority",
              "writable": true
            },
            {
              "name": "shareMint",
              "writable": true
            },
            {
              "name": "usdcMint"
            },
            {
              "name": "phoenixMint",
              "writable": true
            },
            {
              "name": "vaultUsdc",
              "writable": true
            },
            {
              "name": "vaultPhoenix",
              "writable": true
            },
            {
              "name": "vaultAsset",
              "writable": true,
              "optional": true
            },
            {
              "name": "assetMint",
              "docs": [
                "is read from it."
              ]
            },
            {
              "name": "assetTokenProgram"
            },
            {
              "name": "traderAccount",
              "writable": true
            },
            {
              "name": "phoenix",
              "accounts": [
                {
                  "name": "phoenixProgram",
                  "address": "EtrnLzgbS7nMMy5fbD42kXiUzGg8XQzJ972Xtk1cjWih"
                },
                {
                  "name": "logAuthority",
                  "address": "GdxfTLSsdSY37G6fZoYtdGDSfgFnbT2EmRpuePZxWShS"
                },
                {
                  "name": "globalConfig",
                  "writable": true,
                  "address": "2zskx2iyCvb6Stg7RBZkt1f6MrF4dpYtMG3yMvKwqtUZ"
                },
                {
                  "name": "hawkeyeProgram",
                  "address": "RiSeVw3ZjNfsaXPRb4mgaqYaEEt41pNNJoDvVh7pgQj"
                },
                {
                  "name": "emberProgram",
                  "address": "EMBERpYNE6ehWmXymZZS2skiFmCa9V5dp14e1iduM5qy"
                },
                {
                  "name": "emberState"
                },
                {
                  "name": "emberVault",
                  "writable": true
                }
              ]
            },
            {
              "name": "perpAssetMap",
              "writable": true
            },
            {
              "name": "orderbook",
              "writable": true
            },
            {
              "name": "splineCollection",
              "writable": true
            },
            {
              "name": "withdrawQueue",
              "writable": true
            },
            {
              "name": "globalVault",
              "writable": true
            }
          ]
        },
        {
          "name": "keeperAsset",
          "docs": [
            "The keeper's asset tokens (spot vaults); the SOL vault swaps lamports with `keeper`."
          ],
          "writable": true,
          "optional": true
        },
        {
          "name": "keeperUsdc",
          "writable": true
        },
        {
          "name": "tokenProgram",
          "address": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        },
        {
          "name": "systemProgram",
          "address": "11111111111111111111111111111111"
        }
      ],
      "args": [
        {
          "name": "gti",
          "type": "u8"
        },
        {
          "name": "atb",
          "type": "u8"
        },
        {
          "name": "buyAsset",
          "type": "bool"
        },
        {
          "name": "assetAmount",
          "type": "u64"
        },
        {
          "name": "usdcAmount",
          "type": "u64"
        }
      ]
    },
    {
      "name": "setConfig",
      "discriminator": [
        108,
        158,
        154,
        175,
        212,
        98,
        52,
        66
      ],
      "accounts": [
        {
          "name": "admin",
          "signer": true
        },
        {
          "name": "config",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  99,
                  111,
                  110,
                  102,
                  105,
                  103
                ]
              }
            ]
          }
        }
      ],
      "args": [
        {
          "name": "admin",
          "type": "pubkey"
        },
        {
          "name": "keeper",
          "type": "pubkey"
        },
        {
          "name": "swapProgram",
          "type": "pubkey"
        },
        {
          "name": "paused",
          "type": "bool"
        }
      ]
    },
    {
      "name": "setDepositsOpen",
      "docs": [
        "The keeper's funding guard (or the admin): open or close a vault's deposits."
      ],
      "discriminator": [
        91,
        221,
        147,
        146,
        85,
        176,
        56,
        189
      ],
      "accounts": [
        {
          "name": "signer",
          "docs": [
            "The keeper or the admin."
          ],
          "signer": true
        },
        {
          "name": "config",
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  99,
                  111,
                  110,
                  102,
                  105,
                  103
                ]
              }
            ]
          }
        },
        {
          "name": "basisVault",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  98,
                  97,
                  115,
                  105,
                  115,
                  95,
                  118,
                  97,
                  117,
                  108,
                  116
                ]
              },
              {
                "kind": "account",
                "path": "basisVault.assetMint",
                "account": "basisVault"
              }
            ]
          }
        }
      ],
      "args": [
        {
          "name": "open",
          "type": "bool"
        }
      ]
    },
    {
      "name": "updateVault",
      "discriminator": [
        67,
        229,
        185,
        188,
        226,
        11,
        210,
        60
      ],
      "accounts": [
        {
          "name": "admin",
          "signer": true
        },
        {
          "name": "config",
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  99,
                  111,
                  110,
                  102,
                  105,
                  103
                ]
              }
            ]
          }
        },
        {
          "name": "basisVault",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  98,
                  97,
                  115,
                  105,
                  115,
                  95,
                  118,
                  97,
                  117,
                  108,
                  116
                ]
              },
              {
                "kind": "account",
                "path": "basisVault.assetMint",
                "account": "basisVault"
              }
            ]
          }
        }
      ],
      "args": [
        {
          "name": "params",
          "type": {
            "defined": {
              "name": "vaultParams"
            }
          }
        }
      ]
    },
    {
      "name": "withdraw",
      "discriminator": [
        183,
        18,
        70,
        156,
        148,
        109,
        161,
        34
      ],
      "accounts": [
        {
          "name": "user",
          "docs": [
            "Receives the SOL vault's payout as lamports."
          ],
          "writable": true,
          "signer": true
        },
        {
          "name": "core",
          "accounts": [
            {
              "name": "config",
              "pda": {
                "seeds": [
                  {
                    "kind": "const",
                    "value": [
                      99,
                      111,
                      110,
                      102,
                      105,
                      103
                    ]
                  }
                ]
              }
            },
            {
              "name": "basisVault",
              "docs": [
                "A vault of this program: only `create_vault` makes one, at its asset's PDA, so the",
                "owner and type checks are enough (re-deriving the PDA here would overflow the stack).",
                "Writable for the keeper's daily rebalancing count."
              ],
              "writable": true
            },
            {
              "name": "authority",
              "writable": true
            },
            {
              "name": "shareMint",
              "writable": true
            },
            {
              "name": "usdcMint"
            },
            {
              "name": "phoenixMint",
              "writable": true
            },
            {
              "name": "vaultUsdc",
              "writable": true
            },
            {
              "name": "vaultPhoenix",
              "writable": true
            },
            {
              "name": "vaultAsset",
              "writable": true,
              "optional": true
            },
            {
              "name": "assetMint",
              "docs": [
                "is read from it."
              ]
            },
            {
              "name": "assetTokenProgram"
            },
            {
              "name": "traderAccount",
              "writable": true
            },
            {
              "name": "phoenix",
              "accounts": [
                {
                  "name": "phoenixProgram",
                  "address": "EtrnLzgbS7nMMy5fbD42kXiUzGg8XQzJ972Xtk1cjWih"
                },
                {
                  "name": "logAuthority",
                  "address": "GdxfTLSsdSY37G6fZoYtdGDSfgFnbT2EmRpuePZxWShS"
                },
                {
                  "name": "globalConfig",
                  "writable": true,
                  "address": "2zskx2iyCvb6Stg7RBZkt1f6MrF4dpYtMG3yMvKwqtUZ"
                },
                {
                  "name": "hawkeyeProgram",
                  "address": "RiSeVw3ZjNfsaXPRb4mgaqYaEEt41pNNJoDvVh7pgQj"
                },
                {
                  "name": "emberProgram",
                  "address": "EMBERpYNE6ehWmXymZZS2skiFmCa9V5dp14e1iduM5qy"
                },
                {
                  "name": "emberState"
                },
                {
                  "name": "emberVault",
                  "writable": true
                }
              ]
            },
            {
              "name": "perpAssetMap",
              "writable": true
            },
            {
              "name": "orderbook",
              "writable": true
            },
            {
              "name": "splineCollection",
              "writable": true
            },
            {
              "name": "withdrawQueue",
              "writable": true
            },
            {
              "name": "globalVault",
              "writable": true
            }
          ]
        },
        {
          "name": "userShares",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "account",
                "path": "user"
              },
              {
                "kind": "const",
                "value": [
                  6,
                  221,
                  246,
                  225,
                  215,
                  101,
                  161,
                  147,
                  217,
                  203,
                  225,
                  70,
                  206,
                  235,
                  121,
                  172,
                  28,
                  180,
                  133,
                  237,
                  95,
                  91,
                  55,
                  145,
                  58,
                  140,
                  245,
                  133,
                  126,
                  255,
                  0,
                  169
                ]
              },
              {
                "kind": "account",
                "path": "core.shareMint",
                "account": "vaultCore"
              }
            ],
            "program": {
              "kind": "const",
              "value": [
                140,
                151,
                37,
                143,
                78,
                36,
                137,
                241,
                187,
                61,
                16,
                41,
                20,
                142,
                13,
                131,
                11,
                90,
                19,
                153,
                218,
                255,
                16,
                132,
                4,
                142,
                123,
                216,
                219,
                233,
                248,
                89
              ]
            }
          }
        },
        {
          "name": "userAsset",
          "docs": [
            "Receives a spot vault's asset."
          ],
          "writable": true,
          "optional": true
        },
        {
          "name": "userUsdc",
          "docs": [
            "Receives the USDC part of the payout."
          ],
          "writable": true,
          "optional": true
        },
        {
          "name": "swapProgram",
          "optional": true
        },
        {
          "name": "vaultWsol",
          "writable": true,
          "optional": true
        },
        {
          "name": "tokenProgram",
          "address": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        },
        {
          "name": "associatedTokenProgram",
          "address": "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
        },
        {
          "name": "systemProgram",
          "address": "11111111111111111111111111111111"
        }
      ],
      "args": [
        {
          "name": "gti",
          "type": "u8"
        },
        {
          "name": "atb",
          "type": "u8"
        },
        {
          "name": "shares",
          "type": "u64"
        },
        {
          "name": "minAssetOut",
          "type": "u64"
        },
        {
          "name": "minUsdcOut",
          "type": "u64"
        },
        {
          "name": "receive",
          "type": {
            "defined": {
              "name": "receive"
            }
          }
        },
        {
          "name": "route",
          "type": {
            "option": {
              "defined": {
                "name": "swapRoute"
              }
            }
          }
        }
      ]
    }
  ],
  "accounts": [
    {
      "name": "basisVault",
      "discriminator": [
        123,
        245,
        72,
        41,
        28,
        175,
        157,
        209
      ]
    },
    {
      "name": "config",
      "discriminator": [
        155,
        12,
        170,
        224,
        30,
        250,
        204,
        130
      ]
    }
  ],
  "events": [
    {
      "name": "deposited",
      "discriminator": [
        111,
        141,
        26,
        45,
        161,
        35,
        100,
        57
      ]
    },
    {
      "name": "depositsSet",
      "discriminator": [
        128,
        53,
        43,
        162,
        119,
        135,
        44,
        22
      ]
    },
    {
      "name": "rebalanced",
      "discriminator": [
        74,
        101,
        57,
        244,
        181,
        179,
        52,
        182
      ]
    },
    {
      "name": "swapped",
      "discriminator": [
        217,
        52,
        52,
        83,
        147,
        135,
        96,
        109
      ]
    },
    {
      "name": "vaultCreated",
      "discriminator": [
        117,
        25,
        120,
        254,
        75,
        236,
        78,
        115
      ]
    },
    {
      "name": "withdrawn",
      "discriminator": [
        20,
        89,
        223,
        198,
        194,
        124,
        219,
        13
      ]
    }
  ],
  "errors": [
    {
      "code": 6000,
      "name": "unauthorized",
      "msg": "Signer is not allowed to do this"
    },
    {
      "code": 6001,
      "name": "paused",
      "msg": "The protocol is paused"
    },
    {
      "code": 6002,
      "name": "depositsClosed",
      "msg": "Deposits into this vault are closed"
    },
    {
      "code": 6003,
      "name": "zeroAmount",
      "msg": "Amount must be greater than zero"
    },
    {
      "code": 6004,
      "name": "mathOverflow",
      "msg": "Math overflow"
    },
    {
      "code": 6005,
      "name": "divisionByZero",
      "msg": "Division by zero"
    },
    {
      "code": 6006,
      "name": "invalidMint",
      "msg": "Mint does not match the vault"
    },
    {
      "code": 6007,
      "name": "invalidTokenAccount",
      "msg": "Token account does not match the vault"
    },
    {
      "code": 6008,
      "name": "invalidPhoenixAccount",
      "msg": "Phoenix account does not match the vault"
    },
    {
      "code": 6009,
      "name": "missingPhoenixAccounts",
      "msg": "Phoenix trader index accounts are missing"
    },
    {
      "code": 6010,
      "name": "phoenixInstruction",
      "msg": "Phoenix instruction could not be built or read"
    },
    {
      "code": 6011,
      "name": "invalidPrice",
      "msg": "Phoenix returned an unusable price"
    },
    {
      "code": 6012,
      "name": "maxPositionReached",
      "msg": "The vault's position would pass its cap"
    },
    {
      "code": 6013,
      "name": "openInterestShareReached",
      "msg": "The vault's short would pass its share of the market's open interest"
    },
    {
      "code": 6014,
      "name": "depositTooLarge",
      "msg": "The deposit is larger than the vault takes at once"
    },
    {
      "code": 6015,
      "name": "marketAfterHours",
      "msg": "The market is trading after hours: deposits wait for the session"
    },
    {
      "code": 6016,
      "name": "unsupportedMint",
      "msg": "The asset's token has an extension the vault does not support"
    },
    {
      "code": 6017,
      "name": "vaultInsolvent",
      "msg": "The vault owes more than it holds: deposits would pay its debt"
    },
    {
      "code": 6018,
      "name": "rebalanceLimitReached",
      "msg": "The keeper's rebalancing would pass its daily limit"
    },
    {
      "code": 6019,
      "name": "multiplierChanging",
      "msg": "The token's multiplier is changing: deposits wait until it has settled"
    },
    {
      "code": 6020,
      "name": "depositOffMix",
      "msg": "A deposit without a swap must match the vault's asset and USDC mix"
    },
    {
      "code": 6021,
      "name": "marginTooThin",
      "msg": "The vault's margin would fall below its target"
    },
    {
      "code": 6022,
      "name": "sharesTooLow",
      "msg": "Fewer shares than the minimum"
    },
    {
      "code": 6023,
      "name": "payoutTooLow",
      "msg": "Payout is below the minimum"
    },
    {
      "code": 6024,
      "name": "nothingDeposited",
      "msg": "The deposit added no value"
    },
    {
      "code": 6025,
      "name": "nothingToWithdraw",
      "msg": "Phoenix lets nothing leave the vault right now"
    },
    {
      "code": 6026,
      "name": "notEnoughShares",
      "msg": "Not enough shares"
    },
    {
      "code": 6027,
      "name": "priceOutOfBand",
      "msg": "Trade price is outside the allowed band around the mark price"
    },
    {
      "code": 6028,
      "name": "wrongVaultKind",
      "msg": "This vault kind does not take this account"
    },
    {
      "code": 6029,
      "name": "invalidSwapAccounts",
      "msg": "Swap accounts are missing or wrong"
    },
    {
      "code": 6030,
      "name": "swapNotNeeded",
      "msg": "This deposit or withdrawal needs no swap"
    },
    {
      "code": 6031,
      "name": "swapAmountOff",
      "msg": "The swap did not spend the planned amount"
    },
    {
      "code": 6032,
      "name": "swapPriceTooLow",
      "msg": "The swap paid less than the mark price allows"
    },
    {
      "code": 6033,
      "name": "swapMovedVaultFunds",
      "msg": "The swap touched vault funds it was not given"
    },
    {
      "code": 6034,
      "name": "invalidParameter",
      "msg": "Invalid parameter"
    }
  ],
  "types": [
    {
      "name": "basisVault",
      "docs": [
        "One vault per asset. Depositors get the vault's shares; the vault holds the asset and an",
        "equal short on Phoenix, so its value is flat in USD and it earns the funding longs pay",
        "shorts. The share price is the vault's NAV over its supply. Phoenix fills market orders",
        "in the same instruction, so every deposit and withdrawal settles at once and is priced",
        "from the NAV before and after its own trade: each user carries their own trading cost."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "kind",
            "type": {
              "defined": {
                "name": "vaultKind"
              }
            }
          },
          {
            "name": "assetMint",
            "docs": [
              "The asset; the native mint for the SOL vault."
            ],
            "type": "pubkey"
          },
          {
            "name": "assetDecimals",
            "type": "u8"
          },
          {
            "name": "assetTokenProgram",
            "docs": [
              "The asset's token program: SPL Token, or Token-2022 for tokenized stocks."
            ],
            "type": "pubkey"
          },
          {
            "name": "shareMint",
            "type": "pubkey"
          },
          {
            "name": "authority",
            "type": "pubkey"
          },
          {
            "name": "usdcAccount",
            "docs": [
              "The authority's token accounts; `asset_account` is unset for the SOL vault."
            ],
            "type": "pubkey"
          },
          {
            "name": "phoenixAccount",
            "type": "pubkey"
          },
          {
            "name": "assetAccount",
            "type": "pubkey"
          },
          {
            "name": "traderAccount",
            "type": "pubkey"
          },
          {
            "name": "assetId",
            "docs": [
              "The Phoenix market the vault shorts."
            ],
            "type": "u32"
          },
          {
            "name": "orderbook",
            "type": "pubkey"
          },
          {
            "name": "splineCollection",
            "type": "pubkey"
          },
          {
            "name": "baseLotDecimals",
            "docs": [
              "Base lots per whole asset are `10^base_lot_decimals`."
            ],
            "type": "u8"
          },
          {
            "name": "tickSize",
            "docs": [
              "Quote lots per base lot per price tick."
            ],
            "type": "u64"
          },
          {
            "name": "maxPositionUsd",
            "docs": [
              "Max short notional (USD, 6 dp)."
            ],
            "type": "u64"
          },
          {
            "name": "maxAssetUnits",
            "docs": [
              "Max asset the vault holds, in its smallest units. For the SOL vault this stays under",
              "the native SOL Phoenix counts as collateral per trader (500 SOL on mainnet)."
            ],
            "type": "u64"
          },
          {
            "name": "maxOiShareBps",
            "docs": [
              "Max share of the Phoenix market's open interest the vault's short may be, in bps.",
              "Shorts earn funding only while longs outweigh them; a vault that is most of the short",
              "side pushes funding against itself."
            ],
            "type": "u16"
          },
          {
            "name": "maxDepositUsd",
            "docs": [
              "Max value a single deposit may add (USD, 6 dp): large deposits come in parts the",
              "order book can take."
            ],
            "type": "u64"
          },
          {
            "name": "slippageBps",
            "docs": [
              "Price band around Phoenix's mark price for the vault's orders and users' swaps."
            ],
            "type": "u16"
          },
          {
            "name": "keeperBandBps",
            "docs": [
              "The keeper's own, tighter band for `rebalance`, where the keeper is the vault's",
              "counterparty."
            ],
            "type": "u16"
          },
          {
            "name": "maxRebalanceUsd",
            "docs": [
              "Max value the keeper may rebalance per `REBALANCE_WINDOW_SECONDS` (USD, 6 dp): what a",
              "stolen keeper key could cost the vault is at most the band on this."
            ],
            "type": "u64"
          },
          {
            "name": "rebalanceWindowStart",
            "type": "i64"
          },
          {
            "name": "rebalanceWindowUsd",
            "type": "u64"
          },
          {
            "name": "marginBps",
            "docs": [
              "Phoenix effective collateral the vault keeps per dollar of short, in bps. Deposits",
              "and reinvesting may not take the vault below it."
            ],
            "type": "u16"
          },
          {
            "name": "targetMarginBps",
            "docs": [
              "USDC margin a single-asset deposit sets aside per dollar of asset, in bps: 10,000",
              "(1x) splits a deposit half asset, half USDC. 0 for the SOL vault, whose SOL is its own",
              "margin."
            ],
            "type": "u16"
          },
          {
            "name": "depositsOpen",
            "type": "bool"
          },
          {
            "name": "afterHoursDeposits",
            "docs": [
              "Whether deposits stay open while Phoenix trades the market after hours (stock markets",
              "on weekends and holidays, when the book is thin and the price is held near the last",
              "close) or reopens it. Withdrawals are always open."
            ],
            "type": "bool"
          },
          {
            "name": "bump",
            "type": "u8"
          },
          {
            "name": "authorityNonce",
            "docs": [
              "Part of the authority's address, picked by the admin at `create_vault` (little-endian",
              "u64), so nobody can claim the vault's Phoenix trader before the vault exists."
            ],
            "type": {
              "array": [
                "u8",
                8
              ]
            }
          },
          {
            "name": "authorityBump",
            "type": "u8"
          }
        ]
      }
    },
    {
      "name": "config",
      "docs": [
        "Protocol-wide roles and the Phoenix accounts every vault shares."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "admin",
            "type": "pubkey"
          },
          {
            "name": "keeper",
            "docs": [
              "The bot that reinvests funding and keeps each vault's margin near its target."
            ],
            "type": "pubkey"
          },
          {
            "name": "usdcMint",
            "type": "pubkey"
          },
          {
            "name": "phoenixMint",
            "docs": [
              "Phoenix's collateral token (PhUSD), minted 1:1 for USDC by Ember."
            ],
            "type": "pubkey"
          },
          {
            "name": "perpAssetMap",
            "type": "pubkey"
          },
          {
            "name": "withdrawQueue",
            "type": "pubkey"
          },
          {
            "name": "globalVault",
            "docs": [
              "Phoenix's token vault for `phoenix_mint`."
            ],
            "type": "pubkey"
          },
          {
            "name": "swapProgram",
            "docs": [
              "The swap venue the vault may route single-asset deposits and withdrawals through",
              "(Jupiter on mainnet)."
            ],
            "type": "pubkey"
          },
          {
            "name": "paused",
            "type": "bool"
          },
          {
            "name": "bump",
            "type": "u8"
          }
        ]
      }
    },
    {
      "name": "deposited",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "basisVault",
            "type": "pubkey"
          },
          {
            "name": "user",
            "type": "pubkey"
          },
          {
            "name": "assetAmount",
            "type": "u64"
          },
          {
            "name": "usdcAmount",
            "type": "u64"
          },
          {
            "name": "shares",
            "type": "u64"
          },
          {
            "name": "valueUsd",
            "docs": [
              "Value the deposit added after its own trade (USD, 6 dp)."
            ],
            "type": "u64"
          },
          {
            "name": "navBeforeUsd",
            "type": "u64"
          },
          {
            "name": "positionLots",
            "docs": [
              "The vault's position after the deposit, in base lots; negative is short."
            ],
            "type": "i64"
          }
        ]
      }
    },
    {
      "name": "depositsSet",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "basisVault",
            "type": "pubkey"
          },
          {
            "name": "open",
            "type": "bool"
          },
          {
            "name": "by",
            "docs": [
              "The keeper or the admin."
            ],
            "type": "pubkey"
          }
        ]
      }
    },
    {
      "name": "marketParams",
      "docs": [
        "The Phoenix market a vault shorts: its asset id and lot and tick sizes."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "assetId",
            "type": "u32"
          },
          {
            "name": "baseLotDecimals",
            "type": "u8"
          },
          {
            "name": "tickSize",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "rebalanced",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "basisVault",
            "type": "pubkey"
          },
          {
            "name": "buyAsset",
            "docs": [
              "True when funding USDC was reinvested into the asset."
            ],
            "type": "bool"
          },
          {
            "name": "assetAmount",
            "type": "u64"
          },
          {
            "name": "usdcAmount",
            "type": "u64"
          },
          {
            "name": "navBeforeUsd",
            "type": "u64"
          },
          {
            "name": "navAfterUsd",
            "type": "u64"
          },
          {
            "name": "positionLots",
            "type": "i64"
          }
        ]
      }
    },
    {
      "name": "receive",
      "docs": [
        "How a withdrawal is paid."
      ],
      "type": {
        "kind": "enum",
        "variants": [
          {
            "name": "both"
          },
          {
            "name": "asset"
          },
          {
            "name": "usdc"
          }
        ]
      }
    },
    {
      "name": "swapRoute",
      "docs": [
        "A venue instruction built off-chain for the vault authority."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "accounts",
            "docs": [
              "How many `remaining_accounts` after Phoenix's index accounts the route takes."
            ],
            "type": "u8"
          },
          {
            "name": "data",
            "type": "bytes"
          }
        ]
      }
    },
    {
      "name": "swapped",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "basisVault",
            "type": "pubkey"
          },
          {
            "name": "soldUsdc",
            "docs": [
              "True when the vault sold USDC for the asset."
            ],
            "type": "bool"
          },
          {
            "name": "spent",
            "type": "u64"
          },
          {
            "name": "received",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "vaultCreated",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "basisVault",
            "type": "pubkey"
          },
          {
            "name": "assetMint",
            "type": "pubkey"
          },
          {
            "name": "shareMint",
            "type": "pubkey"
          },
          {
            "name": "traderAccount",
            "type": "pubkey"
          },
          {
            "name": "assetId",
            "type": "u32"
          }
        ]
      }
    },
    {
      "name": "vaultKind",
      "docs": [
        "How a vault holds the asset it hedges."
      ],
      "type": {
        "kind": "enum",
        "variants": [
          {
            "name": "nativeSol"
          },
          {
            "name": "spot"
          }
        ]
      }
    },
    {
      "name": "vaultParams",
      "docs": [
        "A vault's caps and trading settings; see `BasisVault` for each."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "maxPositionUsd",
            "type": "u64"
          },
          {
            "name": "maxAssetUnits",
            "type": "u64"
          },
          {
            "name": "maxOiShareBps",
            "type": "u16"
          },
          {
            "name": "maxDepositUsd",
            "type": "u64"
          },
          {
            "name": "slippageBps",
            "type": "u16"
          },
          {
            "name": "keeperBandBps",
            "type": "u16"
          },
          {
            "name": "maxRebalanceUsd",
            "type": "u64"
          },
          {
            "name": "marginBps",
            "type": "u16"
          },
          {
            "name": "targetMarginBps",
            "type": "u16"
          },
          {
            "name": "depositsOpen",
            "type": "bool"
          },
          {
            "name": "afterHoursDeposits",
            "type": "bool"
          }
        ]
      }
    },
    {
      "name": "withdrawn",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "basisVault",
            "type": "pubkey"
          },
          {
            "name": "user",
            "type": "pubkey"
          },
          {
            "name": "shares",
            "type": "u64"
          },
          {
            "name": "payoutUsd",
            "type": "u64"
          },
          {
            "name": "assetOut",
            "type": "u64"
          },
          {
            "name": "usdcOut",
            "type": "u64"
          },
          {
            "name": "positionLots",
            "type": "i64"
          }
        ]
      }
    }
  ]
};
