#!/usr/bin/env python3
"""Bridge script for the Tauri desktop UI.

The script wraps ``SmartToneDownloader`` and returns a single JSON payload to stdout.
"""

from __future__ import annotations

import argparse
import contextlib
import importlib.util
import io
import json
import os
import sys
import warnings
from pathlib import Path
from typing import Dict, List, Optional


REPO_ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = REPO_ROOT / "allah.py"

warnings.filterwarnings("ignore", category=FutureWarning)


def _import_allah(module_path: Path):
    spec = importlib.util.spec_from_file_location("tone_downloader", module_path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Failed to import module: {module_path}")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _safe_tone_dir_name(title: str, tone_id: int) -> str:
    safe_title = "".join(c if c.isalnum() or c in (" ", "-", "_") else "_" for c in title).strip()
    safe_title = safe_title[:50] or "tone"
    return f"{safe_title}_{tone_id}"


def _build_tone_pool(downloader, analysis: Dict, max_results_to_analyze: int) -> List[Dict]:
    all_tones: List[Dict] = []
    seen_ids = set()

    def add_results(results: Dict):
        nonlocal all_tones
        data = results.get("data") or []
        for tone in data[:max_results_to_analyze]:
            tone_id = tone.get("id")
            if tone_id is None or tone_id in seen_ids:
                continue
            seen_ids.add(tone_id)
            all_tones.append(tone)

    for query in analysis.get("search_queries") or []:
        results = downloader.tone_client.search_tones(
            query=query,
            gear=analysis.get("gear_type"),
            page_size=25,
        )
        add_results(results)

    if len(all_tones) < 10 and (analysis.get("fallback_queries") or []):
        for query in analysis.get("fallback_queries") or []:
            if len(all_tones) >= max_results_to_analyze:
                break
            results = downloader.tone_client.search_tones(
                query=query,
                gear=analysis.get("gear_type"),
                page_size=25,
            )
            add_results(results)

    return all_tones


def _summarize_tone(tone: Dict) -> Dict:
    user = tone.get("user") or {}
    return {
        "id": tone.get("id"),
        "title": tone.get("title"),
        "description": tone.get("description"),
        "gear": tone.get("gear"),
        "platform": tone.get("platform"),
        "downloads_count": tone.get("downloads_count"),
        "author": user.get("username"),
        "url": tone.get("url"),
    }


def _load_key(explicit_value: Optional[str], env_name: str) -> Optional[str]:
    if explicit_value:
        return explicit_value.strip()
    env_value = os.getenv(env_name)
    if env_value:
        return env_value.strip()
    return None


def _read_keys_file(path: Path) -> Dict[str, str]:
    if not path.exists():
        return {}

    lines = [ln.strip() for ln in path.read_text(encoding="utf-8", errors="replace").splitlines()]
    lines = [ln for ln in lines if ln and not ln.startswith("#")]
    data: Dict[str, str] = {}
    for ln in lines:
        if "=" in ln:
            k, v = ln.split("=", 1)
            data[k.strip()] = v.strip().strip("'\"")
    return data


def _resolve_keys(args: argparse.Namespace) -> Dict[str, str]:
    keys_file = _read_keys_file(REPO_ROOT / "keys.txt")
    tone_key = (
        _load_key(args.tone3000_key, "TONE3000_API_KEY")
        or keys_file.get("TONE3000_API_KEY")
    )
    gemini_key = (
        _load_key(args.gemini_key, "GEMINI_API_KEY")
        or keys_file.get("GEMINI_API_KEY")
    )

    if not tone_key or not gemini_key:
        raise ValueError(
            "Missing API keys. Provide both TONE3000 and Gemini keys from UI, env vars, or keys.txt."
        )
    return {"tone3000": tone_key, "gemini": gemini_key}


def run_download(args: argparse.Namespace) -> Dict:
    output_dir = Path(args.output_dir).resolve()
    output_dir.mkdir(parents=True, exist_ok=True)

    mod = _import_allah(MODULE_PATH)
    keys = _resolve_keys(args)
    logs_io = io.StringIO()
    downloaded_count = 0
    downloaded_models: List[Dict] = []

    with contextlib.redirect_stdout(logs_io):
        downloader = mod.SmartToneDownloader(
            tone3000_api_key=keys["tone3000"],
            gemini_api_key=keys["gemini"],
        )
        analysis = downloader.analyze_tone_request(args.request)
        tone_pool = _build_tone_pool(downloader, analysis, max_results_to_analyze=args.max_results)
        selected_tones = downloader.select_best_tones(
            user_request=args.request,
            tones=tone_pool,
            max_selections=args.max_tones,
        )

        for tone in selected_tones:
            tone_dir = output_dir / _safe_tone_dir_name(tone.get("title", "tone"), tone.get("id", 0))
            tone_dir.mkdir(parents=True, exist_ok=True)
            (tone_dir / "info.json").write_text(
                json.dumps(tone, indent=2, ensure_ascii=False),
                encoding="utf-8",
            )

            all_models = downloader.tone_client.get_models(tone["id"])
            selected_models = downloader.filter_models(
                user_request=args.request,
                tone_title=tone.get("title", ""),
                tone_description=tone.get("description", ""),
                models=all_models,
            )

            for model in selected_models:
                filename = downloader._normalize_model_filename(model["name"], tone.get("platform"))
                target_path = tone_dir / filename
                if target_path.exists():
                    downloaded_models.append(
                        {
                            "tone_id": tone.get("id"),
                            "tone_title": tone.get("title"),
                            "model_name": filename,
                            "status": "skipped_exists",
                            "path": str(target_path),
                            "size_mb": round(target_path.stat().st_size / (1024 * 1024), 2),
                        }
                    )
                    continue

                downloader.tone_client.download_model(model["model_url"], str(target_path))
                size_mb = round(target_path.stat().st_size / (1024 * 1024), 2)
                downloaded_count += 1
                downloaded_models.append(
                    {
                        "tone_id": tone.get("id"),
                        "tone_title": tone.get("title"),
                        "model_name": filename,
                        "status": "downloaded",
                        "path": str(target_path),
                        "size_mb": size_mb,
                    }
                )

    logs = logs_io.getvalue()
    return {
        "ok": True,
        "request": args.request,
        "analysis": analysis,
        "pool_size": len(tone_pool),
        "selected_tones": [_summarize_tone(t) for t in selected_tones],
        "downloaded_count": downloaded_count,
        "model_items": downloaded_models,
        "output_dir": str(output_dir),
        "logs": logs,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Tauri bridge runner for Tone3000 downloader")
    parser.add_argument("--request", required=True, help="User tone request")
    parser.add_argument("--output-dir", default="./smart_downloaded_tones", help="Download directory")
    parser.add_argument("--max-tones", type=int, default=3, help="Maximum selected tones")
    parser.add_argument("--max-results", type=int, default=15, help="Max candidate tones for Gemini analysis")
    parser.add_argument("--tone3000-key", default=None, help="Tone3000 API key")
    parser.add_argument("--gemini-key", default=None, help="Gemini API key")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        payload = run_download(args)
    except Exception as exc:  # pragma: no cover - runtime guard
        payload = {
            "ok": False,
            "error": f"{type(exc).__name__}: {exc}",
        }
    sys.stdout.write(json.dumps(payload, ensure_ascii=False))
    sys.stdout.flush()
    return 0 if payload.get("ok") else 1


if __name__ == "__main__":
    raise SystemExit(main())
