"""Smoke-test a configured stdio MCP server using only Python's standard library.

Usage: python scripts/check-mcp.py examples/mcp-windows.json
From WSL, add --from-wsl to exercise the Windows wsl.exe bridge.
"""

import argparse
import json
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("config")
parser.add_argument("--from-wsl", action="store_true")
args = parser.parse_args()
with open(args.config, encoding="utf-8") as config_file:
    server = json.load(config_file)["mcpServers"]["agent-memory"]
command = server["command"]
if args.from_wsl:
    command = "/mnt/c/Windows/System32/wsl.exe"
messages = [
    {"id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-03-26", "capabilities": {},
        "clientInfo": {"name": "agent-memory-smoke", "version": "1"}}},
    {"method": "notifications/initialized"},
    {"id": 2, "method": "tools/list", "params": {}},
    {"id": 3, "method": "tools/call", "params": {
        "name": "memory_search", "arguments": {
            "project": "agent-memory", "query": "Markdown"}}},
    {"id": 4, "method": "tools/call", "params": {
        "name": "memory_changes", "arguments": {"project": "agent-memory"}}},
    {"id": 5, "method": "tools/call", "params": {
        "name": "memory_search", "arguments": {
            "project": "agent-memory", "query": "проверки",
            "kind": "implementation", "status": "implemented"}}},
    {"id": 7, "method": "tools/call", "params": {
        "name": "memory_runtime_info", "arguments": {"project": "agent-memory"}}},
    {"id": 8, "method": "tools/call", "params": {
        "name": "memory_health", "arguments": {"project": "agent-memory"}}},
    {"id": 6, "method": "tools/call", "params": {
        "name": "memory_link_diagnostics", "arguments": {"project": "agent-memory"}}},
]
process = subprocess.Popen(
    [command, *server["args"]], stdin=subprocess.PIPE,
    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8",
)
try:
    output, errors = process.communicate(
        "".join(json.dumps({"jsonrpc": "2.0", **m}) + "\n" for m in messages),
        timeout=30,
    )
except subprocess.TimeoutExpired:
    process.kill()
    process.communicate()
    raise SystemExit("MCP check timed out")
if process.returncode:
    raise SystemExit(errors or f"Server exited with {process.returncode}")
responses = {m["id"]: m for line in output.splitlines()
             if "id" in (m := json.loads(line))}
for request_id in (1, 2, 3, 4, 5, 6, 7, 8):
    if request_id not in responses or "error" in responses[request_id]:
        raise SystemExit(f"Request {request_id} failed: {responses.get(request_id)}")
tools = {tool["name"] for tool in responses[2]["result"]["tools"]}
required = {"memory_health", "memory_runtime_info", "memory_batch_write", "memory_batch_write_operations", "memory_batch_write_resume", "memory_context", "memory_contradictions", "memory_search", "memory_get", "memory_sources", "memory_list", "memory_links",
            "memory_get_record", "memory_history", "memory_revision_get",
            "memory_write", "memory_restore", "memory_sync_status",
            "memory_changes", "memory_freshness", "memory_pull",
            "memory_link_diagnostics", "memory_rename", "memory_verify_evidence",
            "memory_review_queue", "memory_rename_operations", "memory_rename_resume"}
if not required <= tools:
    raise SystemExit(f"Missing tools: {required - tools}")
search = responses[3]["result"]
if search.get("isError") or not search.get("structuredContent", {}).get("hits"):
    raise SystemExit(f"Search failed or returned no knowledge: {search}")
for request_id in (4, 5, 6, 7, 8):
    if responses[request_id]["result"].get("isError"):
        raise SystemExit(f"Tool smoke failed: {responses[request_id]}")
print(f"OK: initialize, {len(tools)} tools, search, changes, Russian filters, link diagnostics, runtime info, health")
