"""Shared non-destructive setup for the VM regression harnesses."""

from pathlib import Path
import tempfile
import tomllib


def new_run_directory(parent):
    """Keep all existing output and create an isolated child for this run."""
    parent = Path(parent).absolute()
    if parent.is_symlink():
        raise ValueError("test output directory must not be a symlink")
    parent.mkdir(parents=True, exist_ok=True)
    return tempfile.mkdtemp(prefix="run-", dir=parent)


def package_version():
    manifest = Path(__file__).resolve().parents[1] / "Cargo.toml"
    with manifest.open("rb") as stream:
        return tomllib.load(stream)["package"]["version"]
