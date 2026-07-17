#!/usr/bin/env python3
"""Reusable lifecycle and HTTP JSON-RPC helpers for an ephemeral Bleavit node."""

from __future__ import annotations

import json
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


class JsonRpcError(RuntimeError):
    pass


class JsonRpcHttp:
    def __init__(self, url: str, timeout: float = 15.0):
        self.url = url
        self.timeout = timeout
        self.next_id = 1

    def call(
        self,
        method: str,
        params: list[Any] | None = None,
        timeout: float | None = None,
    ) -> Any:
        request_id = self.next_id
        self.next_id += 1
        body = json.dumps(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "method": method,
                "params": params or [],
            }
        ).encode("utf-8")
        request = urllib.request.Request(
            self.url,
            data=body,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            with urllib.request.urlopen(
                request, timeout=self.timeout if timeout is None else timeout
            ) as response:
                payload = json.load(response)
        except (OSError, urllib.error.URLError, json.JSONDecodeError) as error:
            raise JsonRpcError(f"{method} request failed: {error}") from error
        if "error" in payload:
            raise JsonRpcError(f"{method} returned JSON-RPC error: {payload['error']}")
        if "result" not in payload:
            raise JsonRpcError(f"{method} returned no result")
        return payload["result"]


def reserve_tcp_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


class NodeProcess:
    def __init__(
        self,
        binary: Path,
        chain_spec: Path,
        boot_timeout: float = 120.0,
        block_time_ms: int = 1000,
        max_boot_attempts: int = 3,
        port_reserver=reserve_tcp_port,
        launcher=subprocess.Popen,
        rpc_factory=JsonRpcHttp,
        clock=time.monotonic,
        sleeper=time.sleep,
    ):
        if max_boot_attempts < 1:
            raise ValueError("max_boot_attempts must be positive")
        self.binary = binary.resolve()
        self.chain_spec = chain_spec.resolve()
        self.boot_timeout = boot_timeout
        self.block_time_ms = block_time_ms
        self.max_boot_attempts = max_boot_attempts
        self._port_reserver = port_reserver
        self._launcher = launcher
        self._rpc_factory = rpc_factory
        self._clock = clock
        self._sleeper = sleeper
        self.port = self._port_reserver()
        self.http_url = f"http://127.0.0.1:{self.port}"
        self.ws_url = f"ws://127.0.0.1:{self.port}"
        self._temp: tempfile.TemporaryDirectory[str] | None = None
        self._log_handle = None
        self.process: subprocess.Popen[bytes] | None = None
        self.log_path: Path | None = None

    def __enter__(self) -> "NodeProcess":
        if not self.binary.is_file():
            raise FileNotFoundError(f"node binary not found: {self.binary}")
        if not self.chain_spec.is_file():
            raise FileNotFoundError(f"chain spec not found: {self.chain_spec}")
        self._temp = tempfile.TemporaryDirectory(prefix="bleavit-release-node-")
        base = Path(self._temp.name)
        self.log_path = base / "node.log"
        self._log_handle = self.log_path.open("wb")
        last_error = "node did not answer JSON-RPC"
        for attempt in range(1, self.max_boot_attempts + 1):
            if attempt > 1:
                self.port = self._port_reserver()
                self.http_url = f"http://127.0.0.1:{self.port}"
                self.ws_url = f"ws://127.0.0.1:{self.port}"
            command = [
                str(self.binary),
                "--chain",
                str(self.chain_spec),
                "--base-path",
                str(base / f"data-{attempt}"),
                "--rpc-port",
                str(self.port),
                "--rpc-methods",
                "unsafe",
                "--rpc-cors",
                "all",
                "--no-telemetry",
                "--no-prometheus",
                "--dev-block-time",
                str(self.block_time_ms),
            ]
            self.process = self._launcher(
                command,
                stdout=self._log_handle,
                stderr=subprocess.STDOUT,
            )
            client = self._rpc_factory(self.http_url, timeout=2.0)
            deadline = self._clock() + self.boot_timeout
            exited = False
            while self._clock() < deadline:
                if self.process.poll() is not None:
                    last_error = (
                        f"node exited with status {self.process.returncode} "
                        f"on boot attempt {attempt}/{self.max_boot_attempts}"
                    )
                    exited = True
                    break
                try:
                    client.call("state_getRuntimeVersion")
                    return self
                except JsonRpcError as error:
                    last_error = str(error)
                    self._sleeper(0.25)
            if not exited:
                break
            self._stop_process()
        log_tail = self._read_log_tail()
        self.__exit__(None, None, None)
        raise RuntimeError(f"node failed to boot: {last_error}\n{log_tail}")

    def _stop_process(self) -> None:
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.process = None

    def _read_log_tail(self) -> str:
        if self._log_handle is not None:
            self._log_handle.flush()
        if self.log_path is None or not self.log_path.exists():
            return "(node log unavailable)"
        return self.log_path.read_text(encoding="utf-8", errors="replace")[-8000:]

    def __exit__(self, exc_type, exc_value, traceback) -> None:
        self._stop_process()
        if self._log_handle is not None:
            self._log_handle.close()
            self._log_handle = None
        if self._temp is not None:
            self._temp.cleanup()
            self._temp = None
