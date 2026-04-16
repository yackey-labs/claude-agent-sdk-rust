#!/usr/bin/env python3
"""Benchmark: Python claude-agent-sdk — one-shot query."""

import asyncio
import os
import resource
import time

from claude_agent_sdk import query, ClaudeAgentOptions


async def main():
    prompt = "What is 2 + 2? Reply with just the number."
    options = ClaudeAgentOptions(max_turns=1)

    start = time.monotonic()
    result_text = ""
    async for msg in query(prompt=prompt, options=options):
        if hasattr(msg, "content"):
            for block in msg.content:
                if hasattr(block, "text"):
                    result_text += block.text

    elapsed = time.monotonic() - start
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    self_usage = resource.getrusage(resource.RUSAGE_SELF)

    print(f"answer: {result_text.strip()}")
    print(f"wall_ms: {elapsed * 1000:.0f}")
    print(f"self_user_ms: {self_usage.ru_utime * 1000:.0f}")
    print(f"self_sys_ms: {self_usage.ru_stime * 1000:.0f}")
    print(f"child_user_ms: {usage.ru_utime * 1000:.0f}")
    print(f"child_sys_ms: {usage.ru_stime * 1000:.0f}")
    print(f"max_rss_kb: {self_usage.ru_maxrss}")


if __name__ == "__main__":
    asyncio.run(main())
