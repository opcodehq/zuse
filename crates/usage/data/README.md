The bundled model-prices.json is an offline snapshot of the public LiteLLM model pricing catalogue, distributed under the accompanying MIT license. The snapshot preserves model order and the token-rate fields used by usage reports. It contains 1,313 model entries and matches the pinned UI reference catalogue. Its SHA-256 is recorded below so the fallback remains reproducible.

Production profiles use this snapshot immediately when no disk cache exists, then refresh from https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json. Cached prices remain fresh for 24 hours. A failed refresh retains the last valid catalogue.

SHA-256: `80f2e6df3d0684b567ad93ff60475de1624e3eeb65d3b77c4ea9328e76c8bdcb`
