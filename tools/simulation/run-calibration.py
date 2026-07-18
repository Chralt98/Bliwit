#!/usr/bin/env python3
from __future__ import annotations

import argparse
from pathlib import Path
import sys
import time


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "reference-model" / "src"))
sys.path.insert(0, str(ROOT / "simulation" / "src"))

from bleavit_simulation.calibration import run_full_calibration  # noqa: E402
from bleavit_simulation.evidence import (  # noqa: E402
    check_artifact,
    write_artifact,
)


DEFAULT_ARTIFACT = ROOT / "simulation" / "results" / "phase0-calibration.json"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Bleavit doc-15 §4.9 Phase-0 economic calibration"
    )
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument(
        "--full",
        action="store_true",
        help="run >=10^4 synthetic proposals and write the evidence artifact",
    )
    mode.add_argument(
        "--check",
        action="store_true",
        help="validate the committed artifact and reproduce its pinned subsample",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_ARTIFACT,
        help="artifact path (default: simulation/results/phase0-calibration.json)",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    output = args.output.resolve()
    try:
        if args.full:
            started = time.monotonic()
            artifact = run_full_calibration()
            write_artifact(output, artifact)
            elapsed = time.monotonic() - started
            print(
                "wrote {} proposals to {} in {:.3f}s".format(
                    artifact["proposal_count"], output, elapsed
                )
            )
            if artifact["violations"]:
                print("normative violations:", file=sys.stderr)
                for violation in artifact["violations"]:
                    print(f"- {violation}", file=sys.stderr)
                return 1
            return 0
        result = check_artifact(output)
        print(
            "calibration structure OK: {} proposals; {} byte-exact subsample rows".format(
                result["checked_proposals"], result["reproduced_subsample"]
            )
        )
        if result["violations"]:
            print("normative violations:", file=sys.stderr)
            for violation in result["violations"]:
                print(f"- {violation}", file=sys.stderr)
            return 1
        return 0
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"calibration error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
