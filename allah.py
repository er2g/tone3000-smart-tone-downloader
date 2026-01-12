#!/usr/bin/env python3
"""
TONE3000 AI-Powered Smart Tone Downloader
Gemini 2.5 Flash ile akıllı ton arama ve indirme
"""

import requests
import json
import os
import re
from pathlib import Path
from urllib.parse import urlencode
from typing import List, Dict, Optional
import google.generativeai as genai
from google.generativeai.types import GenerationConfig

class TONE3000:
    BASE_URL = "https://www.tone3000.com/api/v1"
    
    def __init__(self, api_key: Optional[str] = None):
        self.access_token = None
        self.refresh_token = None
        
        if api_key:
            self.authenticate(api_key)
    
    def authenticate(self, api_key: str):
        """API key'i session token'a çevir"""
        response = requests.post(
            f"{self.BASE_URL}/auth/session",
            json={"api_key": api_key}
        )
        response.raise_for_status()
        data = response.json()
        
        self.access_token = data["access_token"]
        self.refresh_token = data["refresh_token"]
        print(f"✓ TONE3000 authenticated")
    
    def _headers(self):
        if not self.access_token:
            raise Exception("Not authenticated!")
        return {
            "Authorization": f"Bearer {self.access_token}",
            "Content-Type": "application/json"
        }
    
    def search_tones(
        self, 
        query: str, 
        gear: Optional[str] = None,
        page_size: int = 25
    ) -> Dict:
        """Ton ara"""
        params = {
            "query": query,
            "page_size": min(page_size, 25),
            "sort": "downloads-all-time"
        }
        
        if gear:
            params["gear"] = gear
        
        url = f"{self.BASE_URL}/tones/search?{urlencode(params)}"
        response = requests.get(url, headers=self._headers())
        response.raise_for_status()
        
        return response.json()
    
    def get_models(self, tone_id: int) -> List[Dict]:
        """Ton için tüm modelleri al"""
        url = f"{self.BASE_URL}/models?tone_id={tone_id}&page_size=100"
        response = requests.get(url, headers=self._headers())
        response.raise_for_status()
        
        return response.json()["data"]
    
    def download_model(self, model_url: str, output_path: str):
        """Model dosyasını indir"""
        response = requests.get(
            model_url,
            headers={"Authorization": f"Bearer {self.access_token}"},
            stream=True
        )
        response.raise_for_status()
        
        with open(output_path, "wb") as f:
            for chunk in response.iter_content(chunk_size=8192):
                f.write(chunk)


class SmartToneDownloader:
    _ANALYZE_SCHEMA = {
        "type": "object",
        "properties": {
            "search_queries": {"type": "array", "items": {"type": "string"}, "max_items": 3},
            "gear_type": {"type": "string", "enum": ["amp", "ir", "pedal"], "nullable": True},
            "description": {"type": "string"},
            "fallback_queries": {"type": "array", "items": {"type": "string"}, "max_items": 3},
        },
        "required": ["search_queries", "gear_type", "description", "fallback_queries"],
    }

    _SELECT_SCHEMA = {
        "type": "object",
        "properties": {
            "selected_indices": {"type": "array", "items": {"type": "integer"}, "min_items": 1},
        },
        "required": ["selected_indices"],
    }

    _FILTER_SCHEMA = {
        "type": "object",
        "properties": {
            "selected_indices": {"type": "array", "items": {"type": "integer"}, "min_items": 1},
        },
        "required": ["selected_indices"],
    }

    def __init__(self, tone3000_api_key: str, gemini_api_key: str):
        self.tone_client = TONE3000(api_key=tone3000_api_key)

        # Gemini yapılandır
        genai.configure(api_key=gemini_api_key)
        self.model = genai.GenerativeModel("gemini-2.5-flash")
        self._json_generation_config_base = {
            "response_mime_type": "application/json",
            "temperature": 0,
            "max_output_tokens": 1024,
        }
        print("✓ Gemini 2.5 Flash initialized")
    
    def _safe_filename(self, name: str) -> str:
        safe = re.sub(r'[<>:"/\\\\|?*\\x00-\\x1F]', "_", name).strip(" .")
        return safe or "model"

    def _normalize_model_filename(self, name: str, platform: Optional[str]) -> str:
        basename = self._safe_filename(Path(name).name)
        if Path(basename).suffix:
            return basename

        if (platform or "").lower() == "nam":
            return f"{basename}.nam"

        return basename

    def _text_contains_boost(self, text: str) -> bool:
        t = (text or "").lower()
        if not t:
            return False

        keywords = (
            "boost",
            "boosted",
            "overdrive",
            "od ",
            " od",
            "tubescreamer",
            "tube screamer",
            "ts808",
            "ts-808",
            "ts9",
            "ts-9",
            "sd1",
            "sd-1",
            "klon",
            "treble booster",
            "rangemaster",
        )
        return any(k in t for k in keywords)

    def _tone_contains_boost(self, tone: Dict) -> bool:
        if (tone.get("gear") or "").lower() != "amp":
            return False
        text = f"{tone.get('title','')}\n{tone.get('description','')}"
        return self._text_contains_boost(text)

    def _tone_is_preamp_or_boost_pedal(self, tone: Dict) -> bool:
        if (tone.get("gear") or "").lower() != "pedal":
            return False
        text = f"{tone.get('title','')}\n{tone.get('description','')}"
        t = text.lower()
        keywords = (
            "preamp",
            "boost",
            "overdrive",
            "tubescreamer",
            "tube screamer",
            "ts808",
            "ts-808",
            "ts9",
            "ts-9",
            "sd-1",
            "sd1",
            "klon",
        )
        return any(k in t for k in keywords)

    def _postprocess_selected_indices(
        self,
        tones: List[Dict],
        selected_indices: List[int],
        max_selections: int,
    ) -> List[int]:
        indices = [i for i in selected_indices if 0 <= i < len(tones)]
        seen = set()
        indices = [i for i in indices if not (i in seen or seen.add(i))]

        amp_has_boost = any(self._tone_contains_boost(tones[i]) for i in indices)
        if amp_has_boost:
            indices = [i for i in indices if not self._tone_is_preamp_or_boost_pedal(tones[i])]

        if len(indices) >= max_selections:
            return indices[:max_selections]

        def is_allowed(i: int) -> bool:
            if amp_has_boost and self._tone_is_preamp_or_boost_pedal(tones[i]):
                return False
            return True

        remaining = [
            i
            for i in sorted(range(len(tones)), key=lambda j: tones[j].get("downloads_count", 0), reverse=True)
            if i not in set(indices) and is_allowed(i)
        ]

        for i in remaining:
            indices.append(i)
            if len(indices) >= max_selections:
                break

        return indices

    def _parse_json_response(self, text: str) -> Dict:
        text = (text or "").strip()
        if not text:
            raise ValueError("Empty Gemini response")

        def loads_obj(raw: str) -> Dict:
            value = json.loads(raw)
            if not isinstance(value, dict):
                raise ValueError("Gemini response JSON is not an object")
            return value

        try:
            return loads_obj(text)
        except json.JSONDecodeError:
            pass

        if text.startswith("```json"):
            text = text.split("```json", 1)[1].split("```", 1)[0].strip()
        elif text.startswith("```"):
            text = text.split("```", 1)[1].split("```", 1)[0].strip()

        text_single_line = text.replace("\r", "").replace("\n", " ").strip()
        if text_single_line and text_single_line != text:
            try:
                return loads_obj(text_single_line)
            except json.JSONDecodeError:
                pass

        decoder = json.JSONDecoder()
        starts = [i for i in (text.find("{"), text.find("[")) if i != -1]
        if starts:
            start = min(starts)
            try:
                value, _ = decoder.raw_decode(text[start:])
                if isinstance(value, dict):
                    return value
            except json.JSONDecodeError:
                pass

        raise ValueError(f"Invalid JSON from Gemini: {text[:200]}")

    def _response_text(self, response) -> str:
        text = getattr(response, "text", None)
        if isinstance(text, str) and text.strip():
            return text

        try:
            candidates = getattr(response, "candidates", None) or []
            if not candidates:
                return ""
            parts = getattr(candidates[0].content, "parts", None) or []
            chunks = []
            for part in parts:
                part_text = getattr(part, "text", None)
                if isinstance(part_text, str):
                    chunks.append(part_text)
            return "".join(chunks).strip()
        except Exception:
            return (getattr(response, "text", "") or "").strip()

    def _generate_json(self, prompt: str, schema: Optional[Dict] = None) -> Dict:
        last_error: Optional[Exception] = None
        for attempt in range(2):
            attempt_prompt = prompt
            if attempt == 1:
                attempt_prompt = (
                    prompt
                    + "\n\nIMPORTANT: Your previous response was invalid JSON. "
                    + "Return ONLY valid JSON that matches the required schema. "
                    + "Do not include newlines inside string values."
                )

            response = self.model.generate_content(
                attempt_prompt,
                generation_config=GenerationConfig(
                    **self._json_generation_config_base,
                    response_schema=schema,
                ),
            )

            text = self._response_text(response)
            try:
                return self._parse_json_response(text)
            except Exception as e:
                last_error = e

        raise last_error if last_error else ValueError("Failed to get valid JSON from Gemini")

    def analyze_tone_request(self, user_request: str) -> Dict:
        """
        Kullanıcının ton talebini analiz et, hangi ekipman/arama yapılacağını belirle
        """
        prompt = f"""
Kullanıcı şu tonu arıyor: "{user_request}"

Bu isteğe uygun ekipman/arama terimlerini çıkar.
Kurallar:
- Sadece popüler ve bulunması muhtemel ekipmanları seç.
- `search_queries` en fazla 3 kısa arama terimi olsun.
- `fallback_queries` en fazla 3 alternatif olsun.
- `gear_type` sadece "amp" veya "ir" veya "pedal" veya null olsun.
- Tüm string alanları tek satır olsun (newline yok).

{{
  "search_queries": ["arama1", "arama2", "arama3"],  // En fazla 3 arama terimi (popüler ve bulunabilir olanlar)
  "gear_type": "amp" veya "ir" veya "pedal" veya null,  // Ekipman tipi
  "description": "Kısa açıklama - hangi ton arıyoruz",
  "fallback_queries": ["alternatif1", "alternatif2"]  // Alternatif/benzer tonlar için
}}

Sadece JSON döndür, başka açıklama yapma.
"""

        print(f"\n🤖 Gemini analyzing request...")
        analysis = self._generate_json(prompt, schema=self._ANALYZE_SCHEMA)
        
        print(f"✓ Analysis: {analysis['description']}")
        print(f"  Search queries: {', '.join(analysis['search_queries'])}")
        if "fallback_queries" in analysis and analysis["fallback_queries"]:
            print(f"  Fallback queries: {', '.join(analysis['fallback_queries'])}")
        print(f"  Gear type: {analysis['gear_type'] or 'all'}")
        
        return analysis
    
    def select_best_tones(
        self,
        user_request: str,
        tones: List[Dict],
        max_selections: int = 3
    ) -> List[Dict]:
        """
        Bulunan tonlardan en uygun olanları Gemini ile seç
        """
        if not tones:
            return []

        # Prompt boyutunu sınırlamak için en popüler ilk N tonu değerlendir
        max_candidates = 15
        candidates = sorted(
            tones,
            key=lambda t: t.get("downloads_count", 0) or 0,
            reverse=True,
        )[:max_candidates]

        # Tonları Gemini'ye göstermek için özetle
        tone_summaries = []
        for i, tone in enumerate(candidates):
            desc = (tone.get("description") or "No description").replace("\r", " ").replace("\n", " ")
            desc = desc[:160]
            summary = {
                "index": i,
                "title": tone["title"],
                "description": desc,
                "gear": tone["gear"],
                "platform": tone["platform"],
                "downloads": tone["downloads_count"],
                "contains_boost_in_chain": self._tone_contains_boost(tone),
                "is_preamp_or_boost_pedal": self._tone_is_preamp_or_boost_pedal(tone),
            }
            tone_summaries.append(summary)

        prompt = f"""
Kullanıcı şu tonu arıyor: "{user_request}"

Bulunan tonlar:
{json.dumps(tone_summaries, ensure_ascii=False, separators=(',', ':'))}

Bu tonlardan EN UYGUN {max_selections} tanesini seç.
Seçerken şunlara dikkat et:
- Açıklama kullanıcının isteğine uyuyor mu?
- İndirme sayısı yüksek mi (popüler mi)?
- Ton ismi ve açıklaması ne kadar ilgili?
- Kullanıcı spesifik bir müzisyen/şarkı istediyse, ona en yakın olan hangisi?
- Eğer bir amfi simülasyonunun açıklamasında zaten boost/overdrive (örn. TS/SD-1/Klon) olduğu yazıyorsa, ayrıca preamp/boost pedalı seçme (redundant olmasın).
- Sadece listelenen indeksleri seç.

JSON formatında sadece seçtiğin tonların INDEX numaralarını döndür:
{{
  "selected_indices": [0, 2, 5]
}}

Sadece JSON döndür, başka açıklama yapma.
"""

        print(f"\n🤖 Gemini selecting best tones from {len(tones)} results...")
        selection = self._generate_json(prompt, schema=self._SELECT_SCHEMA)

        selected_indices = selection.get("selected_indices") or []
        print(f"✓ Selected {len(selected_indices)} tones")

        # Seçilen tonları döndür
        raw_indices = selected_indices
        indices = self._postprocess_selected_indices(
            tones=candidates,
            selected_indices=raw_indices,
            max_selections=max_selections,
        )
        selected_tones = [candidates[i] for i in indices]
        return selected_tones
    
    def filter_models(
        self,
        user_request: str,
        tone_title: str,
        tone_description: str,
        models: List[Dict]
    ) -> List[Dict]:
        """
        Bir ton için hangi modellerin indirileceğini Gemini ile belirle
        """
        # Model özetleri
        model_summaries = []
        for i, model in enumerate(models):
            summary = {
                "index": i,
                "name": model["name"],
                "size": model["size"]
            }
            model_summaries.append(summary)
        
        prompt = f"""
Kullanıcı şu tonu arıyor: "{user_request}"
Ton: "{tone_title}"
Açıklama: "{tone_description}"

Bu ton için şu modeller mevcut:
{json.dumps(model_summaries, ensure_ascii=False, separators=(',', ':'))}

Bu tonun SADECE kullanıcının ihtiyacı olan modellerini seç.
Örneğin:
- Eğer "clean" ton isteniyorsa "CRUNCH" veya "HIGH GAIN" kanalları seçme
- Eğer "high gain" isteniyorsa "CLEAN" kanalı seçme  
- Aynı kanalın birden fazla gain seviyesi varsa kullanıcının isteğine en uygununu seç
- "RED" genelde high-gain, "CRUNCH" orta-gain, "CLEAN" clean anlamına gelir
- Size olarak "standard" yeterli, "nano" veya "feather" performans için gerekliyse seç
- Eğer sadece 1-2 model varsa ve ilgili görünüyorlarsa hepsini seç

Maksimum 5 model seç.

JSON formatında sadece seçtiğin modellerin INDEX numaralarını döndür:    
{{"selected_indices": [0, 2]}}

Sadece JSON döndür, başka açıklama yapma.
"""

        selection = self._generate_json(prompt, schema=self._FILTER_SCHEMA)
        
        selected_indices = selection.get("selected_indices") or []
        print(f"    🤖 Selected {len(selected_indices)} models")

        # Seçilen modelleri döndür
        selected_models = [models[i] for i in selected_indices if i < len(models)]
        return selected_models
    
    def smart_download(
        self,
        user_request: str,
        output_dir: str = "./smart_tones",
        max_tones: int = 3,
        max_results_to_analyze: int = 15
    ):
        """
        Akıllı ton indirme - Gemini ile analiz yaparak
        
        Args:
            user_request: Kullanıcının ton talebi (örn: "Van Halen brown sound")
            output_dir: İndirme klasörü
            max_tones: Maksimum kaç ton indirilecek
            max_results_to_analyze: Gemini'ye göndermek için max sonuç sayısı
        """
        
        Path(output_dir).mkdir(parents=True, exist_ok=True)
        
        print(f"\n{'='*70}")
        print(f"🎸 Smart Tone Download: {user_request}")
        print(f"{'='*70}")
        
        # 1. Gemini ile analiz - hangi ekipman aranacak?
        analysis = self.analyze_tone_request(user_request)
        
        # 2. Her arama terimi için tonları bul
        all_tones = []
        seen_ids = set()
        
        # Önce ana aramaları dene
        for query in analysis["search_queries"]:
            print(f"\n🔍 Searching: {query}")
            results = self.tone_client.search_tones(
                query=query,
                gear=analysis["gear_type"],
                page_size=25
            )
            
            # Duplicate'leri filtrele
            added_count = 0
            for tone in results["data"][:max_results_to_analyze]:
                if tone["id"] not in seen_ids:
                    all_tones.append(tone)
                    seen_ids.add(tone["id"])
                    added_count += 1
            
            print(f"  Found {len(results['data'])} tones (added {added_count} new)")
        
        # Eğer yeterli ton bulunamadıysa fallback'leri dene
        if len(all_tones) < 10 and "fallback_queries" in analysis:
            print(f"\n⚠️  Not enough tones found, trying fallback searches...")
            for query in analysis["fallback_queries"]:
                if len(all_tones) >= max_results_to_analyze:
                    break
                    
                print(f"\n🔍 Fallback search: {query}")
                results = self.tone_client.search_tones(
                    query=query,
                    gear=analysis["gear_type"],
                    page_size=25
                )
                
                added_count = 0
                for tone in results["data"][:max_results_to_analyze]:
                    if tone["id"] not in seen_ids:
                        all_tones.append(tone)
                        seen_ids.add(tone["id"])
                        added_count += 1
                
                print(f"  Found {len(results['data'])} tones (added {added_count} new)")
        
        if not all_tones:
            print("❌ No tones found!")
            return
        
        print(f"\n📊 Total unique tones found: {len(all_tones)}")
        
        # 3. Gemini ile en iyi tonları seç
        selected_tones = self.select_best_tones(
            user_request=user_request,
            tones=all_tones,
            max_selections=max_tones
        )
        
        # 4. Seçilen tonları indir
        total_downloaded = 0
        
        for idx, tone in enumerate(selected_tones, 1):
            print(f"\n{'─'*70}")
            print(f"[{idx}/{len(selected_tones)}] {tone['title']}")
            print(f"  User: {tone['user']['username']}")
            print(f"  Downloads: {tone['downloads_count']:,}")
            print(f"  Description: {tone.get('description', 'N/A')[:100]}...")
            
            # Güvenli klasör adı
            safe_title = "".join(
                c if c.isalnum() or c in (' ', '-', '_') else '_' 
                for c in tone['title']
            ).strip()[:50]  # Max 50 karakter
            
            tone_dir = Path(output_dir) / f"{safe_title}_{tone['id']}"
            tone_dir.mkdir(exist_ok=True)
            
            # Ton bilgilerini kaydet
            with open(tone_dir / "info.json", "w", encoding="utf-8") as f:
                json.dump(tone, f, indent=2, ensure_ascii=False)
            
            # Modelleri al
            all_models = self.tone_client.get_models(tone["id"])
            print(f"  Total models available: {len(all_models)}")
            
            # Gemini ile modelleri filtrele
            selected_models = self.filter_models(
                user_request=user_request,
                tone_title=tone["title"],
                tone_description=tone.get("description", ""),
                models=all_models
            )
            
            # Seçilen modelleri indir
            for model in selected_models:
                filename = self._normalize_model_filename(
                    model["name"],
                    tone.get("platform"),
                )
                output_path = tone_dir / filename
                
                if output_path.exists():
                    print(f"    ⊘ Skipped: {filename} (exists)")
                    continue
                
                try:
                    print(f"    ⬇ Downloading: {filename} ({model['size']})...", end=" ")
                    self.tone_client.download_model(model["model_url"], str(output_path))
                    size_mb = output_path.stat().st_size / (1024 * 1024)
                    print(f"✓ ({size_mb:.1f} MB)")
                    total_downloaded += 1
                except Exception as e:
                    print(f"✗ Error: {e}")
        
        print(f"\n{'='*70}")
        print(f"✅ Done! Downloaded {total_downloaded} models to {output_dir}")
        print(f"{'='*70}")


def main():
    """Örnek kullanım"""
    
    # API keys
    TONE3000_KEY = os.getenv("TONE3000_API_KEY") or input("TONE3000 API key: ").strip()
    GEMINI_KEY = os.getenv("GEMINI_API_KEY") or input("Gemini API key: ").strip()
    
    # Smart downloader oluştur
    downloader = SmartToneDownloader(
        tone3000_api_key=TONE3000_KEY,
        gemini_api_key=GEMINI_KEY
    )
    
    # Kullanıcıdan ton talebi al
    print("\n" + "="*70)
    print("🎸 TONE3000 Smart Downloader (powered by Gemini 2.5 Flash)")
    print("="*70)
    print("\nÖrnekler:")
    print("  • Van Halen brown sound")
    print("  • 90'lar death metal tonu")
    print("  • John Mayer clean ton")
    print("  • Metallica Master of Puppets riff tonu")
    print("  • Pink Floyd Comfortably Numb solo tonu")
    print()
    
    user_request = input("Hangi tonu arıyorsun? ").strip()
    
    if not user_request:
        print("❌ Ton talebi boş!")
        return
    
    # Akıllı indirme
    downloader.smart_download(
        user_request=user_request,
        output_dir="./smart_downloaded_tones",
        max_tones=3,  # En fazla 3 ton indir
        max_results_to_analyze=15  # İlk 15 sonucu Gemini'ye gönder
    )


if __name__ == "__main__":
    main()
