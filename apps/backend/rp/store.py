"""Reading and writing the small files several threads and processes share (caches, settings, a run's results).

Three rules, each learned the hard way:
  * a file is replaced, never rewritten in place: it is written to a temporary file beside it and moved over the
    old one, so a reader sees the old content or the new, never half of either;
  * on Windows that move fails while anybody has the old file open (a reader of ours, a sync client, a virus
    scanner), so it is tried again for a few seconds before giving up, and the temporary file never stays behind;
  * read-change-write happens under a lock (`update_json`): one inside this process, and a lock file beside the
    data so that a second process (the pipeline run from a terminal while the app is open) waits its turn too.
"""

import json
import os
import threading
import time
from collections.abc import Callable
from contextlib import contextmanager
from pathlib import Path

_lock = threading.RLock()
PATIENCE = 5.0  # seconds to keep trying to replace a file that is open elsewhere
STALE = 60.0  # a lock file older than this was left by a process that died


def replace(path: Path, write: Callable[[Path], None]):
    """Produce the file with `write(temporary path)`, then move it into place."""
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f"{path.name}.{os.getpid()}.{threading.get_ident()}.tmp")
    try:
        write(temporary)
        deadline = time.monotonic() + PATIENCE
        while True:
            try:
                os.replace(temporary, path)
                return
            except PermissionError:  # Windows: the destination is open somewhere
                if time.monotonic() > deadline:
                    raise
                time.sleep(0.05)
    finally:
        temporary.unlink(missing_ok=True)  # gone already if the move succeeded


def write_text(path: Path, text: str):
    replace(path, lambda temporary: temporary.write_text(text, encoding="utf-8"))


def write_bytes(path: Path, content: bytes):
    replace(path, lambda temporary: temporary.write_bytes(content))


def read_json(path: Path, default=None):
    """The file's content, or `default` when there is no such file."""
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return default


def write_json(path: Path, content, indent=None):
    write_text(path, json.dumps(content, indent=indent, ensure_ascii=False))


@contextmanager
def _between_processes(path: Path):
    """Hold `<path>.lock` for the duration: created exclusively, so only one process can have it."""
    lock = path.with_name(path.name + ".lock")
    lock.parent.mkdir(parents=True, exist_ok=True)
    while True:
        try:
            os.close(os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY))
            break
        except FileExistsError:
            try:
                if time.time() - lock.stat().st_mtime > STALE:
                    lock.unlink(missing_ok=True)  # its owner is gone
                    continue
            except FileNotFoundError:
                continue  # released just now
            time.sleep(0.02)
    try:
        yield
    finally:
        lock.unlink(missing_ok=True)


def update_json(path: Path, change: Callable[[dict], None], indent=None) -> dict:
    """Read a JSON object, let `change` alter it, write it back: as one step that neither another thread nor
    another process can interleave."""
    with _lock, _between_processes(path):
        content = read_json(path, {})
        change(content)
        write_json(path, content, indent)
        return content
