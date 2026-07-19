"""Frozen executable entry point for the Agnes Python sidecar."""

from app.main import run_sidecar


if __name__ == "__main__":
    raise SystemExit(run_sidecar())
