"""
Cloto MCP Server: Web Search
Multi-provider web search with page content extraction.
Fallback chain: SearXNG (self-hosted) → Tavily (cloud API) → DuckDuckGo (zero-config).
"""

import asyncio
import json
import os
import sys

# Clear proxy env vars injected by kernel isolation — websearch needs direct
# internet access to reach DuckDuckGo/SearXNG/Tavily endpoints.
for _proxy_key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY"]:
    os.environ.pop(_proxy_key, None)

import httpx
from mcp.server.stdio import stdio_server

sys.path.insert(0, os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")))

from common.mcp_utils import ToolRegistry
from common.search import create_search_provider, PROVIDER, SEARXNG_URL, TAVILY_API_KEY

# ============================================================
# Configuration
# ============================================================

DEFAULT_MAX_RESULTS = int(os.environ.get("CLOTO_SEARCH_MAX_RESULTS", "5"))
FETCH_MAX_LENGTH = int(os.environ.get("CLOTO_FETCH_MAX_LENGTH", "10000"))
REQUEST_TIMEOUT = int(os.environ.get("CLOTO_SEARCH_TIMEOUT", "15"))

provider = create_search_provider()


# ============================================================
# Page Fetcher
# ============================================================

async def fetch_page_content(url: str, max_length: int) -> str:
    """Fetch a URL and extract text content."""
    client = httpx.AsyncClient(timeout=REQUEST_TIMEOUT, follow_redirects=True, proxy=None)
    try:
        resp = await client.get(url, headers={
            "User-Agent": "ClotoCore/0.4 (Web Search MCP Server)",
            "Accept": "text/html,application/xhtml+xml,text/plain",
        })
        resp.raise_for_status()
        content_type = resp.headers.get("content-type", "")

        if "text/html" in content_type:
            return html_to_text(resp.text)[:max_length]
        elif "text/plain" in content_type or "application/json" in content_type:
            return resp.text[:max_length]
        else:
            return f"[Unsupported content type: {content_type}]"
    except Exception as e:
        return f"[Error fetching {url}: {e}]"
    finally:
        await client.aclose()


def html_to_text(html: str) -> str:
    """Simple HTML to text conversion without heavy dependencies."""
    import re
    # Remove script and style blocks
    text = re.sub(r'<script[^>]*>.*?</script>', '', html, flags=re.DOTALL | re.IGNORECASE)
    text = re.sub(r'<style[^>]*>.*?</style>', '', text, flags=re.DOTALL | re.IGNORECASE)
    # Convert common block elements to newlines
    text = re.sub(r'<(?:p|div|h[1-6]|li|br|tr)[^>]*>', '\n', text, flags=re.IGNORECASE)
    # Remove remaining tags
    text = re.sub(r'<[^>]+>', '', text)
    # Decode common entities
    text = text.replace('&amp;', '&').replace('&lt;', '<').replace('&gt;', '>')
    text = text.replace('&quot;', '"').replace('&#39;', "'").replace('&nbsp;', ' ')
    # Collapse whitespace
    text = re.sub(r'\n\s*\n', '\n\n', text)
    text = re.sub(r' +', ' ', text)
    return text.strip()


# ============================================================
# Provider Health Check
# ============================================================

async def check_provider_status(name: str) -> dict:
    """Check if a specific provider is configured and reachable."""
    if name == "searxng":
        configured = bool(SEARXNG_URL)
        if not configured:
            return {"name": name, "configured": False, "reachable": False}
        try:
            async with httpx.AsyncClient(timeout=5) as client:
                resp = await client.get(f"{SEARXNG_URL}/")
                reachable = resp.status_code < 500
        except Exception:
            reachable = False
        return {
            "name": name,
            "configured": True,
            "reachable": reachable,
            "url": SEARXNG_URL,
            "setup_hint": "Run 'docker compose up -d' in the ClotoCore project root." if not reachable else None,
        }
    elif name == "tavily":
        configured = bool(TAVILY_API_KEY)
        return {
            "name": name,
            "configured": configured,
            "reachable": configured,  # If key is set, API is reachable (cloud service)
            "setup_hint": "Register at https://tavily.com (free, no credit card) and add TAVILY_API_KEY to .env." if not configured else None,
        }
    elif name == "duckduckgo":
        # DuckDuckGo HTML scraping — no external deps, always available
        error_detail = None
        try:
            async with httpx.AsyncClient(timeout=10, proxy=None) as client:
                resp = await client.get("https://html.duckduckgo.com/html/", params={"q": "test"},
                                        headers={"User-Agent": "Mozilla/5.0 (compatible; ClotoCore/0.6)"})
                reachable = resp.status_code == 200
                if not reachable:
                    error_detail = f"HTTP {resp.status_code}"
        except Exception as e:
            reachable = False
            error_detail = f"{type(e).__name__}: {e}"
        return {
            "name": name,
            "configured": True,
            "reachable": reachable,
            "note": "Zero-config fallback via HTML scraping. No external deps required."
                if reachable else f"DuckDuckGo HTML endpoint unreachable. Error: {error_detail}",
        }
    return {"name": name, "configured": False, "reachable": False}


# ============================================================
# MCP Server
# ============================================================

registry = ToolRegistry("cloto-mcp-websearch")


@registry.tool(
    "web_search",
    "Search the web and return relevant results with titles, URLs, "
    "and snippets. Use this to find current information, documentation, "
    "news, or any web-based knowledge.",
    {
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "The search query"},
            "max_results": {"type": "integer", "description": "Maximum results to return (default: 5, max: 20)"},
            "language": {"type": "string", "description": "Language code (e.g., 'en', 'ja'). Default: 'en'"},
            "time_range": {"type": "string", "enum": ["day", "week", "month", "year"], "description": "Filter results by recency"},
        },
        "required": ["query"],
    },
)
async def handle_web_search(arguments: dict) -> dict:
    query = arguments.get("query", "")
    max_results = min(arguments.get("max_results", DEFAULT_MAX_RESULTS), 20)
    language = arguments.get("language", "en")
    time_range = arguments.get("time_range")

    if not query.strip():
        return {"error": "Empty query"}

    try:
        results = await provider.search(query, max_results, language, time_range)
        return {
            "provider": PROVIDER,
            "query": query,
            "results": results,
            "total_results": len(results),
        }
    except Exception as e:
        return {
            "error": f"All search providers failed. Last error: {e}",
            "provider": PROVIDER,
            "query": query,
        }


@registry.tool(
    "fetch_page",
    "Fetch a web page and extract its text content. "
    "Use after web_search to read the full content of a result.",
    {
        "type": "object",
        "properties": {
            "url": {"type": "string", "description": "The URL to fetch"},
            "max_length": {"type": "integer", "description": "Maximum characters to return (default: 10000)"},
        },
        "required": ["url"],
    },
)
async def handle_fetch_page(arguments: dict) -> dict:
    url = arguments.get("url", "")
    max_length = arguments.get("max_length", FETCH_MAX_LENGTH)

    if not url.strip():
        return {"error": "Empty URL"}

    content = await fetch_page_content(url, max_length)
    return {
        "url": url,
        "content": content,
        "length": len(content),
        "truncated": len(content) >= max_length,
    }


@registry.tool(
    "search_status",
    "Check which web search providers are configured and reachable. "
    "Returns the status of each provider in the fallback chain "
    "(SearXNG, Tavily, DuckDuckGo) with setup hints for unconfigured providers. "
    "Use this when search fails or when the user asks about search capabilities.",
    {"type": "object", "properties": {}},
)
async def handle_search_status(arguments: dict) -> dict:
    statuses = await asyncio.gather(
        check_provider_status("searxng"),
        check_provider_status("tavily"),
        check_provider_status("duckduckgo"),
    )
    chain = list(statuses)

    active = "none"
    for s in chain:
        if s["reachable"]:
            active = s["name"]
            break

    # Debug: include proxy env vars to diagnose connectivity issues
    proxy_env = {k: os.environ.get(k, "<not set>") for k in
                 ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY",
                  "CLOTO_LLM_PROXY", "CLOTO_LLM_PROXY_PORT", "NO_PROXY"]}
    return {"mode": PROVIDER, "active_provider": active, "chain": chain, "debug_proxy_env": proxy_env}


# ============================================================
# Entry Point
# ============================================================

async def main():
    try:
        async with stdio_server() as (read_stream, write_stream):
            await registry.server.run(read_stream, write_stream, registry.server.create_initialization_options())
    finally:
        if hasattr(provider, "aclose"):
            await provider.aclose()


if __name__ == "__main__":
    asyncio.run(main())
