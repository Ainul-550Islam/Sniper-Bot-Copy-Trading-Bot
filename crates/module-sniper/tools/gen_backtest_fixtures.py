#!/usr/bin/env python3
"""Generate the golden backtest fixtures (GAP-MAP P1).

Every fixture is SYNTHETIC: a seeded xorshift64 price walk plus a
config matrix, with expected outcomes computed by this script — an
independent implementation of the same formulas the Rust simulator
uses, so `tests/backtest_golden.rs` is a real cross-check rather than
a self-confirmation.

Re-run after changing the simulator math:
    python3 crates/module-sniper/tools/gen_backtest_fixtures.py

The script is deterministic; the generated JSON must be committed.
"""

import json
import os

MASK = 0xFFFF_FFFF_FFFF_FFFF

DRIFTS = [0.006, 0.001, 0.0, -0.001, -0.004]
ENTRY_SIZES = [5_000_000, 10_000_000, 25_000_000, 50_000_000]
CONGESTIONS = [0.0, 0.2, 0.5, 0.8]

CONGESTION_FULL_TIP = 5_000_000.0
MAX_IMPACT_BPS = 5_000.0
SLIPPAGE_COST_FACTOR = 0.25
MAX_RESERVE_FRACTION = 0.05

FIXTURE_COUNT = 56
SEED = 1


class Xorshift64:
    def __init__(self, state):
        self.s = state & MASK

    def next_u64(self):
        x = self.s
        x = (x ^ ((x << 13) & MASK)) & MASK
        x = (x ^ (x >> 7)) & MASK
        x = (x ^ ((x << 17) & MASK)) & MASK
        self.s = x
        return x

    def unit(self):
        return (self.next_u64() >> 11) / float(1 << 53)


def synthetic_scenario(seed, i):
    idx = seed + i
    drift = DRIFTS[idx % len(DRIFTS)]
    entry = ENTRY_SIZES[idx % len(ENTRY_SIZES)]
    congestion = CONGESTIONS[(seed + i // 5) % len(CONGESTIONS)]
    rng = Xorshift64(max(1, (seed * 1_000_003 + i) & MASK))

    initial_reserve_sol = 30.0 + rng.unit() * 20.0
    price = 0.000_000_03 + rng.unit() * 0.000_000_05
    ticks = []
    for step in range(200):
        if step > 0:
            shock = (rng.unit() * 2.0 - 1.0) * 0.02
            price *= 1.0 + drift + shock
            if price <= 0.0:
                price = 1e-9
        drain = 1.0 - 0.1 * (step / 199.0)
        ticks.append(
            {
                "t_ms": step * 250,
                "price_sol": price,
                "quote_reserve_lamports": int(initial_reserve_sol * drain * 1e9),
            }
        )
    return {
        "label": "synthetic",
        "name": f"syn_{seed:03}_{i:04}",
        "seed": seed,
        "entry_t_ms": 0,
        "entry_quote_lamports": entry,
        "initial_reserve_sol": initial_reserve_sol,
        "congestion": congestion,
        "ticks": ticks,
    }


# (fill_config, exit_config) matrix, cycled over the 56 scenarios.
CONFIGS = [
    # A: fast + cheap tip
    ({"entry_latency_ms": 150, "slippage_pct": 10.0, "tip_lamports": 100_000},
     {"take_profit_pct": 0.5, "stop_loss_pct": 0.2, "max_hold_ms": 45_000, "exit_cost_bps": 100.0}),
    # B: defaults
    ({"entry_latency_ms": 250, "slippage_pct": 15.0, "tip_lamports": 1_000_000},
     {"take_profit_pct": 1.0, "stop_loss_pct": 0.3, "max_hold_ms": 60_000, "exit_cost_bps": 100.0}),
    # C: slow + big tip + patient exit
    ({"entry_latency_ms": 400, "slippage_pct": 20.0, "tip_lamports": 2_500_000},
     {"take_profit_pct": 2.0, "stop_loss_pct": 0.5, "max_hold_ms": 90_000, "exit_cost_bps": 150.0}),
    # D: fast + full competitive tip
    ({"entry_latency_ms": 150, "slippage_pct": 10.0, "tip_lamports": 5_000_000},
     {"take_profit_pct": 1.0, "stop_loss_pct": 0.3, "max_hold_ms": 60_000, "exit_cost_bps": 100.0}),
    # E: very slow, loose exit
    ({"entry_latency_ms": 600, "slippage_pct": 25.0, "tip_lamports": 500_000},
     {"take_profit_pct": 1.5, "stop_loss_pct": 0.4, "max_hold_ms": 120_000, "exit_cost_bps": 200.0}),
    # F: fastest, zero tip
    ({"entry_latency_ms": 100, "slippage_pct": 5.0, "tip_lamports": 0},
     {"take_profit_pct": 0.75, "stop_loss_pct": 0.25, "max_hold_ms": 30_000, "exit_cost_bps": 50.0}),
    # G: moon-shot hunt
    ({"entry_latency_ms": 250, "slippage_pct": 15.0, "tip_lamports": 100_000},
     {"take_profit_pct": 3.0, "stop_loss_pct": 0.6, "max_hold_ms": 180_000, "exit_cost_bps": 250.0}),
    # H: middle of the road
    ({"entry_latency_ms": 350, "slippage_pct": 12.0, "tip_lamports": 3_000_000},
     {"take_profit_pct": 1.2, "stop_loss_pct": 0.35, "max_hold_ms": 75_000, "exit_cost_bps": 120.0}),
]


def simulate_fill(scenario, cfg):
    required_tip = scenario["congestion"] * CONGESTION_FULL_TIP
    if cfg["tip_lamports"] < required_tip:
        return {"kind": "rejected", "reason": "outbid_in_congestion"}
    land_t = scenario["entry_t_ms"] + cfg["entry_latency_ms"]
    fill_tick = next((t for t in scenario["ticks"] if t["t_ms"] >= land_t), None)
    if fill_tick is None:
        return {"kind": "rejected", "reason": "no_tick_after_latency"}
    entry_sol = scenario["entry_quote_lamports"] / 1e9
    impact_bps = min(MAX_IMPACT_BPS, (entry_sol / scenario["initial_reserve_sol"]) * 10_000.0)
    slippage_cost_bps = max(0.0, cfg["slippage_pct"]) * 100.0 * SLIPPAGE_COST_FACTOR
    fill_price = fill_tick["price_sol"] * (1.0 + (impact_bps + slippage_cost_bps) / 10_000.0)
    fraction = min(1.0, (scenario["initial_reserve_sol"] * MAX_RESERVE_FRACTION) / entry_sol)
    cost = entry_sol * fraction
    tokens = cost / fill_price
    return {
        "kind": "filled",
        "fill_t_ms": fill_tick["t_ms"],
        "fill_price_sol": fill_price,
        "cost_sol": cost,
        "tokens_bought": tokens,
        "filled_fraction": fraction,
        "impact_bps": impact_bps,
    }


def simulate_exit(scenario, cfg, fill):
    cost = fill["cost_sol"]
    path = [t for t in scenario["ticks"] if t["t_ms"] >= fill["fill_t_ms"]]
    if not path:
        path = list(scenario["ticks"])
    stop_at = cost * (1.0 - max(0.0, cfg["stop_loss_pct"]))
    target_at = cost * (1.0 + max(0.0, cfg["take_profit_pct"]))
    for tick in path:
        gross = fill["tokens_bought"] * tick["price_sol"]
        net = gross * (1.0 - cfg["exit_cost_bps"] / 10_000.0)
        if net <= stop_at:
            return {"reason": "stop_loss", "exit_t_ms": tick["t_ms"], "exit_price_sol": tick["price_sol"],
                    "gross_sol": gross, "net_sol": net}
        if net >= target_at:
            return {"reason": "take_profit", "exit_t_ms": tick["t_ms"], "exit_price_sol": tick["price_sol"],
                    "gross_sol": gross, "net_sol": net}
        if tick["t_ms"] - scenario["entry_t_ms"] >= cfg["max_hold_ms"]:
            return {"reason": "max_hold", "exit_t_ms": tick["t_ms"], "exit_price_sol": tick["price_sol"],
                    "gross_sol": gross, "net_sol": net}
    last = path[-1]
    gross = fill["tokens_bought"] * last["price_sol"]
    net = gross * (1.0 - cfg["exit_cost_bps"] / 10_000.0)
    return {"reason": "end_of_data", "exit_t_ms": last["t_ms"], "exit_price_sol": last["price_sol"],
            "gross_sol": gross, "net_sol": net}


def main():
    out_dir = os.path.join(os.path.dirname(__file__), "..", "tests", "fixtures", "backtest")
    out_dir = os.path.normpath(out_dir)
    os.makedirs(out_dir, exist_ok=True)

    stats = {"filled": 0, "rejected": 0}
    exit_reasons = {}
    for i in range(FIXTURE_COUNT):
        scenario = synthetic_scenario(SEED, i)
        fill_cfg, exit_cfg = CONFIGS[i % len(CONFIGS)]
        fill = simulate_fill(scenario, fill_cfg)
        if fill["kind"] == "rejected":
            stats["rejected"] += 1
            expected = {"executed": False, "reject_reason": fill["reason"]}
        else:
            stats["filled"] += 1
            ex = simulate_exit(scenario, exit_cfg, fill)
            exit_reasons[ex["reason"]] = exit_reasons.get(ex["reason"], 0) + 1
            expected = {
                "executed": True,
                "fill_t_ms": fill["fill_t_ms"],
                "fill_price_sol": fill["fill_price_sol"],
                "cost_sol": fill["cost_sol"],
                "tokens_bought": fill["tokens_bought"],
                "filled_fraction": fill["filled_fraction"],
                "impact_bps": fill["impact_bps"],
                "exit_reason": ex["reason"],
                "exit_t_ms": ex["exit_t_ms"],
                "exit_price_sol": ex["exit_price_sol"],
                "gross_sol": ex["gross_sol"],
                "net_sol": ex["net_sol"],
                "pnl_sol": ex["net_sol"] - fill["cost_sol"],
            }
        fixture = {
            "label": "synthetic",
            "note": "SYNTHETIC data generated by gen_backtest_fixtures.py — "
                    "never present these numbers as real launch performance.",
            "fixture_version": 1,
            "scenario": scenario,
            "fill_config": fill_cfg,
            "exit_config": exit_cfg,
            "expected": expected,
        }
        path = os.path.join(out_dir, f"{scenario['name']}.json")
        with open(path, "w") as f:
            json.dump(fixture, f, indent=1)
            f.write("\n")

    print(f"wrote {FIXTURE_COUNT} fixtures to {out_dir}")
    print("fill stats:", stats)
    print("exit reasons:", exit_reasons)


if __name__ == "__main__":
    main()
