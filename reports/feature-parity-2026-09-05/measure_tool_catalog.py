"""Measure Atlas's local MCP catalog, not a client's prompt or billed usage."""
import argparse
import asyncio
import hashlib
import json
from pathlib import Path

import tiktoken
from atlas.mcp_server import mcp


async def measure(repo, output):
    entries = [tool.model_dump(mode="json", by_alias=True, exclude_none=True)
               for tool in await mcp.list_tools()]
    payload = {"tools": entries, "instructions": mcp.instructions}
    serialize = lambda value: json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    raw = serialize(payload)
    encoder = tiktoken.get_encoding("cl100k_base")
    evidence = {
        "scope": "Local FastMCP list_tools descriptors plus server instructions, compact JSON; not client prompt format or billed usage.",
        "model_or_repository_started": False,
        "tool_count": len(entries), "tokenizer": "cl100k_base",
        "characters": len(raw), "serialized_tokens": len(encoder.encode(raw)),
        "source_sha256": hashlib.sha256((repo / "python_shell/atlas/mcp_server.py").read_bytes()).hexdigest(),
        "payload_sha256": hashlib.sha256(raw.encode()).hexdigest(), "payload": payload,
        "per_tool_serialized_tokens": {tool["name"]: len(encoder.encode(serialize(tool))) for tool in entries},
    }
    output.write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps({key: value for key, value in evidence.items()
                      if key not in ("payload", "per_tool_serialized_tokens")}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--atlas-repo", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    asyncio.run(measure(args.atlas_repo, args.output))
