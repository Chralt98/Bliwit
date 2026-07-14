from decimal import ROUND_DOWN, Decimal, getcontext
getcontext().prec = 80
ONE = Decimal("1")
EPS_W = Decimal("0.000000001")

# 05 §4.4: ε_W = 1e-9 (one FixedU64 base unit) keeps the settlement log finite
# for a zeroed epoch; final values live on the 1e9 FixedU64 grid.
EPSILON_W = Decimal("1e-9")
FIXED_GRID = Decimal("1e-9")

def geometric_mean(values):
    prod = Decimal(1)
    for v in values: prod *= Decimal(v)
    return prod ** (Decimal(1) / Decimal(len(values)))

def winsorize(values, lo, hi): return [min(max(Decimal(v), Decimal(lo)), Decimal(hi)) for v in values]
def minmax_normalize(value, lo, hi):
    lo=Decimal(lo); hi=Decimal(hi); value=Decimal(value)
    if hi <= lo: raise ValueError("bad range")
    return min(max((value-lo)/(hi-lo), Decimal(0)), Decimal(1))

def gate(x, lo, hi):
    """Smoothstep gate g(t) = t^2 (3 - 2t) over [lo, hi] (05 §4.4 (3))."""
    x=Decimal(x); lo=Decimal(lo); hi=Decimal(hi)
    if x <= lo: return Decimal(0)
    if x >= hi: return Decimal(1)
    t=(x-lo)/(hi-lo)
    return t*t*(Decimal(3)-Decimal(2)*t)

def settlement_score(w_next, w_next_2):
    """Scalar settlement s for an epoch-e cohort (05 §4.4 (4), 08 §8.1).

    s = GeoMean(max(W_{e+1}, ε_W), max(W_{e+2}, ε_W)), rounded down to the
    FixedU64 1e9 grid. The runtime evaluates the same statistic through the
    64.64 exp2/log2 pipeline with per-step rounding (05 §4.4); grid-level
    bit-identity against that pipeline is the A7/M3 conformance-vector
    obligation.
    """
    a = max(Decimal(w_next), EPSILON_W)
    b = max(Decimal(w_next_2), EPSILON_W)
    return (a * b).sqrt().quantize(FIXED_GRID, rounding=ROUND_DOWN)
