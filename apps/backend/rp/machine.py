"""What this computer can do, as far as it changes how the app behaves: whether it has a graphics card PyTorch
can use.  Asked of the driver's own tool rather than of PyTorch, so that answering does not load PyTorch."""

import subprocess

_asked = {}


def gpu() -> str | None:
    """The name of the NVIDIA graphics card, or None when there is none (or its driver is not installed)."""
    if "gpu" not in _asked:
        try:
            said = subprocess.run(
                ["nvidia-smi", "--query-gpu=name", "--format=csv,noheader"],
                capture_output=True,
                text=True,
                timeout=8,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
            names = [line.strip() for line in said.stdout.splitlines() if line.strip()]
            _asked["gpu"] = names[0] if said.returncode == 0 and names else None
        except (OSError, subprocess.SubprocessError):
            _asked["gpu"] = None
    return _asked["gpu"]


def summary() -> dict:
    """{"gpu", "embedding": what that means for the slowest step, in words for the interface}."""
    name = gpu()
    return {
        "gpu": name,
        "embedding": f"Papers are embedded on the graphics card ({name}): about a thousand papers a minute."
        if name
        else "No NVIDIA graphics card was found, so papers are embedded on the processor. Everything works, but "
        "embedding is many times slower: allow about a minute per hundred papers, and prefer a smaller embedding "
        "model. Papers already embedded are never embedded again.",
    }
