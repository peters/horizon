#!/usr/bin/env python3
"""Classic WebDriver protocol spike against a hosted real-device grid (#628).

Speaks W3C WebDriver over HTTPS with nothing but the standard library so the run proves
what a Horizon-internal client needs: HTTPS plus Basic authentication to one control
origin, New Session as the allocation step, navigation, script execution, screenshots,
touch actions, element interaction, provider session metadata as physical-device
evidence, and explicit release verified at the provider. Every step records a typed
outcome (passed, failed, unsupported, unknown) instead of guessing. Nothing here retries
New Session or a mutation after an ambiguous response.

Credentials come only from a netrc file (never arguments or the environment) and are
sent only to the configured hub and API origins. Session ids are treated as sensitive:
they go to the private report file, and stdout shows a short digest.
"""
from __future__ import annotations

import argparse
import base64
import dataclasses
import datetime as _dt
import hashlib
import http.client
import json
import netrc
import os
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Dict, List, Optional

REPORT_SCHEMA = "horizon.remote-browser-spike.report"
REPORT_SCHEMA_VERSION = 1
DEFAULT_HUB = "https://hub-cloud.browserstack.com/wd/hub"
DEFAULT_API = "https://api.browserstack.com"
DEFAULT_NETRC = os.path.expanduser("~/.config/horizon-dev/browserstack.netrc")
DEFAULT_URL = "https://peters.github.io/horizon-mobile-fixture/"
ALLOCATION_TIMEOUT_SECONDS = 180
COMMAND_TIMEOUT_SECONDS = 60
RELEASE_TIMEOUT_SECONDS = 30
RELEASE_POLL_SECONDS = 3
# Provider session statuses after which the device is no longer held. Release
# verification asks whether the resource is terminal, not whether the test passed.
TERMINAL_SESSION_STATUSES = frozenset({"done", "completed", "passed", "failed", "timeout", "error"})
MAX_RESPONSE_BYTES = 32 * 1024 * 1024

TARGETS: Dict[str, Dict[str, Any]] = {
    "ios": {"browserName": "Safari", "deviceName": "iPhone 16", "osVersion": "18"},
    "android": {"browserName": "Chrome", "deviceName": "Google Pixel 9", "osVersion": "16.0"},
}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Authorization must never follow a redirect to another origin."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):  # noqa: D401
        return None


@dataclasses.dataclass
class Transport:
    hub: str
    api: str
    auth_header: str
    hub_origin: str = dataclasses.field(init=False)
    api_origin: str = dataclasses.field(init=False)

    def __post_init__(self) -> None:
        for name, value in (("hub", self.hub), ("api", self.api)):
            parts = urllib.parse.urlsplit(value)
            if parts.scheme != "https":
                raise SystemExit(f"{name} must be https, got {parts.scheme}")
            if parts.username or parts.password or parts.query:
                raise SystemExit(f"{name} must not carry userinfo or a query string")
        self.hub_origin = _origin(self.hub)
        self.api_origin = _origin(self.api)
        self.opener = urllib.request.build_opener(NoRedirect())

    def request(self, url: str, method: str, body: Optional[dict], timeout: float) -> Dict[str, Any]:
        origin = _origin(url)
        if origin not in (self.hub_origin, self.api_origin):
            raise RuntimeError(f"refusing to send credentials to {origin}")
        data = None if body is None else json.dumps(body).encode()
        headers = {"Authorization": self.auth_header, "Accept": "application/json"}
        if data is not None:
            headers["Content-Type"] = "application/json; charset=utf-8"
        req = urllib.request.Request(url, data=data, method=method, headers=headers)
        started = time.monotonic()
        # Any failure before a complete response body is a no-status result: the
        # caller cannot know whether the server acted, so it must not assume failure.
        try:
            try:
                with self.opener.open(req, timeout=timeout) as resp:
                    raw = resp.read(MAX_RESPONSE_BYTES + 1)
                    status = resp.status
            except urllib.error.HTTPError as err:
                raw = err.read(MAX_RESPONSE_BYTES + 1)
                status = err.code
        except (urllib.error.URLError, http.client.HTTPException, TimeoutError, OSError) as err:
            return {"status": None, "error": type(err).__name__, "detail": str(err)[:200],
                    "elapsed_ms": _ms(started)}
        if len(raw) > MAX_RESPONSE_BYTES:
            return {"status": status, "error": "oversized_response", "elapsed_ms": _ms(started)}
        try:
            parsed = json.loads(raw.decode("utf-8")) if raw else {}
        except ValueError:
            parsed = {"malformed": raw[:120].decode("utf-8", "replace")}
        return {"status": status, "body": parsed, "elapsed_ms": _ms(started)}


def _origin(url: str) -> str:
    parts = urllib.parse.urlsplit(url)
    return f"{parts.scheme}://{parts.netloc}".lower()


def _ms(started: float) -> int:
    return int((time.monotonic() - started) * 1000)


def load_auth(path: str, host: str) -> str:
    """Read one Basic credential from a private netrc file.

    An explicit path bypasses Python's own ownership and mode check, so the
    file is rejected here when any group or other permission bit is set.
    """
    try:
        mode = os.stat(path).st_mode
    except OSError as err:
        raise SystemExit(f"netrc unusable: {type(err).__name__}") from None
    if mode & 0o077:
        raise SystemExit("netrc unusable: file must not be readable or writable by group or others (chmod 600)")
    try:
        entry = netrc.netrc(path).authenticators(host)
    except (OSError, netrc.NetrcParseError) as err:
        raise SystemExit(f"netrc unusable: {type(err).__name__}") from None
    if entry is None or not entry[0] or not entry[2]:
        raise SystemExit(f"netrc has no complete entry for {host}")
    token = base64.b64encode(f"{entry[0]}:{entry[2]}".encode()).decode()
    return f"Basic {token}"


@dataclasses.dataclass
class Step:
    name: str
    outcome: str
    elapsed_ms: int
    detail: Dict[str, Any] = dataclasses.field(default_factory=dict)


class Spike:
    def __init__(self, transport: Transport, out_dir: str, target: str, url: str, build: str) -> None:
        self.t = transport
        self.out_dir = out_dir
        self.target = target
        self.url = url
        self.build = build
        self.session_id: Optional[str] = None
        self.steps: List[Step] = []
        self.provider: Dict[str, Any] = {}

    # --- protocol helpers -------------------------------------------------
    def _session_url(self, suffix: str) -> str:
        if self.session_id is None:
            raise RuntimeError("no session")
        return f"{self.t.hub}/session/{urllib.parse.quote(self.session_id, safe='')}/{suffix}"

    def cmd(self, method: str, suffix: str, body: Optional[dict] = None,
            timeout: float = COMMAND_TIMEOUT_SECONDS) -> Dict[str, Any]:
        return self.t.request(self._session_url(suffix), method, body, timeout)

    def value(self, response: Dict[str, Any]) -> Any:
        body = response.get("body") or {}
        return body.get("value") if isinstance(body, dict) else None

    def is_error(self, response: Dict[str, Any]) -> Optional[str]:
        if response.get("error"):
            return response["error"]
        if response.get("status") != 200:
            value = self.value(response)
            code = value.get("error") if isinstance(value, dict) else None
            return code or f"http_{response.get('status')}"
        return None

    def record(self, name: str, started: float, outcome: str, **detail: Any) -> Step:
        step = Step(name, outcome, _ms(started), detail)
        self.steps.append(step)
        print(f"  {outcome:<11} {name} ({step.elapsed_ms} ms)", flush=True)
        return step

    def script(self, expression: str) -> Dict[str, Any]:
        return self.cmd("POST", "execute/sync", {"script": f"return ({expression});", "args": []})

    # --- steps --------------------------------------------------------------
    def new_session(self) -> bool:
        target = TARGETS[self.target]
        caps = {
            "capabilities": {
                "alwaysMatch": {
                    "browserName": target["browserName"],
                    "bstack:options": {
                        "deviceName": target["deviceName"],
                        "osVersion": target["osVersion"],
                        "realMobile": "true",
                        "projectName": "horizon-628",
                        "buildName": self.build,
                        "sessionName": f"spike-{self.target}",
                        "idleTimeout": 60,
                        "local": "false",
                        "debug": "false",
                        "networkLogs": "false",
                        "consoleLogs": "disable",
                    },
                }
            }
        }
        started = time.monotonic()
        response = self.t.request(f"{self.t.hub}/session", "POST", caps, ALLOCATION_TIMEOUT_SECONDS)
        outcome, error = classify_new_session(response)
        if outcome == "unknown":
            self.record("new_session", started, "unknown", error=error, note="allocation-unknown; not retried")
            return False
        if outcome == "failed":
            self.record("new_session", started, "failed", error=error, message=_message(self.value(response)))
            return False
        value = self.value(response) or {}
        self.session_id = value["sessionId"]
        caps_out = value.get("capabilities") or {}
        self.record("new_session", started, "passed", session_digest=_digest(self.session_id),
                    negotiated={k: caps_out.get(k) for k in ("browserName", "browserVersion", "platformName")},
                    extension_keys=sorted(k for k in caps_out if ":" in k))
        return True

    def navigate(self) -> None:
        started = time.monotonic()
        response = self.cmd("POST", "url", {"url": self.url})
        error = self.is_error(response)
        if error:
            self.record("navigate", started, "failed", error=error)
            return
        current = self.value(self.cmd("GET", "url"))
        title = self.value(self.cmd("GET", "title"))
        ok = isinstance(current, str) and current.startswith(self.url) and title == "Horizon mobile fixture"
        self.record("navigate", started, "passed" if ok else "failed", committed_url=current, title=title)

    def page_metrics(self) -> Optional[dict]:
        started = time.monotonic()
        response = self.script(
            "{ua: navigator.userAgent, dpr: window.devicePixelRatio, inner: [innerWidth, innerHeight],"
            " screen: [screen.width, screen.height], touchPoints: navigator.maxTouchPoints,"
            " platform: navigator.platform, visual: window.visualViewport ? [visualViewport.width, visualViewport.height] : null,"
            " generation: document.getElementById('generation').textContent}")
        error = self.is_error(response)
        value = self.value(response) if not error else None
        self.record("execute_script", started, "failed" if error else "passed",
                    error=error, metrics=value)
        return value if isinstance(value, dict) else None

    def screenshot(self, label: str, metrics: Optional[dict]) -> None:
        started = time.monotonic()
        response = self.cmd("GET", "screenshot")
        error = self.is_error(response)
        if error:
            self.record(f"screenshot_{label}", started, "failed", error=error)
            return
        try:
            png = base64.b64decode(self.value(response), validate=True)
            width, height = struct.unpack(">II", png[16:24])
        except (ValueError, struct.error, TypeError):
            self.record(f"screenshot_{label}", started, "failed", error="malformed_png")
            return
        path = os.path.join(self.out_dir, f"{self.target}-{label}.png")
        with open(path, "wb") as handle:
            handle.write(png)
        ratio = None
        if metrics and metrics.get("inner"):
            ratio = round(width / metrics["inner"][0], 3)
        self.record(f"screenshot_{label}", started, "passed", pixels=[width, height], bytes=len(png),
                    screenshot_to_css_ratio=ratio, reported_dpr=metrics.get("dpr") if metrics else None)

    def scroll_variants(self) -> None:
        """Try each portable scroll mechanism and record which ones move a real page."""
        touch = {"actions": [{
            "type": "pointer", "id": "finger", "parameters": {"pointerType": "touch"},
            "actions": [
                {"type": "pointerMove", "duration": 0, "x": 150, "y": 550},
                {"type": "pointerDown", "button": 0},
                {"type": "pause", "duration": 150},
                {"type": "pointerMove", "duration": 250, "x": 150, "y": 450},
                {"type": "pointerMove", "duration": 250, "x": 150, "y": 300},
                {"type": "pointerMove", "duration": 250, "x": 150, "y": 150},
                {"type": "pause", "duration": 100},
                {"type": "pointerUp", "button": 0},
            ]}]}
        wheel = {"actions": [{"type": "wheel", "id": "wheel", "actions": [
            {"type": "scroll", "x": 150, "y": 300, "deltaX": 0, "deltaY": 600, "duration": 200}]}]}
        variants = [
            ("scroll_touch_swipe", lambda: self.cmd("POST", "actions", touch)),
            ("scroll_wheel_action", lambda: self.cmd("POST", "actions", wheel)),
            ("scroll_mobile_swipe", lambda: self.cmd("POST", "execute/sync", {
                "script": "mobile: swipe", "args": [{"direction": "up"}]})),
            ("scroll_mobile_scroll", lambda: self.cmd("POST", "execute/sync", {
                "script": "mobile: scroll", "args": [{"direction": "down"}]})),
            ("scroll_script", lambda: self.cmd("POST", "execute/sync", {
                "script": "window.scrollBy(0, 600); return true;", "args": []})),
        ]
        for name, send in variants:
            started = time.monotonic()
            self.cmd("POST", "execute/sync", {"script": "window.scrollTo(0, 0);", "args": []})
            time.sleep(0.4)
            before = self.value(self.script("Math.round(window.scrollY)"))
            response = send()
            error = self.is_error(response)
            self.cmd("DELETE", "actions")
            if error:
                unsupported = error in ("unsupported operation", "unknown command", "unknown method", "invalid argument")
                self.record(name, started, "unsupported" if unsupported else "failed", error=error,
                            message=_message(self.value(response)))
                continue
            time.sleep(1.0)
            after = self.value(self.script("Math.round(window.scrollY)"))
            moved = isinstance(before, (int, float)) and isinstance(after, (int, float)) and after > before
            self.record(name, started, "passed" if moved else "failed", scroll_before=before, scroll_after=after)
        self.cmd("POST", "execute/sync", {"script": "window.scrollTo(0, 0);", "args": []})

    def element_form(self) -> None:
        started = time.monotonic()
        found = self.cmd("POST", "element", {"using": "css selector", "value": "#name"})
        error = self.is_error(found)
        if error:
            self.record("element_fill_submit", started, "failed", error=error, stage="find")
            return
        element = self.value(found) or {}
        element_id = next(iter(element.values()), None)
        if not element_id:
            self.record("element_fill_submit", started, "failed", error="missing_element_id")
            return
        typed = self.cmd("POST", f"element/{element_id}/value", {"text": "horizon"})
        error = self.is_error(typed)
        if error:
            self.record("element_fill_submit", started, "failed", error=error, stage="value")
            return
        time.sleep(1.0)
        focus_probe = self.value(self.script(
            "{visual: window.visualViewport ? [Math.round(visualViewport.width), Math.round(visualViewport.height)] : null,"
            " inner: [innerWidth, innerHeight], focused: document.activeElement && document.activeElement.id,"
            " viewportHistory: window.fixture.state.viewport}"))
        submit = self.cmd("POST", "element", {"using": "css selector", "value": "#submit"})
        submit_id = next(iter((self.value(submit) or {}).values()), None)
        clicked = self.cmd("POST", f"element/{submit_id}/click", {}) if submit_id else {"error": "no_submit"}
        error = self.is_error(clicked)
        result = self.value(self.script("document.getElementById('result').textContent"))
        ok = not error and result == "result:horizon"
        self.record("element_fill_submit", started, "passed" if ok else "failed", error=error, result=result,
                    keyboard_probe=focus_probe)

    def drawer(self) -> None:
        started = time.monotonic()
        button = self.value(self.cmd("POST", "element", {"using": "css selector", "value": "#open-drawer"})) or {}
        button_id = next(iter(button.values()), None)
        if not button_id:
            self.record("modal_drawer", started, "failed", error="missing_button")
            return
        self.cmd("POST", f"element/{button_id}/click", {})
        time.sleep(0.5)
        hidden = self.value(self.script("document.getElementById('drawer').getAttribute('aria-hidden')"))
        displayed = self.value(self.cmd("GET", "element/" + _drawer_id(self) + "/displayed")) if _drawer_id(self) else None
        self.record("modal_drawer", started, "passed" if hidden == "false" else "failed",
                    aria_hidden=hidden, displayed=displayed)

    def iframe(self) -> None:
        started = time.monotonic()
        frame = self.value(self.cmd("POST", "element", {"using": "css selector", "value": "#frame"})) or {}
        frame_ref = dict(frame) if frame else None
        if not frame_ref:
            self.record("iframe_context", started, "failed", error="missing_frame")
            return
        switched = self.cmd("POST", "frame", {"id": frame_ref})
        error = self.is_error(switched)
        if error:
            self.record("iframe_context", started, "unsupported" if error == "unknown command" else "failed", error=error)
            return
        button = self.value(self.cmd("POST", "element", {"using": "css selector", "value": "#frame-button"})) or {}
        button_id = next(iter(button.values()), None)
        if button_id:
            self.cmd("POST", f"element/{button_id}/click", {})
        inner = self.value(self.script("document.getElementById('frame-state').textContent"))
        returned_by = None
        for label, body in (("frame/parent", None), ("frame:null", {"id": None})):
            if body is None:
                self.cmd("POST", "frame/parent", {})
            else:
                self.cmd("POST", "frame", body)
            probe = self.value(self.script("document.getElementById('frame-result') !== null"))
            if probe is True:
                returned_by = label
                break
        outer = self.value(self.script("document.getElementById('frame-result').textContent")) if returned_by else None
        ok = inner == "frame:clicked" and outer == "frame:clicked"
        self.record("iframe_context", started, "passed" if ok else "failed", inner=inner, outer=outer,
                    returned_to_top_by=returned_by)

    def orientation(self) -> None:
        started = time.monotonic()
        current = self.cmd("GET", "orientation")
        error = self.is_error(current)
        if error:
            self.record("orientation", started, "unsupported", error=error)
            return
        landscape = self.cmd("POST", "orientation", {"orientation": "LANDSCAPE"})
        error = self.is_error(landscape)
        time.sleep(1.0)
        metrics = self.value(self.script("[innerWidth, innerHeight]"))
        restore = self.cmd("POST", "orientation", {"orientation": "PORTRAIT"})
        self.record("orientation", started, "failed" if error or self.is_error(restore) else "passed",
                    initial=self.value(current), landscape_inner=metrics, error=error)

    def window_rect(self) -> None:
        started = time.monotonic()
        response = self.cmd("GET", "window/rect")
        error = self.is_error(response)
        self.record("window_rect", started, "unsupported" if error else "passed", error=error, rect=self.value(response))

    def provider_metadata(self, phase: str) -> Optional[dict]:
        started = time.monotonic()
        response = self.t.request(f"{self.t.api}/automate/sessions/{self.session_id}.json", "GET", None,
                                  COMMAND_TIMEOUT_SECONDS)
        error = self.is_error(response)
        body = response.get("body") or {}
        session = body.get("automation_session") if isinstance(body, dict) else None
        if error or not isinstance(session, dict):
            self.record(f"provider_metadata_{phase}", started, "failed", error=error or "missing_session")
            return None
        keep = {k: session.get(k) for k in ("device", "os", "os_version", "browser", "browser_version", "status",
                                            "duration", "build_name", "project_name")}
        self.record(f"provider_metadata_{phase}", started, "passed", **keep)
        return keep

    def verify_device(self, actual: Optional[dict]) -> None:
        started = time.monotonic()
        target = TARGETS[self.target]
        if not actual:
            self.record("physical_device_evidence", started, "unknown", note="no provider metadata")
            return
        actual_version = str(actual.get("os_version"))
        requested_version = target["osVersion"]
        matches = actual.get("device") == target["deviceName"] and version_matches(requested_version, actual_version)
        self.record("physical_device_evidence", started, "passed" if matches else "failed",
                    requested={"device": target["deviceName"], "os_version": target["osVersion"]},
                    actual={"device": actual.get("device"), "os_version": actual.get("os_version")},
                    version_resolved=actual_version != requested_version,
                    basis="provider session metadata (realMobile capability + automate session record)")

    def release(self) -> None:
        started = time.monotonic()
        response = self.t.request(self._session_url("").rstrip("/"), "DELETE", None, RELEASE_TIMEOUT_SECONDS)
        error = self.is_error(response)
        if response.get("status") is None:
            self.record("release", started, "unknown", error=error, note="release-unknown; provider cleanup unverified")
            return
        if error:
            self.record("release", started, "failed", error=error)
            return
        deadline = time.monotonic() + RELEASE_TIMEOUT_SECONDS
        status = None
        while time.monotonic() < deadline:
            meta = self.provider_metadata("release_poll")
            status = meta.get("status") if meta else None
            if status in TERMINAL_SESSION_STATUSES:
                break
            time.sleep(RELEASE_POLL_SECONDS)
        self.record("release", started, "passed" if status in TERMINAL_SESSION_STATUSES else "unknown",
                    provider_status=status)

    # --- driver -------------------------------------------------------------
    def run(self) -> Dict[str, Any]:
        print(f"target={self.target} build={self.build}", flush=True)
        if self.new_session():
            try:
                actual = self.provider_metadata("after_allocation")
                self.verify_device(actual)
                self.navigate()
                metrics = self.page_metrics()
                self.screenshot("initial", metrics)
                self.window_rect()
                self.scroll_variants()
                self.element_form()
                self.screenshot("after_form", metrics)
                self.drawer()
                self.iframe()
                self.orientation()
            finally:
                self.release()
        report = {
            "schema": REPORT_SCHEMA, "schema_version": REPORT_SCHEMA_VERSION,
            "generated_at": _dt.datetime.now(_dt.timezone.utc).isoformat(timespec="seconds"),
            "target": self.target, "requested": TARGETS[self.target], "url": self.url, "build": self.build,
            "hub_origin": self.t.hub_origin, "session_id": self.session_id,
            "steps": [dataclasses.asdict(step) for step in self.steps],
        }
        path = os.path.join(self.out_dir, f"{self.target}-report.json")
        with open(path, "w", encoding="utf-8") as handle:
            json.dump(report, handle, indent=1)
        print(f"report: {path}", flush=True)
        return report


def classify_new_session(response: Dict[str, Any]) -> "tuple[str, Optional[str]]":
    """Allocation is ambiguous whenever there is no trustworthy result.

    Only a complete HTTP response that carries a WebDriver error is a failure. A
    transport failure (any exception, including resets and truncated bodies) or an
    HTTP 200 without a usable session id may still have allocated a billable device,
    so both are `unknown` and never retried.
    """
    if response.get("status") is None:
        return "unknown", response.get("error") or "no_response"
    body = response.get("body") or {}
    value = body.get("value") if isinstance(body, dict) else None
    if response.get("status") != 200:
        code = value.get("error") if isinstance(value, dict) else None
        return "failed", code or f"http_{response.get('status')}"
    if not isinstance(value, dict) or not isinstance(value.get("sessionId"), str) or not value["sessionId"]:
        return "unknown", "unusable_success_response"
    return "passed", None


def version_matches(requested: str, actual: str) -> bool:
    """A provider may resolve a requested major version to a patch release (18 -> 18.6, 16.0 -> 16.0)."""
    if actual == requested:
        return True
    requested_parts = [part for part in requested.split(".") if part]
    return actual.split(".")[:len(requested_parts)] == requested_parts


def _message(value: Any) -> Optional[str]:
    return value.get("message", "")[:160] if isinstance(value, dict) else None


def _drawer_id(spike: Spike) -> Optional[str]:
    found = spike.value(spike.cmd("POST", "element", {"using": "css selector", "value": "#drawer"})) or {}
    return next(iter(found.values()), None)


def _digest(session_id: str) -> str:
    return hashlib.sha256(session_id.encode()).hexdigest()[:12]


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--target", choices=sorted(TARGETS), required=True)
    parser.add_argument("--out", required=True, help="private directory for the report and screenshots")
    parser.add_argument("--netrc", default=DEFAULT_NETRC)
    parser.add_argument("--hub", default=DEFAULT_HUB)
    parser.add_argument("--api", default=DEFAULT_API)
    parser.add_argument("--url", default=DEFAULT_URL)
    parser.add_argument("--build", default=_dt.datetime.now(_dt.timezone.utc).strftime("spike-%Y%m%dT%H%M%SZ"))
    args = parser.parse_args(argv)
    ensure_private_directory(args.out)
    hub_host = urllib.parse.urlsplit(args.hub).hostname or ""
    transport = Transport(args.hub, args.api, load_auth(args.netrc, hub_host))
    report = Spike(transport, args.out, args.target, args.url, args.build).run()
    return exit_code(report["steps"])


def exit_code(steps: List[Dict[str, Any]]) -> int:
    """Non-zero if any recorded step failed or ended unknown, even when a later step with the same name passed."""
    return 1 if any(step["outcome"] in ("failed", "unknown") for step in steps) else 0


def ensure_private_directory(path: str) -> None:
    """Create the report directory or accept an existing one only when it is private to this user."""
    os.makedirs(path, mode=0o700, exist_ok=True)
    mode = os.stat(path).st_mode
    if mode & 0o077:
        raise SystemExit("output directory must not be accessible by group or others (chmod 700); it receives session ids")


if __name__ == "__main__":
    sys.exit(main())
