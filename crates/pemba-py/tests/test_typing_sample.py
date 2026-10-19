"""Runs the ``mypy --strict`` usage sample, so it is also correct at runtime."""

import typing_sample


def test_typing_sample_runs() -> None:
    typing_sample.main()
