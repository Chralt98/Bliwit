#!/usr/bin/env python3
"""Generate a small high-precision LMSR regression corpus for futarchy-fixed.

This is an interim M2 fixture generator. It uses Python Decimal with 80 digits of
precision for deterministic values covering the normative V1-V6 table and domain
edges; the full M3 reference-model MPFR-256 >=10^7 point corpus remains the
normative completion gate tracked in PLAN.md.
"""
from __future__ import annotations

import argparse
from decimal import Decimal, getcontext
from pathlib import Path

getcontext().prec = 80
ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates" / "futarchy-fixed" / "fixtures" / "lmsr_corpus.csv"

B = Decimal(10_000)
LN2 = Decimal(2).ln()

def cost(ql: Decimal, qs: Decimal, b: Decimal = B) -> Decimal:
    m = max(ql, qs)
    d = abs(ql - qs) / b
    return m + b * (Decimal(1) + (-d).exp()).ln()

def price_long(ql: Decimal, qs: Decimal, b: Decimal = B) -> Decimal:
    d = (ql - qs) / b
    return Decimal(1) / (Decimal(1) + (-d).exp())

def disp(p_from: Decimal, p_to: Decimal, b: Decimal = B) -> Decimal:
    def logit(p: Decimal) -> Decimal:
        return (p / (Decimal(1) - p)).ln()
    return b * abs(logit(p_to) - logit(p_from))

def fmt(x: Decimal) -> str:
    return format(x, ".24f")

def raw_64x64(x: Decimal) -> int:
    return int((x * (1 << 64)).to_integral_value(rounding="ROUND_HALF_UP"))

rows: list[tuple[str, Decimal]] = []
rows.append(("cost_0_0", cost(Decimal(0), Decimal(0))))
rows.append(("v1_buy_1000_long_cost", cost(Decimal(1000), Decimal(0)) - cost(Decimal(0), Decimal(0))))
rows.append(("v2_price_after_v1", price_long(Decimal(1000), Decimal(0))))
rows.append(("v3_displace_0_5_to_0_6", disp(Decimal("0.5"), Decimal("0.6"))))
v3_delta = rows[-1][1]
rows.append(("v3_cost_0_5_to_0_6", cost(v3_delta, Decimal(0)) - cost(Decimal(0), Decimal(0))))
rows.append(("v4_worst_case_loss", B * LN2))
rows.append(("domain_edge_cost_480000_0", cost(Decimal(480000), Decimal(0))))
for ql, qs in [(2500, 0), (0, 2500), (12345, 6789), (6789, 12345), (240000, 0), (0, 240000)]:
    qld, qsd = Decimal(ql), Decimal(qs)
    rows.append((f"cost_{ql}_{qs}", cost(qld, qsd)))
    rows.append((f"price_{ql}_{qs}", price_long(qld, qsd)))

content = (
    "# name,value,raw_64x64_nearest\n"
    + "".join(f"{name},{fmt(value)},{raw_64x64(value)}\n" for name, value in rows)
)

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument(
    "--check",
    action="store_true",
    help="fail if the committed corpus differs from generated output",
)
args = parser.parse_args()

if args.check:
    current = OUT.read_text(encoding="utf-8") if OUT.exists() else ""
    if current != content:
        raise SystemExit(f"{OUT.relative_to(ROOT)} is stale; regenerate with {Path(__file__).name}")
    print(f"{OUT.relative_to(ROOT)} is up to date ({len(rows)} rows)")
else:
    OUT.write_text(content, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)} ({len(rows)} rows)")
