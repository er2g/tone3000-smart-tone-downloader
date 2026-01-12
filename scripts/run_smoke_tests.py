import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional, Tuple


REPO_ROOT = Path(__file__).resolve().parents[1]
KEYS_PATH = REPO_ROOT / "keys.txt"
MODULE_PATH = REPO_ROOT / "allah.py"


@dataclass(frozen=True)
class ApiKeys:
    tone3000: str
    gemini: str


def _strip_quotes(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
        return value[1:-1].strip()
    return value


def load_keys(path: Path) -> ApiKeys:
    if not path.exists():
        raise FileNotFoundError(f"Missing keys file: {path}")

    lines = [ln.strip() for ln in path.read_text(encoding="utf-8", errors="replace").splitlines()]
    lines = [ln for ln in lines if ln and not ln.startswith("#")]

    kv: Dict[str, str] = {}
    for ln in lines:
        if "=" in ln:
            k, v = ln.split("=", 1)
            k = k.strip()
            v = _strip_quotes(v)
            kv[k] = v

    tone = kv.get("TONE3000_API_KEY")
    gemini = kv.get("GEMINI_API_KEY")

    if tone and gemini:
        return ApiKeys(tone3000=tone, gemini=gemini)

    if len(lines) >= 2:
        a = _strip_quotes(lines[0]).lstrip("\ufeff")
        b = _strip_quotes(lines[1]).lstrip("\ufeff")

        uuid_re = r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
        is_uuid_a = __import__("re").match(uuid_re, a) is not None
        is_uuid_b = __import__("re").match(uuid_re, b) is not None
        is_gemini_a = a.startswith("AIza")
        is_gemini_b = b.startswith("AIza")

        if is_uuid_a and is_gemini_b:
            return ApiKeys(tone3000=a, gemini=b)
        if is_uuid_b and is_gemini_a:
            return ApiKeys(tone3000=b, gemini=a)

        if is_gemini_a and not is_gemini_b:
            return ApiKeys(tone3000=b, gemini=a)
        if is_gemini_b and not is_gemini_a:
            return ApiKeys(tone3000=a, gemini=b)

        return ApiKeys(tone3000=a, gemini=b)

    raise ValueError(
        "keys.txt format not recognized; expected TONE3000_API_KEY=... and GEMINI_API_KEY=..., "
        "or first two non-empty lines as keys."
    )


def import_allah(module_path: Path):
    import importlib.util

    spec = importlib.util.spec_from_file_location("tone_downloader", module_path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Failed to import module: {module_path}")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def build_tone_pool(downloader, analysis: Dict, max_results_to_analyze: int) -> List[Dict]:
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


def check_redundant_boost(downloader, selected: List[Dict]) -> bool:
    amp_has_boost = any(downloader._tone_contains_boost(t) for t in selected)
    if not amp_has_boost:
        return False
    return any(downloader._tone_is_preamp_or_boost_pedal(t) for t in selected)


def summarize_selected(downloader, selected: List[Dict]) -> List[Dict]:
    out = []
    for t in selected:
        out.append(
            {
                "id": t.get("id"),
                "title": t.get("title"),
                "gear": t.get("gear"),
                "platform": t.get("platform"),
                "downloads_count": t.get("downloads_count"),
                "contains_boost_in_chain": downloader._tone_contains_boost(t),
                "is_preamp_or_boost_pedal": downloader._tone_is_preamp_or_boost_pedal(t),
                "url": t.get("url"),
            }
        )
    return out


def run_one(
    downloader,
    user_request: str,
    output_dir: Path,
    max_selections: int = 3,
    max_results_to_analyze: int = 12,
) -> Tuple[Dict, List[Dict]]:
    analysis = downloader.analyze_tone_request(user_request)
    pool = build_tone_pool(downloader, analysis, max_results_to_analyze=max_results_to_analyze)
    selected = downloader.select_best_tones(user_request=user_request, tones=pool, max_selections=max_selections)

    output_dir.mkdir(parents=True, exist_ok=True)
    report = {
        "user_request": user_request,
        "analysis": analysis,
        "pool_size": len(pool),
        "selected": summarize_selected(downloader, selected),
        "redundant_boost_violation": check_redundant_boost(downloader, selected),
    }

    (output_dir / "report.json").write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    return report, selected


def main() -> int:
    keys = load_keys(KEYS_PATH)

    mod = import_allah(MODULE_PATH)
    downloader = mod.SmartToneDownloader(tone3000_api_key=keys.tone3000, gemini_api_key=keys.gemini)

    tests = [
        "Metallica Master of Puppets rhythm tone",
        "John Mayer clean tone",
        "Van Halen brown sound",
        "90'lar death metal tonu",
        "modern djent tight high gain tone",
    ]

    base_dir = REPO_ROOT / "smart_downloaded_tones" / "_smoke_tests"
    base_dir.mkdir(parents=True, exist_ok=True)

    failures: List[str] = []
    for i, req in enumerate(tests, 1):
        out_dir = base_dir / f"test_{i}"
        try:
            report, _ = run_one(downloader, req, out_dir)
        except Exception as e:
            failures.append(f"test_{i}: {req} -> {type(e).__name__}: {e}")
            continue

        if report.get("pool_size", 0) == 0:
            failures.append(f"test_{i}: {req} -> no tones found")
        if report.get("redundant_boost_violation"):
            failures.append(f"test_{i}: {req} -> redundant boost/preamp selection detected")

    if failures:
        sys.stderr.write("FAILURES:\n" + "\n".join(f"- {f}" for f in failures) + "\n")
        return 1

    print(f"OK: {len(tests)} tests passed; reports saved under {base_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
