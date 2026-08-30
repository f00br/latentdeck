"""Explicit raw-latent to LC batch conversion surface."""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from pathlib import Path

from . import CartridgeError, inspect, pack_raw_h3

MAX_CONVERSION_INPUTS = 4096


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="latentdeck-convert",
        description="Convert existing raw H3 Safetensors files into validated LC cartridges.",
    )
    parser.add_argument("inputs", nargs="+", type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--recursive", action="store_true")
    return parser


def _conversion_plan(
    inputs: list[Path], output_dir: Path, *, recursive: bool
) -> list[tuple[Path, Path]]:
    plan: list[tuple[Path, Path]] = []
    for provided in inputs:
        source = provided.resolve()
        if source.is_dir():
            candidates = (
                source.rglob("*.safetensors")
                if recursive
                else source.glob("*.safetensors")
            )
            for candidate in sorted(candidates, key=lambda path: str(path).casefold()):
                relative = candidate.relative_to(source).with_suffix(".lc")
                plan.append((candidate.resolve(), output_dir / relative))
        else:
            plan.append((source, output_dir / f"{source.stem}.lc"))
        if len(plan) > MAX_CONVERSION_INPUTS:
            raise ValueError(f"conversion input limit is {MAX_CONVERSION_INPUTS} files")
    return plan


def convert_main(argv: Sequence[str] | None = None) -> int:
    """Convert explicitly named raw H3 inputs and emit one bounded JSON report."""

    arguments = _parser().parse_args(argv)
    output_dir = arguments.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    try:
        plan = _conversion_plan(arguments.inputs, output_dir, recursive=arguments.recursive)
    except ValueError as error:
        json.dump(
            {"status": "error", "code": "input_limit", "detail": str(error)},
            sys.stdout,
            indent=2,
        )
        print()
        return 2
    if not plan:
        json.dump(
            {
                "status": "error",
                "code": "no_inputs",
                "detail": "no .safetensors files were selected for conversion",
            },
            sys.stdout,
            indent=2,
        )
        print()
        return 2
    seen_outputs: dict[str, Path] = {}
    for source, output in plan:
        key = str(output.resolve()).casefold()
        previous = seen_outputs.get(key)
        if previous is not None:
            json.dump(
                {
                    "status": "error",
                    "code": "output_collision",
                    "detail": "multiple raw inputs resolve to the same LC output",
                    "sources": [str(previous), str(source)],
                    "output": str(output.resolve()),
                },
                sys.stdout,
                indent=2,
            )
            print()
            return 2
        seen_outputs[key] = source
    items: list[dict[str, object]] = []
    failed = 0
    for source, output in plan:
        output.parent.mkdir(parents=True, exist_ok=True)
        try:
            receipt = pack_raw_h3(
                source,
                output,
                provenance={
                    "created_by": {"name": "latentdeck-convert", "version": "0.1.0"},
                    "source_kind": "raw_h3_safetensors",
                },
            )
            inspection = inspect(output)
            items.append(
                {
                    "status": "ok",
                    "source": str(source),
                    "output": str(output),
                    "validation": receipt["validation"],
                    "cartridge_id": inspection["manifest"]["cartridge_id"],
                    "profile": inspection["profile"],
                }
            )
        except CartridgeError as error:
            failed += 1
            items.append(
                {
                    "status": "error",
                    "source": str(source),
                    "output": str(output),
                    "code": error.code,
                    "detail": error.detail,
                }
            )
    converted = len(items) - failed
    json.dump(
        {
            "status": "ok" if failed == 0 else "error",
            "converted": converted,
            "failed": failed,
            "items": items,
        },
        sys.stdout,
        indent=2,
    )
    print()
    return 0 if failed == 0 else 3


if __name__ == "__main__":
    raise SystemExit(convert_main())
