#!/usr/bin/env python3
"""Check the Hetzner Cloud API surface that `horizon-cloud::hetzner` uses against
Hetzner's current OpenAPI spec, so the adapter never relies on a removed or
deprecated operation or field. The human-readable docs can lag; the spec and the
changelog (https://docs.hetzner.cloud/changelog) are the sources of truth.

Usage: check-hetzner-api.py [SPEC_JSON]   (downloads the current spec when omitted)
Run it before changing the adapter; update USED when the adapter changes.
"""

import json
import sys
import urllib.request

SPEC_URL = "https://docs.hetzner.cloud/cloud.spec.json"

SERVER = ["id", "name", "status", "public_net.ipv4.ip", "server_type.name", "server_type.cores",
          "server_type.memory", "server_type.disk", "location.name", "labels", "volumes"]
VOLUME = ["id", "name", "size", "location.name", "server", "linux_device", "status", "labels"]
ACTION = ["id", "status", "error.message"]
SERVER_TYPE = ["name", "cores", "memory", "disk", "cpu_type", "architecture", "prices.[].location",
               "prices.[].price_hourly.net", "prices.[].price_monthly.net", "locations.[].name",
               "locations.[].deprecation", "locations.[].available", "locations.[].recommended"]
PRICING = ["pricing.currency", "pricing.volume.price_per_gb_month.net", "pricing.primary_ips.[].type",
           "pricing.primary_ips.[].prices.[].location", "pricing.primary_ips.[].prices.[].price_monthly.net"]


def prefixed(prefix, fields):
    return [f"{prefix}.{field}" for field in fields]


# (method, path, query parameters, request fields, response status, response fields)
USED = [
    ("get", "/servers", ["label_selector", "page", "per_page"], [], "200",
     prefixed("servers.[]", SERVER) + ["meta.pagination.next_page"]),
    ("post", "/servers", [], ["name", "server_type", "location", "image", "user_data", "labels",
                              "start_after_create", "public_net.enable_ipv4", "public_net.enable_ipv6",
                              "volumes", "automount"], "201", prefixed("server", SERVER) + prefixed("action", ACTION)),
    ("get", "/servers/{id}", [], [], "200", prefixed("server", SERVER)),
    ("delete", "/servers/{id}", [], [], "200", prefixed("action", ACTION)),
    ("post", "/servers/{id}/actions/poweron", [], [], "201", prefixed("action", ACTION)),
    ("post", "/servers/{id}/actions/shutdown", [], [], "201", prefixed("action", ACTION)),
    ("post", "/servers/{id}/actions/poweroff", [], [], "201", prefixed("action", ACTION)),
    ("get", "/actions/{id}", [], [], "200", prefixed("action", ACTION)),
    ("get", "/volumes", ["label_selector", "page", "per_page"], [], "200",
     prefixed("volumes.[]", VOLUME) + ["meta.pagination.next_page"]),
    ("post", "/volumes", [], ["name", "size", "location", "format", "labels"], "201",
     prefixed("volume", VOLUME) + prefixed("action", ACTION)),
    ("get", "/volumes/{id}", [], [], "200", prefixed("volume", VOLUME)),
    ("delete", "/volumes/{id}", [], [], None, []),
    ("post", "/volumes/{id}/actions/attach", [], ["server", "automount"], "201", prefixed("action", ACTION)),
    ("post", "/volumes/{id}/actions/detach", [], [], "201", prefixed("action", ACTION)),
    ("get", "/server_types", ["page", "per_page"], [], "200",
     prefixed("server_types.[]", SERVER_TYPE) + ["meta.pagination.next_page"]),
    ("get", "/pricing", [], [], "200", PRICING),
]


class Spec:
    def __init__(self, document):
        self.document = document

    def resolve(self, node):
        for _ in range(64):
            if not (isinstance(node, dict) and "$ref" in node):
                return node
            target = self.document
            for part in node["$ref"].lstrip("#/").split("/"):
                target = target[part]
            node = target
        raise ValueError("reference loop")

    @staticmethod
    def deprecated(node):
        note = (node.get("description") or "").lstrip().lower()
        return bool(node.get("deprecated")) or note.startswith(("this field is deprecated", "**deprecated"))

    def properties(self, node):
        node = self.resolve(node)
        merged = dict(node.get("properties", {}))
        for combination in ("allOf", "oneOf", "anyOf"):
            for part in node.get(combination, []):
                merged.update(self.properties(part))
        return merged

    def field(self, schema, dotted):
        """Problems with one dotted field path; `[]` steps into array items."""
        node = self.resolve(schema)
        for part in dotted.split("."):
            if part == "[]":
                node = self.resolve(node.get("items", {}))
                continue
            properties = self.properties(node)
            if part not in properties:
                return [f"missing '{part}'"]
            node = self.resolve(properties[part])
            if self.deprecated(node):
                return [f"deprecated '{part}'"]
        return []

    def check(self, method, path, query, body, status, fields):
        name = f"{method.upper()} {path}"
        operation = self.document.get("paths", {}).get(path, {}).get(method)
        if operation is None:
            return [f"{name}: operation missing"]
        problems = []
        if self.deprecated(operation):
            problems.append(f"{name}: operation deprecated")
        parameters = {self.resolve(p)["name"]: self.resolve(p) for p in operation.get("parameters", [])}
        for parameter in query:
            if parameter not in parameters:
                problems.append(f"{name}: parameter '{parameter}' missing")
            elif self.deprecated(parameters[parameter]):
                problems.append(f"{name}: parameter '{parameter}' deprecated")
        if body:
            schema = operation["requestBody"]["content"]["application/json"]["schema"]
            problems += [f"{name} body {f}: {p}" for f in body for p in self.field(schema, f)]
        if status:
            schema = operation["responses"][status]["content"]["application/json"]["schema"]
            problems += [f"{name} {status} {f}: {p}" for f in fields for p in self.field(schema, f)]
        return problems


def self_test(spec):
    """The checker must catch the changes that already broke a draft of the adapter."""
    removed = spec.check("get", "/servers/{id}", [], [], "200", ["server.datacenter.location.name"])
    retired = spec.check("get", "/datacenters", [], [], None, [])
    return [] if removed and retired else ["self-test: known removed or deprecated API was not detected"]


def main():
    if len(sys.argv) > 1:
        with open(sys.argv[1], encoding="utf-8") as handle:
            document = json.load(handle)
    else:
        with urllib.request.urlopen(SPEC_URL, timeout=60) as response:
            document = json.load(response)
    spec = Spec(document)
    problems = self_test(spec)
    for entry in USED:
        problems += spec.check(*entry)
    for problem in problems:
        print(problem)
    info = document.get("info", {})
    print(f"{len(problems)} problem(s) against {info.get('title')} {info.get('version')}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
