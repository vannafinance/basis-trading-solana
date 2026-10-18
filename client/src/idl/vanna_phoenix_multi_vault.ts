/**
 * Program IDL in camelCase format in order to be used in JS/TS.
 *
 * Note that this is only a type helper and is not the actual IDL. The original
 * IDL can be found at `target/idl/vanna_phoenix_multi_vault.json`.
 */
export type VannaPhoenixMultiVault = {
  "address": "ppTyvPjN6xCEvCX7zsjPBip393fVVm8RMvkiU3Lz9WA",
  "metadata": {
    "name": "vannaPhoenixMultiVault",
    "version": "0.1.0",
    "spec": "0.1.0",
    "description": "Vanna multi-strategy basis vault on Solana: one USDC vault whose keeper spreads it over hedged Phoenix perp markets by funding and open interest"
  },
  "instructions": [
    {
      "name": "addLeg",
      "discriminator": [
        145,
        145,
        38,
        60,
        188,
        195,
        243,
        248
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
          "name": "vault",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  118,
                  97,
                  117,
                  108,
                  116
                ]
              },
              {
                "kind": "account",
                "path": "vault.id",
                "account": "vault"
              }
            ]
          }
        },
        {
          "name": "leg",
          "writable": true
        },
        {
          "name": "authority"
        },
        {
          "name": "assetMint"
        },
        {
          "name": "vaultAsset",
          "writable": true,
          "optional": true
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
          "name": "assetTokenProgram",
          "docs": [
            "Token for the SOL leg."
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
              "name": "legKind"
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
              "name": "legParams"
            }
          }
        }
      ]
    },
    {
      "name": "cancelWithdraw",
      "discriminator": [
        112,
        53,
        226,
        58,
        158,
        30,
        37,
        168
      ],
      "accounts": [
        {
          "name": "user",
          "docs": [
            "The request's user."
          ],
          "writable": true,
          "signer": true
        },
        {
          "name": "vault",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  118,
                  97,
                  117,
                  108,
                  116
                ]
              },
              {
                "kind": "account",
                "path": "vault.id",
                "account": "vault"
              }
            ]
          },
          "relations": [
            "request"
          ]
        },
        {
          "name": "authority"
        },
        {
          "name": "shareMint",
          "writable": true
        },
        {
          "name": "escrowShares",
          "writable": true
        },
        {
          "name": "userShares",
          "writable": true
        },
        {
          "name": "request",
          "writable": true
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
      "args": []
    },
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
          "name": "vault",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  118,
                  97,
                  117,
                  108,
                  116
                ]
              },
              {
                "kind": "arg",
                "path": "id"
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
                "path": "vault"
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
                "path": "vault"
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
          "name": "escrowShares",
          "writable": true
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
          "name": "id",
          "type": "u16"
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
      "name": "deploy",
      "discriminator": [
        67,
        36,
        143,
        118,
        36,
        164,
        92,
        217
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
              "name": "vault",
              "docs": [
                "A vault of this program: only `create_vault` makes one, at its id's PDA, so the owner",
                "and type checks are enough. Writable for the keeper's turnover count and request ids."
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
              "name": "vaultUsdc",
              "writable": true
            },
            {
              "name": "vaultPhoenix",
              "writable": true
            },
            {
              "name": "escrowShares",
              "writable": true
            },
            {
              "name": "traderAccount",
              "writable": true
            },
            {
              "name": "perpAssetMap",
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
                }
              ]
            }
          ]
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
          "name": "swapProgram"
        },
        {
          "name": "vaultWsol",
          "writable": true,
          "optional": true
        },
        {
          "name": "nativeMint",
          "optional": true,
          "address": "So11111111111111111111111111111111111111112"
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
          "name": "legIndex",
          "type": "u8"
        },
        {
          "name": "usdcAmount",
          "type": "u64"
        },
        {
          "name": "route",
          "type": {
            "defined": {
              "name": "swapRoute"
            }
          }
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
              "name": "vault",
              "docs": [
                "A vault of this program: only `create_vault` makes one, at its id's PDA, so the owner",
                "and type checks are enough. Writable for the keeper's turnover count and request ids."
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
              "name": "vaultUsdc",
              "writable": true
            },
            {
              "name": "vaultPhoenix",
              "writable": true
            },
            {
              "name": "escrowShares",
              "writable": true
            },
            {
              "name": "traderAccount",
              "writable": true
            },
            {
              "name": "perpAssetMap",
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
                }
              ]
            }
          ]
        },
        {
          "name": "userUsdc",
          "writable": true
        },
        {
          "name": "userShares",
          "writable": true
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
          "name": "usdcAmount",
          "type": "u64"
        },
        {
          "name": "minShares",
          "type": "u64"
        }
      ]
    },
    {
      "name": "fulfillWithdraw",
      "discriminator": [
        158,
        10,
        86,
        153,
        67,
        210,
        223,
        6
      ],
      "accounts": [
        {
          "name": "cranker",
          "docs": [
            "The keeper, which pays requests oldest first; or anyone else (the user too) once the",
            "buffer holds this request and every other live one."
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
              "name": "vault",
              "docs": [
                "A vault of this program: only `create_vault` makes one, at its id's PDA, so the owner",
                "and type checks are enough. Writable for the keeper's turnover count and request ids."
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
              "name": "vaultUsdc",
              "writable": true
            },
            {
              "name": "vaultPhoenix",
              "writable": true
            },
            {
              "name": "escrowShares",
              "writable": true
            },
            {
              "name": "traderAccount",
              "writable": true
            },
            {
              "name": "perpAssetMap",
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
                }
              ]
            }
          ]
        },
        {
          "name": "request",
          "writable": true
        },
        {
          "name": "user",
          "writable": true,
          "relations": [
            "request"
          ]
        },
        {
          "name": "userUsdc",
          "docs": [
            "The user's USDC account: their associated token account, still theirs. One they closed,",
            "gave away or had frozen is not paid into, or created at the cranker's cost: the keeper",
            "returns the request instead (`return_withdraw`)."
          ],
          "writable": true
        },
        {
          "name": "tokenProgram",
          "address": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
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
          "address": "ppTyvPjN6xCEvCX7zsjPBip393fVVm8RMvkiU3Lz9WA"
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
      "name": "moveMargin",
      "discriminator": [
        179,
        87,
        166,
        207,
        7,
        135,
        185,
        3
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
              "name": "vault",
              "docs": [
                "A vault of this program: only `create_vault` makes one, at its id's PDA, so the owner",
                "and type checks are enough. Writable for the keeper's turnover count and request ids."
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
              "name": "vaultUsdc",
              "writable": true
            },
            {
              "name": "vaultPhoenix",
              "writable": true
            },
            {
              "name": "escrowShares",
              "writable": true
            },
            {
              "name": "traderAccount",
              "writable": true
            },
            {
              "name": "perpAssetMap",
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
                }
              ]
            }
          ]
        },
        {
          "name": "ember",
          "accounts": [
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
          "name": "phoenixMint",
          "writable": true
        },
        {
          "name": "withdrawQueue",
          "writable": true
        },
        {
          "name": "globalVault",
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
          "name": "toPhoenix",
          "type": "bool"
        },
        {
          "name": "amount",
          "type": "u64"
        }
      ]
    },
    {
      "name": "rehedge",
      "discriminator": [
        65,
        171,
        132,
        247,
        160,
        23,
        247,
        91
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
              "name": "vault",
              "docs": [
                "A vault of this program: only `create_vault` makes one, at its id's PDA, so the owner",
                "and type checks are enough. Writable for the keeper's turnover count and request ids."
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
              "name": "vaultUsdc",
              "writable": true
            },
            {
              "name": "vaultPhoenix",
              "writable": true
            },
            {
              "name": "escrowShares",
              "writable": true
            },
            {
              "name": "traderAccount",
              "writable": true
            },
            {
              "name": "perpAssetMap",
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
                }
              ]
            }
          ]
        },
        {
          "name": "orderbook",
          "writable": true
        },
        {
          "name": "splineCollection",
          "writable": true
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
          "name": "legIndex",
          "type": "u8"
        }
      ]
    },
    {
      "name": "requestWithdraw",
      "discriminator": [
        137,
        95,
        187,
        96,
        250,
        138,
        31,
        182
      ],
      "accounts": [
        {
          "name": "user",
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
          "name": "vault",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  118,
                  97,
                  117,
                  108,
                  116
                ]
              },
              {
                "kind": "account",
                "path": "vault.id",
                "account": "vault"
              }
            ]
          }
        },
        {
          "name": "escrowShares",
          "writable": true
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
                "path": "vault.shareMint",
                "account": "vault"
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
          "name": "request",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  119,
                  105,
                  116,
                  104,
                  100,
                  114,
                  97,
                  119,
                  95,
                  114,
                  101,
                  113,
                  117,
                  101,
                  115,
                  116
                ]
              },
              {
                "kind": "account",
                "path": "vault"
              },
              {
                "kind": "account",
                "path": "vault.nextRequestId",
                "account": "vault"
              }
            ]
          }
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
          "name": "shares",
          "type": "u64"
        }
      ]
    },
    {
      "name": "retireLeg",
      "discriminator": [
        115,
        120,
        179,
        141,
        199,
        22,
        232,
        5
      ],
      "accounts": [
        {
          "name": "admin",
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
              "name": "vault",
              "docs": [
                "A vault of this program: only `create_vault` makes one, at its id's PDA, so the owner",
                "and type checks are enough. Writable for the keeper's turnover count and request ids."
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
              "name": "vaultUsdc",
              "writable": true
            },
            {
              "name": "vaultPhoenix",
              "writable": true
            },
            {
              "name": "escrowShares",
              "writable": true
            },
            {
              "name": "traderAccount",
              "writable": true
            },
            {
              "name": "perpAssetMap",
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
                }
              ]
            }
          ]
        },
        {
          "name": "leg",
          "writable": true
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
          "name": "maxUnits",
          "type": "u64"
        }
      ]
    },
    {
      "name": "returnWithdraw",
      "docs": [
        "The keeper (or the admin) gives up on a request it cannot pay; its user takes the",
        "shares back with `cancel_withdraw`."
      ],
      "discriminator": [
        2,
        100,
        141,
        199,
        199,
        83,
        44,
        211
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
          "name": "vault",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  118,
                  97,
                  117,
                  108,
                  116
                ]
              },
              {
                "kind": "account",
                "path": "vault.id",
                "account": "vault"
              }
            ]
          },
          "relations": [
            "request"
          ]
        },
        {
          "name": "request",
          "writable": true
        }
      ],
      "args": []
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
        "The keeper's funding guard (or the admin): open or close the vault's deposits."
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
          "name": "vault",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  118,
                  97,
                  117,
                  108,
                  116
                ]
              },
              {
                "kind": "account",
                "path": "vault.id",
                "account": "vault"
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
      "name": "unwind",
      "discriminator": [
        66,
        99,
        1,
        75,
        47,
        72,
        63,
        85
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
              "name": "vault",
              "docs": [
                "A vault of this program: only `create_vault` makes one, at its id's PDA, so the owner",
                "and type checks are enough. Writable for the keeper's turnover count and request ids."
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
              "name": "vaultUsdc",
              "writable": true
            },
            {
              "name": "vaultPhoenix",
              "writable": true
            },
            {
              "name": "escrowShares",
              "writable": true
            },
            {
              "name": "traderAccount",
              "writable": true
            },
            {
              "name": "perpAssetMap",
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
                }
              ]
            }
          ]
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
          "name": "swapProgram"
        },
        {
          "name": "vaultWsol",
          "writable": true,
          "optional": true
        },
        {
          "name": "nativeMint",
          "optional": true,
          "address": "So11111111111111111111111111111111111111112"
        },
        {
          "name": "withdrawQueue",
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
          "name": "legIndex",
          "type": "u8"
        },
        {
          "name": "assetAmount",
          "type": "u64"
        },
        {
          "name": "route",
          "type": {
            "defined": {
              "name": "swapRoute"
            }
          }
        }
      ]
    },
    {
      "name": "updateLeg",
      "discriminator": [
        179,
        248,
        82,
        30,
        65,
        142,
        149,
        244
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
          "name": "vault",
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  118,
                  97,
                  117,
                  108,
                  116
                ]
              },
              {
                "kind": "account",
                "path": "vault.id",
                "account": "vault"
              }
            ]
          }
        },
        {
          "name": "leg",
          "writable": true
        }
      ],
      "args": [
        {
          "name": "params",
          "type": {
            "defined": {
              "name": "legParams"
            }
          }
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
          "name": "vault",
          "writable": true,
          "pda": {
            "seeds": [
              {
                "kind": "const",
                "value": [
                  118,
                  97,
                  117,
                  108,
                  116
                ]
              },
              {
                "kind": "account",
                "path": "vault.id",
                "account": "vault"
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
              "name": "vault",
              "docs": [
                "A vault of this program: only `create_vault` makes one, at its id's PDA, so the owner",
                "and type checks are enough. Writable for the keeper's turnover count and request ids."
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
              "name": "vaultUsdc",
              "writable": true
            },
            {
              "name": "vaultPhoenix",
              "writable": true
            },
            {
              "name": "escrowShares",
              "writable": true
            },
            {
              "name": "traderAccount",
              "writable": true
            },
            {
              "name": "perpAssetMap",
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
                }
              ]
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
          "name": "userUsdc",
          "writable": true
        },
        {
          "name": "tokenProgram",
          "address": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
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
          "name": "minUsdcOut",
          "type": "u64"
        }
      ]
    }
  ],
  "accounts": [
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
    },
    {
      "name": "leg",
      "discriminator": [
        122,
        171,
        35,
        39,
        138,
        18,
        32,
        45
      ]
    },
    {
      "name": "vault",
      "discriminator": [
        211,
        8,
        232,
        43,
        2,
        152,
        117,
        119
      ]
    },
    {
      "name": "withdrawRequest",
      "discriminator": [
        186,
        239,
        174,
        191,
        189,
        13,
        47,
        196
      ]
    }
  ],
  "events": [
    {
      "name": "deployed",
      "discriminator": [
        50,
        145,
        94,
        59,
        169,
        255,
        159,
        186
      ]
    },
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
      "name": "legAdded",
      "discriminator": [
        113,
        244,
        89,
        107,
        100,
        42,
        26,
        194
      ]
    },
    {
      "name": "legRetired",
      "discriminator": [
        175,
        248,
        117,
        175,
        73,
        206,
        183,
        248
      ]
    },
    {
      "name": "marginMoved",
      "discriminator": [
        127,
        154,
        161,
        153,
        223,
        53,
        127,
        33
      ]
    },
    {
      "name": "rehedged",
      "discriminator": [
        161,
        204,
        207,
        230,
        152,
        94,
        223,
        73
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
      "name": "unwound",
      "discriminator": [
        122,
        36,
        236,
        137,
        74,
        98,
        38,
        164
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
      "name": "withdrawCancelled",
      "discriminator": [
        162,
        153,
        181,
        47,
        154,
        132,
        183,
        117
      ]
    },
    {
      "name": "withdrawRequested",
      "discriminator": [
        114,
        16,
        240,
        206,
        93,
        128,
        151,
        39
      ]
    },
    {
      "name": "withdrawReturned",
      "discriminator": [
        185,
        33,
        114,
        18,
        152,
        199,
        226,
        112
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
      "msg": "Mint does not match"
    },
    {
      "code": 6007,
      "name": "invalidTokenAccount",
      "msg": "Token account does not match"
    },
    {
      "code": 6008,
      "name": "invalidPhoenixAccount",
      "msg": "Phoenix account does not match"
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
      "name": "invalidLegAccounts",
      "msg": "Every leg of the vault must be passed, in order, with its accounts"
    },
    {
      "code": 6013,
      "name": "tooManyLegs",
      "msg": "The vault has as many legs as it may"
    },
    {
      "code": 6014,
      "name": "duplicateLeg",
      "msg": "A leg already trades this market or holds this asset"
    },
    {
      "code": 6015,
      "name": "legInactive",
      "msg": "The leg is not active: the keeper may only unwind it"
    },
    {
      "code": 6016,
      "name": "legRetired",
      "msg": "The leg is retired"
    },
    {
      "code": 6017,
      "name": "legNotFlat",
      "msg": "Only a leg holding nothing and short nothing can be retired"
    },
    {
      "code": 6018,
      "name": "maxPositionReached",
      "msg": "The leg's short would pass its cap"
    },
    {
      "code": 6019,
      "name": "openInterestShareReached",
      "msg": "The leg's short would pass its share of the market's open interest"
    },
    {
      "code": 6020,
      "name": "legShareReached",
      "msg": "The leg would pass its share of the vault's value"
    },
    {
      "code": 6021,
      "name": "maxTvlReached",
      "msg": "The vault would pass its total value cap"
    },
    {
      "code": 6022,
      "name": "depositTooLarge",
      "msg": "The deposit is larger than the vault takes at once"
    },
    {
      "code": 6023,
      "name": "marketAfterHours",
      "msg": "The market is trading after hours"
    },
    {
      "code": 6024,
      "name": "unsupportedMint",
      "msg": "The asset's token has an extension the vault does not support"
    },
    {
      "code": 6025,
      "name": "vaultInsolvent",
      "msg": "The vault owes more than it holds"
    },
    {
      "code": 6026,
      "name": "multiplierChanging",
      "msg": "A token the vault holds is changing its multiplier: wait until it has settled"
    },
    {
      "code": 6027,
      "name": "marginTooThin",
      "msg": "The vault's margin would fall below its target"
    },
    {
      "code": 6028,
      "name": "bufferTooThin",
      "msg": "The keeper would take the vault's USDC below its buffer and pending withdrawals"
    },
    {
      "code": 6029,
      "name": "turnoverLimitReached",
      "msg": "The keeper's trading would pass its daily limit"
    },
    {
      "code": 6030,
      "name": "sharesTooLow",
      "msg": "Fewer shares than the minimum"
    },
    {
      "code": 6031,
      "name": "payoutTooLow",
      "msg": "Payout is below the minimum"
    },
    {
      "code": 6032,
      "name": "nothingDeposited",
      "msg": "The deposit added no value"
    },
    {
      "code": 6033,
      "name": "nothingToWithdraw",
      "msg": "Nothing to withdraw"
    },
    {
      "code": 6034,
      "name": "notEnoughLiquidity",
      "msg": "Not enough USDC in the vault's buffer: request the withdrawal instead"
    },
    {
      "code": 6035,
      "name": "notEnoughShares",
      "msg": "Not enough shares"
    },
    {
      "code": 6036,
      "name": "notEnoughAsset",
      "msg": "Not enough of the asset"
    },
    {
      "code": 6037,
      "name": "wrongLegKind",
      "msg": "This leg kind does not take this account"
    },
    {
      "code": 6038,
      "name": "invalidSwapAccounts",
      "msg": "Swap accounts are missing or wrong"
    },
    {
      "code": 6039,
      "name": "unsupportedRoute",
      "msg": "The swap route is not a Jupiter route the vault can size"
    },
    {
      "code": 6040,
      "name": "swapAmountOff",
      "msg": "The swap did not spend the planned amount"
    },
    {
      "code": 6041,
      "name": "swapPriceTooLow",
      "msg": "The swap paid less than the mark price allows"
    },
    {
      "code": 6042,
      "name": "swapMovedVaultFunds",
      "msg": "The swap touched vault funds it was not given"
    },
    {
      "code": 6043,
      "name": "invalidParameter",
      "msg": "Invalid parameter"
    },
    {
      "code": 6044,
      "name": "requestReturned",
      "msg": "The keeper returned this request: its user can only take the shares back"
    },
    {
      "code": 6045,
      "name": "retireActiveLeg",
      "msg": "A leg that can no longer be priced must be deactivated before it is retired"
    }
  ],
  "types": [
    {
      "name": "config",
      "docs": [
        "Protocol-wide roles and the Phoenix accounts the vault uses."
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
              "The bot that allocates the vault over its legs: it deploys, unwinds, moves margin and",
              "re-hedges, within the limits the program enforces."
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
              "The swap venue the keeper buys and sells the legs' assets through: Jupiter v6, or the",
              "default key (swaps off: no deploys or unwinds; deposits and withdrawals still work)."
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
      "name": "deployed",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "legIndex",
            "type": "u8"
          },
          {
            "name": "usdcSpent",
            "type": "u64"
          },
          {
            "name": "assetBought",
            "type": "u64"
          },
          {
            "name": "positionLots",
            "docs": [
              "The leg's position after it, in base lots; negative is short."
            ],
            "type": "i64"
          },
          {
            "name": "navBeforeUsd",
            "type": "u64"
          },
          {
            "name": "navUsd",
            "type": "u64"
          },
          {
            "name": "supply",
            "type": "u64"
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
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "user",
            "type": "pubkey"
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
              "The deposit's value to the vault after its fee (USD, 6 dp)."
            ],
            "type": "u64"
          },
          {
            "name": "navBeforeUsd",
            "type": "u64"
          },
          {
            "name": "navUsd",
            "type": "u64"
          },
          {
            "name": "supply",
            "type": "u64"
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
            "name": "vault",
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
      "name": "leg",
      "docs": [
        "One market the vault can hold: an asset and its Phoenix perp."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "index",
            "type": "u8"
          },
          {
            "name": "kind",
            "type": {
              "defined": {
                "name": "legKind"
              }
            }
          },
          {
            "name": "assetMint",
            "type": "pubkey"
          },
          {
            "name": "assetDecimals",
            "type": "u8"
          },
          {
            "name": "assetTokenProgram",
            "docs": [
              "SPL Token, or Token-2022 for tokenized stocks."
            ],
            "type": "pubkey"
          },
          {
            "name": "assetAccount",
            "docs": [
              "The authority's asset account; unset for the SOL leg (its SOL is in the trader account)."
            ],
            "type": "pubkey"
          },
          {
            "name": "assetId",
            "docs": [
              "The Phoenix market."
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
            "type": "u8"
          },
          {
            "name": "tickSize",
            "type": "u64"
          },
          {
            "name": "active",
            "docs": [
              "Whether the keeper may deploy into the leg; it may always unwind and re-hedge it."
            ],
            "type": "bool"
          },
          {
            "name": "retired",
            "docs": [
              "A flat leg the admin took out of the vault (`retire_leg`): it holds nothing, takes one",
              "account in the leg list and no Phoenix call, so its market can no longer stop the vault."
            ],
            "type": "bool"
          },
          {
            "name": "maxNotionalUsd",
            "docs": [
              "Max short notional (USD, 6 dp)."
            ],
            "type": "u64"
          },
          {
            "name": "maxAssetUnits",
            "docs": [
              "Max asset held, in its smallest units (the SOL leg stays under Phoenix's native SOL",
              "cap per trader: 500 SOL on mainnet)."
            ],
            "type": "u64"
          },
          {
            "name": "maxOiShareBps",
            "docs": [
              "Max share of the market's open interest the leg's short may be, in bps."
            ],
            "type": "u16"
          },
          {
            "name": "maxNavShareBps",
            "docs": [
              "Max share of the vault's NAV the leg's asset may be worth, in bps; 10,000 is no cap."
            ],
            "type": "u16"
          },
          {
            "name": "afterHoursTrading",
            "docs": [
              "Whether the keeper may deploy while Phoenix trades the market after hours (stocks)."
            ],
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
      "name": "legAdded",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "leg",
            "type": "pubkey"
          },
          {
            "name": "index",
            "type": "u8"
          },
          {
            "name": "assetMint",
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
      "name": "legKind",
      "docs": [
        "How a leg holds the asset it hedges."
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
      "name": "legParams",
      "docs": [
        "A leg's caps; see `Leg` for each."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "active",
            "type": "bool"
          },
          {
            "name": "maxNotionalUsd",
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
            "name": "maxNavShareBps",
            "type": "u16"
          },
          {
            "name": "afterHoursTrading",
            "type": "bool"
          }
        ]
      }
    },
    {
      "name": "legRetired",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "leg",
            "type": "pubkey"
          },
          {
            "name": "index",
            "type": "u8"
          }
        ]
      }
    },
    {
      "name": "marginMoved",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "toPhoenix",
            "docs": [
              "True when USDC moved from the buffer into Phoenix."
            ],
            "type": "bool"
          },
          {
            "name": "amount",
            "type": "u64"
          },
          {
            "name": "navUsd",
            "type": "u64"
          },
          {
            "name": "supply",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "marketParams",
      "docs": [
        "The Phoenix market a leg shorts: its asset id and lot and tick sizes."
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
      "name": "rehedged",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "legIndex",
            "type": "u8"
          },
          {
            "name": "positionLots",
            "type": "i64"
          },
          {
            "name": "navBeforeUsd",
            "type": "u64"
          },
          {
            "name": "navUsd",
            "type": "u64"
          },
          {
            "name": "supply",
            "type": "u64"
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
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "legIndex",
            "type": "u8"
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
      "name": "unwound",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "legIndex",
            "type": "u8"
          },
          {
            "name": "assetSold",
            "type": "u64"
          },
          {
            "name": "usdcReceived",
            "type": "u64"
          },
          {
            "name": "positionLots",
            "type": "i64"
          },
          {
            "name": "navBeforeUsd",
            "type": "u64"
          },
          {
            "name": "navUsd",
            "type": "u64"
          },
          {
            "name": "supply",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "vault",
      "docs": [
        "The multi-strategy vault. Users deposit and withdraw USDC and hold its shares; the keeper",
        "spreads the USDC over the vault's legs, each an asset held with an equal short on Phoenix,",
        "so the value stays flat in USD while it earns the funding longs pay shorts. Every leg's",
        "short sits in one Phoenix cross-margin trader account.",
        "",
        "The share price is the NAV over the supply, priced exactly as in the single-asset vault:",
        "`NAV = idle USDC + Σ legs (asset held × mark × multiplier) + Phoenix equity`."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "id",
            "docs": [
              "Picked by the admin at `create_vault`; part of the vault's address."
            ],
            "type": "u16"
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
              "The authority's USDC account: deposits land here, withdrawals leave from here, and the",
              "keeper deploys from it (the buffer)."
            ],
            "type": "pubkey"
          },
          {
            "name": "phoenixAccount",
            "type": "pubkey"
          },
          {
            "name": "escrowShares",
            "docs": [
              "The authority's share account, holding the shares of pending withdrawal requests."
            ],
            "type": "pubkey"
          },
          {
            "name": "traderAccount",
            "type": "pubkey"
          },
          {
            "name": "legCount",
            "docs": [
              "Legs are numbered 0..leg_count; every deposit and withdrawal passes them all."
            ],
            "type": "u8"
          },
          {
            "name": "nextRequestId",
            "type": "u64"
          },
          {
            "name": "pendingShares",
            "docs": [
              "Shares in live withdrawal requests. Only these are reserved from the buffer: shares",
              "someone sends straight to the escrow are not a request."
            ],
            "type": "u64"
          },
          {
            "name": "maxTvlUsd",
            "docs": [
              "Max NAV (USD, 6 dp) deposits may take the vault to."
            ],
            "type": "u64"
          },
          {
            "name": "maxDepositUsd",
            "docs": [
              "Max USDC one deposit may bring."
            ],
            "type": "u64"
          },
          {
            "name": "entryFeeBps",
            "docs": [
              "Fees on deposits and withdrawals, in bps. They stay in the vault: they pay for the",
              "keeper's trades that money moving in or out causes."
            ],
            "type": "u16"
          },
          {
            "name": "exitFeeBps",
            "type": "u16"
          },
          {
            "name": "bufferBps",
            "docs": [
              "Idle USDC the keeper must leave per dollar of NAV, on top of what pending withdrawal",
              "requests are worth."
            ],
            "type": "u16"
          },
          {
            "name": "marginBps",
            "docs": [
              "Phoenix effective collateral the vault keeps per dollar of total short, in bps."
            ],
            "type": "u16"
          },
          {
            "name": "slippageBps",
            "docs": [
              "Price band around Phoenix's mark for the vault's orders and swaps."
            ],
            "type": "u16"
          },
          {
            "name": "maxTurnoverUsd",
            "docs": [
              "Max value the keeper may deploy and unwind per `TURNOVER_WINDOW_SECONDS` (USD, 6 dp):",
              "bounds what churn could cost the vault."
            ],
            "type": "u64"
          },
          {
            "name": "turnoverWindowStart",
            "type": "i64"
          },
          {
            "name": "turnoverWindowUsd",
            "type": "u64"
          },
          {
            "name": "depositsOpen",
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
      "name": "vaultCreated",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "shareMint",
            "type": "pubkey"
          },
          {
            "name": "traderAccount",
            "type": "pubkey"
          }
        ]
      }
    },
    {
      "name": "vaultParams",
      "docs": [
        "The vault's caps, fees and trading limits; see `Vault` for each."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "maxTvlUsd",
            "type": "u64"
          },
          {
            "name": "maxDepositUsd",
            "type": "u64"
          },
          {
            "name": "entryFeeBps",
            "type": "u16"
          },
          {
            "name": "exitFeeBps",
            "type": "u16"
          },
          {
            "name": "bufferBps",
            "type": "u16"
          },
          {
            "name": "marginBps",
            "type": "u16"
          },
          {
            "name": "slippageBps",
            "type": "u16"
          },
          {
            "name": "maxTurnoverUsd",
            "type": "u64"
          },
          {
            "name": "depositsOpen",
            "type": "bool"
          }
        ]
      }
    },
    {
      "name": "withdrawCancelled",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "user",
            "type": "pubkey"
          },
          {
            "name": "requestId",
            "type": "u64"
          },
          {
            "name": "shares",
            "docs": [
              "Shares back to the user."
            ],
            "type": "u64"
          },
          {
            "name": "feeShares",
            "docs": [
              "Shares burned as the exit fee (none for a request the keeper returned)."
            ],
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "withdrawRequest",
      "docs": [
        "A withdrawal waiting for the keeper to raise its USDC. Its shares sit in the vault's escrow",
        "and keep earning (or losing) with everyone's until it is fulfilled at the NAV then; the",
        "user can cancel it until then."
      ],
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "user",
            "type": "pubkey"
          },
          {
            "name": "id",
            "type": "u64"
          },
          {
            "name": "shares",
            "type": "u64"
          },
          {
            "name": "createdAt",
            "type": "i64"
          },
          {
            "name": "bump",
            "type": "u8"
          },
          {
            "name": "returned",
            "docs": [
              "The keeper gave up on paying it (`return_withdraw`): its shares no longer reserve the",
              "buffer, and its user can only take them back (`cancel_withdraw`)."
            ],
            "type": "bool"
          }
        ]
      }
    },
    {
      "name": "withdrawRequested",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "user",
            "type": "pubkey"
          },
          {
            "name": "request",
            "type": "pubkey"
          },
          {
            "name": "requestId",
            "type": "u64"
          },
          {
            "name": "shares",
            "type": "u64"
          }
        ]
      }
    },
    {
      "name": "withdrawReturned",
      "type": {
        "kind": "struct",
        "fields": [
          {
            "name": "vault",
            "type": "pubkey"
          },
          {
            "name": "user",
            "type": "pubkey"
          },
          {
            "name": "requestId",
            "type": "u64"
          },
          {
            "name": "shares",
            "type": "u64"
          },
          {
            "name": "by",
            "type": "pubkey"
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
            "name": "vault",
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
            "docs": [
              "The shares' value before the fee (USD, 6 dp)."
            ],
            "type": "u64"
          },
          {
            "name": "feeUsd",
            "type": "u64"
          },
          {
            "name": "usdcOut",
            "docs": [
              "The USDC the user received."
            ],
            "type": "u64"
          },
          {
            "name": "requestId",
            "docs": [
              "The request this fulfilled; none for an instant withdrawal."
            ],
            "type": {
              "option": "u64"
            }
          },
          {
            "name": "navUsd",
            "type": "u64"
          },
          {
            "name": "supply",
            "type": "u64"
          }
        ]
      }
    }
  ]
};
