"""One background job at a time, as an explicit state machine.

    idle --start--> running --work returned--> done
                            --work raised----> failed
    done / failed --start--> running

Every change of state goes through `_move`, which refuses a transition the table does not list.
"""

import threading
import traceback
from enum import StrEnum


class State(StrEnum):
    IDLE = "idle"
    RUNNING = "running"
    DONE = "done"
    FAILED = "failed"


TRANSITIONS = {
    State.IDLE: {State.RUNNING},
    State.RUNNING: {State.DONE, State.FAILED},
    State.DONE: {State.RUNNING},
    State.FAILED: {State.RUNNING},
}


class Busy(Exception):
    """A job is already running."""


class Job:
    def __init__(self):
        self._lock = threading.Lock()
        self.state = State.IDLE
        self.log, self.result, self.error = [], None, None

    def _move(self, to: State, **fields):
        with self._lock:
            if to not in TRANSITIONS[self.state]:
                raise Busy() if to is State.RUNNING else RuntimeError(f"job cannot go {self.state.value} -> {to.value}")
            self.state = to
            for name, value in fields.items():
                setattr(self, name, value)

    def start(self, work):
        """Run `work(log)` on a thread; its return value becomes the job's result.  Raises Busy while running."""
        self._move(State.RUNNING, log=[], result=None, error=None)

        def body():
            try:
                self._move(State.DONE, result=work(lambda line: self.log.append(str(line))))
            except Exception as error:  # noqa: BLE001
                self.log.append("ERROR: " + traceback.format_exc())
                self._move(State.FAILED, error=str(error))

        threading.Thread(target=body, daemon=True).start()

    def snapshot(self, since=0) -> dict:
        return {
            "state": self.state.value,
            "result": self.result,
            "error": self.error,
            "log": self.log[since:],
            "total": len(self.log),
        }
