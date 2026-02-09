#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

const TONE3000_BASE_URL: &str = "https://www.tone3000.com/api/v1";
const GEMINI_MODEL: &str = "gemini-2.5-flash";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunRequest {
    request: String,
    tone3000_api_key: Option<String>,
    gemini_api_key: Option<String>,
    output_dir: Option<String>,
    max_tones: Option<u8>,
    max_results: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct AuthResponse {
    access_token: String,
}

#[derive(Debug, Clone)]
struct Analysis {
    search_queries: Vec<String>,
    gear_type: Option<String>,
    description: String,
    fallback_queries: Vec<String>,
}

impl Analysis {
    fn to_json(&self) -> Value {
        json!({
            "search_queries": self.search_queries,
            "gear_type": self.gear_type,
            "description": self.description,
            "fallback_queries": self.fallback_queries,
        })
    }
}

struct Tone3000Session {
    client: Client,
    access_token: String,
}

impl Tone3000Session {
    async fn authenticate(client: Client, api_key: &str) -> Result<Self, String> {
        let url = format!("{TONE3000_BASE_URL}/auth/session");
        let response = client
            .post(&url)
            .json(&json!({ "api_key": api_key }))
            .send()
            .await
            .map_err(|e| format!("Tone3000 auth request failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Tone3000 auth failed: {e}"))?;

        let auth: AuthResponse = response
            .json()
            .await
            .map_err(|e| format!("Tone3000 auth parse error: {e}"))?;

        Ok(Self {
            client,
            access_token: auth.access_token,
        })
    }

    async fn search_tones(
        &self,
        query: &str,
        gear: Option<&str>,
        page_size: usize,
    ) -> Result<Vec<Value>, String> {
        let mut req = self
            .client
            .get(format!("{TONE3000_BASE_URL}/tones/search"))
            .bearer_auth(&self.access_token)
            .query(&[
                ("query", query),
                ("page_size", &page_size.min(25).to_string()),
                ("sort", "downloads-all-time"),
            ]);

        if let Some(gear_type) = gear {
            if !gear_type.is_empty() {
                req = req.query(&[("gear", gear_type)]);
            }
        }

        let value: Value = req
            .send()
            .await
            .map_err(|e| format!("Tone search request failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Tone search failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("Tone search response parse failed: {e}"))?;

        Ok(value
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    async fn get_models(&self, tone_id: i64) -> Result<Vec<Value>, String> {
        let value: Value = self
            .client
            .get(format!("{TONE3000_BASE_URL}/models"))
            .bearer_auth(&self.access_token)
            .query(&[
                ("tone_id", tone_id.to_string()),
                ("page_size", "100".to_string()),
            ])
            .send()
            .await
            .map_err(|e| format!("Get models request failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Get models failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("Get models parse failed: {e}"))?;

        Ok(value
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    async fn download_model(&self, model_url: &str, output_path: &Path) -> Result<(), String> {
        let mut response = self
            .client
            .get(model_url)
            .bearer_auth(&self.access_token)
            .send()
            .await
            .map_err(|e| format!("Model download request failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Model download failed: {e}"))?;

        let mut file = tokio::fs::File::create(output_path).await.map_err(|e| {
            format!(
                "Failed to create output file {}: {e}",
                output_path.display()
            )
        })?;

        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("Failed while streaming model file: {e}"))?
        {
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("Failed while writing model file: {e}"))?;
        }

        Ok(())
    }
}

fn push_log(logs: &mut String, line: impl AsRef<str>) {
    logs.push_str(line.as_ref());
    logs.push('\n');
}

fn sanitize_line(text: &str) -> String {
    text.replace('\r', " ")
        .replace('\n', " ")
        .trim()
        .to_string()
}

fn value_as_i64(value: Option<&Value>) -> i64 {
    match value {
        Some(v) => {
            if let Some(n) = v.as_i64() {
                n
            } else if let Some(n) = v.as_u64() {
                n as i64
            } else if let Some(s) = v.as_str() {
                s.parse::<i64>().unwrap_or(0)
            } else {
                0
            }
        }
        None => 0,
    }
}

fn value_as_string(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn tone_id(tone: &Value) -> Option<i64> {
    tone.get("id").and_then(|v| {
        if let Some(n) = v.as_i64() {
            Some(n)
        } else if let Some(n) = v.as_u64() {
            Some(n as i64)
        } else if let Some(s) = v.as_str() {
            s.parse::<i64>().ok()
        } else {
            None
        }
    })
}

fn tone_downloads(tone: &Value) -> i64 {
    value_as_i64(tone.get("downloads_count"))
}

fn text_contains_boost(text: &str) -> bool {
    let t = text.to_lowercase();
    if t.is_empty() {
        return false;
    }

    [
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
    ]
    .iter()
    .any(|k| t.contains(k))
}

fn tone_contains_boost(tone: &Value) -> bool {
    if value_as_string(tone.get("gear")).to_lowercase() != "amp" {
        return false;
    }

    let text = format!(
        "{}\n{}",
        value_as_string(tone.get("title")),
        value_as_string(tone.get("description"))
    );
    text_contains_boost(&text)
}

fn tone_is_preamp_or_boost_pedal(tone: &Value) -> bool {
    if value_as_string(tone.get("gear")).to_lowercase() != "pedal" {
        return false;
    }

    let text = format!(
        "{}\n{}",
        value_as_string(tone.get("title")),
        value_as_string(tone.get("description"))
    )
    .to_lowercase();

    [
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
    ]
    .iter()
    .any(|k| text.contains(k))
}

fn postprocess_selected_indices(
    tones: &[Value],
    selected_indices: &[usize],
    max_selections: usize,
) -> Vec<usize> {
    let mut unique = Vec::new();
    let mut seen = HashSet::new();
    for idx in selected_indices {
        if *idx < tones.len() && seen.insert(*idx) {
            unique.push(*idx);
        }
    }

    let amp_has_boost = unique.iter().any(|i| tone_contains_boost(&tones[*i]));

    if amp_has_boost {
        unique.retain(|i| !tone_is_preamp_or_boost_pedal(&tones[*i]));
    }

    if unique.len() >= max_selections {
        unique.truncate(max_selections);
        return unique;
    }

    let mut all_indices: Vec<usize> = (0..tones.len()).collect();
    all_indices.sort_by_key(|i| -tone_downloads(&tones[*i]));

    let mut unique_set: HashSet<usize> = unique.iter().copied().collect();
    for idx in all_indices {
        if unique_set.contains(&idx) {
            continue;
        }
        if amp_has_boost && tone_is_preamp_or_boost_pedal(&tones[idx]) {
            continue;
        }
        unique.push(idx);
        unique_set.insert(idx);
        if unique.len() >= max_selections {
            break;
        }
    }

    unique
}

fn safe_filename(name: &str) -> String {
    let basename = Path::new(name)
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or(name);

    let mut out = String::with_capacity(basename.len());
    for ch in basename.chars() {
        if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
            out.push('_');
        } else {
            out.push(ch);
        }
    }

    let trimmed = out.trim_matches([' ', '.']);
    if trimmed.is_empty() {
        "model".to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalize_model_filename(name: &str, platform: Option<&str>) -> String {
    let basename = safe_filename(name);
    if Path::new(&basename).extension().is_some() {
        return basename;
    }

    if platform.unwrap_or_default().eq_ignore_ascii_case("nam") {
        return format!("{basename}.nam");
    }

    basename
}

fn safe_tone_dir_name(title: &str, tone_id: i64) -> String {
    let mut safe: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    safe = safe.trim().to_string();
    if safe.is_empty() {
        safe = "tone".to_string();
    }
    if safe.len() > 50 {
        safe = safe.chars().take(50).collect();
    }

    format!("{safe}_{tone_id}")
}

fn parse_json_object_segment(text: &str) -> Option<Value> {
    let mut de = serde_json::Deserializer::from_str(text);
    let parsed = Value::deserialize(&mut de).ok()?;
    if parsed.is_object() {
        Some(parsed)
    } else {
        None
    }
}

fn parse_json_object_from_text(text: &str) -> Result<Value, String> {
    let mut raw = text.trim().to_string();
    if raw.is_empty() {
        return Err("Empty Gemini response".to_string());
    }

    if let Some(value) = parse_json_object_segment(&raw) {
        return Ok(value);
    }

    if raw.starts_with("```json") {
        raw = raw
            .split_once("```json")
            .and_then(|(_, rest)| rest.split_once("```"))
            .map(|(inside, _)| inside.trim().to_string())
            .unwrap_or(raw);
    } else if raw.starts_with("```") {
        raw = raw
            .split_once("```")
            .and_then(|(_, rest)| rest.split_once("```"))
            .map(|(inside, _)| inside.trim().to_string())
            .unwrap_or(raw);
    }

    if let Some(value) = parse_json_object_segment(&raw) {
        return Ok(value);
    }

    let single_line = sanitize_line(&raw);
    if let Some(value) = parse_json_object_segment(&single_line) {
        return Ok(value);
    }

    for (idx, ch) in raw.char_indices() {
        if ch != '{' {
            continue;
        }
        if let Some(value) = parse_json_object_segment(&raw[idx..]) {
            return Ok(value);
        }
    }

    Err(format!(
        "Invalid JSON from Gemini: {}",
        raw.chars().take(200).collect::<String>()
    ))
}

fn gemini_response_text(response: &Value) -> String {
    if let Some(text) = response.get("text").and_then(Value::as_str) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    response
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|candidate| candidate.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<&str>>()
                .join("")
                .trim()
                .to_string()
        })
        .unwrap_or_default()
}

async fn gemini_generate_json(
    client: &Client,
    api_key: &str,
    prompt: &str,
) -> Result<Value, String> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        GEMINI_MODEL, api_key
    );

    let mut last_error = String::new();

    for attempt in 0..2 {
        let attempt_prompt = if attempt == 0 {
            prompt.to_string()
        } else {
            format!(
                "{prompt}\n\nIMPORTANT: Your previous response was invalid JSON. Return ONLY valid JSON that matches the required schema. Do not include newlines inside string values."
            )
        };

        let body = json!({
            "contents": [
                {
                    "role": "user",
                    "parts": [{ "text": attempt_prompt }]
                }
            ],
            "generationConfig": {
                "responseMimeType": "application/json",
                "temperature": 0,
                "maxOutputTokens": 1024
            }
        });

        let response: Value = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Gemini request failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Gemini API returned error: {e}"))?
            .json()
            .await
            .map_err(|e| format!("Gemini response parse failed: {e}"))?;

        let text = gemini_response_text(&response);
        match parse_json_object_from_text(&text) {
            Ok(value) => return Ok(value),
            Err(e) => last_error = e,
        }
    }

    Err(format!(
        "Failed to get valid JSON from Gemini: {last_error}"
    ))
}

async fn analyze_tone_request(
    client: &Client,
    gemini_api_key: &str,
    user_request: &str,
    logs: &mut String,
) -> Result<Analysis, String> {
    let prompt = format!(
        r#"
Kullanıcı şu tonu arıyor: "{}"

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
"#,
        sanitize_line(user_request)
    );

    push_log(logs, "🤖 Gemini analyzing request...");
    let raw = gemini_generate_json(client, gemini_api_key, &prompt).await?;

    let search_queries: Vec<String> = raw
        .get("search_queries")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(sanitize_line)
                .filter(|s| !s.is_empty())
                .take(3)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    let fallback_queries: Vec<String> = raw
        .get("fallback_queries")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(sanitize_line)
                .filter(|s| !s.is_empty())
                .take(3)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    let mut normalized_search = search_queries;
    if normalized_search.is_empty() {
        normalized_search.push(sanitize_line(user_request));
    }

    let gear_type = raw
        .get("gear_type")
        .and_then(Value::as_str)
        .map(sanitize_line)
        .and_then(|g| {
            if ["amp", "ir", "pedal"].contains(&g.as_str()) {
                Some(g)
            } else {
                None
            }
        });

    let description = raw
        .get("description")
        .and_then(Value::as_str)
        .map(sanitize_line)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "İstek analizi tamamlandı".to_string());

    push_log(logs, format!("✓ Analysis: {description}"));
    push_log(
        logs,
        format!("  Search queries: {}", normalized_search.join(", ")),
    );
    if !fallback_queries.is_empty() {
        push_log(
            logs,
            format!("  Fallback queries: {}", fallback_queries.join(", ")),
        );
    }
    push_log(
        logs,
        format!(
            "  Gear type: {}",
            gear_type.clone().unwrap_or_else(|| "all".to_string())
        ),
    );

    Ok(Analysis {
        search_queries: normalized_search,
        gear_type,
        description,
        fallback_queries,
    })
}

async fn select_best_tones(
    client: &Client,
    gemini_api_key: &str,
    user_request: &str,
    tones: &[Value],
    max_selections: usize,
    logs: &mut String,
) -> Result<Vec<Value>, String> {
    if tones.is_empty() {
        return Ok(Vec::new());
    }

    let mut candidates = tones.to_vec();
    candidates.sort_by_key(|t| -tone_downloads(t));
    candidates.truncate(15);

    let summaries: Vec<Value> = candidates
        .iter()
        .enumerate()
        .map(|(i, tone)| {
            let description = sanitize_line(&value_as_string(tone.get("description")));
            json!({
                "index": i,
                "title": value_as_string(tone.get("title")),
                "description": description.chars().take(160).collect::<String>(),
                "gear": value_as_string(tone.get("gear")),
                "platform": value_as_string(tone.get("platform")),
                "downloads": tone_downloads(tone),
                "contains_boost_in_chain": tone_contains_boost(tone),
                "is_preamp_or_boost_pedal": tone_is_preamp_or_boost_pedal(tone),
            })
        })
        .collect();

    let summaries_json = serde_json::to_string(&summaries)
        .map_err(|e| format!("Failed to serialize tone summaries: {e}"))?;

    let prompt = format!(
        r#"
Kullanıcı şu tonu arıyor: "{}"

Bulunan tonlar:
{}

Bu tonlardan EN UYGUN {} tanesini seç.
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
"#,
        sanitize_line(user_request),
        summaries_json,
        max_selections
    );

    push_log(
        logs,
        format!(
            "🤖 Gemini selecting best tones from {} results...",
            tones.len()
        ),
    );
    let raw = gemini_generate_json(client, gemini_api_key, &prompt).await?;

    let raw_indices: Vec<usize> = raw
        .get("selected_indices")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    if let Some(n) = v.as_u64() {
                        Some(n as usize)
                    } else if let Some(n) = v.as_i64() {
                        if n >= 0 {
                            Some(n as usize)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect::<Vec<usize>>()
        })
        .unwrap_or_default();

    let indices = postprocess_selected_indices(&candidates, &raw_indices, max_selections);
    push_log(logs, format!("✓ Selected {} tones", indices.len()));

    Ok(indices.iter().map(|idx| candidates[*idx].clone()).collect())
}

async fn filter_models(
    client: &Client,
    gemini_api_key: &str,
    user_request: &str,
    tone_title: &str,
    tone_description: &str,
    models: &[Value],
) -> Result<Vec<Value>, String> {
    let summaries: Vec<Value> = models
        .iter()
        .enumerate()
        .map(|(i, model)| {
            json!({
                "index": i,
                "name": value_as_string(model.get("name")),
                "size": value_as_string(model.get("size")),
            })
        })
        .collect();

    let summaries_json = serde_json::to_string(&summaries)
        .map_err(|e| format!("Failed to serialize model summaries: {e}"))?;

    let prompt = format!(
        r#"
Kullanıcı şu tonu arıyor: "{}"
Ton: "{}"
Açıklama: "{}"

Bu ton için şu modeller mevcut:
{}

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
"#,
        sanitize_line(user_request),
        sanitize_line(tone_title),
        sanitize_line(tone_description),
        summaries_json
    );

    let raw = gemini_generate_json(client, gemini_api_key, &prompt).await?;
    let mut indices: Vec<usize> = raw
        .get("selected_indices")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    if let Some(n) = v.as_u64() {
                        Some(n as usize)
                    } else if let Some(n) = v.as_i64() {
                        if n >= 0 {
                            Some(n as usize)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect::<Vec<usize>>()
        })
        .unwrap_or_default();

    indices.retain(|i| *i < models.len());
    indices.truncate(5);

    Ok(indices.into_iter().map(|i| models[i].clone()).collect())
}

fn summarize_tone(tone: &Value) -> Value {
    let author = tone
        .get("user")
        .and_then(|u| u.get("username"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    json!({
        "id": tone_id(tone),
        "title": value_as_string(tone.get("title")),
        "description": value_as_string(tone.get("description")),
        "gear": value_as_string(tone.get("gear")),
        "platform": value_as_string(tone.get("platform")),
        "downloads_count": tone_downloads(tone),
        "author": author,
        "url": value_as_string(tone.get("url")),
    })
}

fn read_keys_file(path: &Path) -> HashMap<String, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return HashMap::new(),
    };

    let mut out = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            out.insert(
                k.trim().to_string(),
                v.trim().trim_matches('"').trim_matches('\'').to_string(),
            );
        }
    }

    out
}

fn resolve_keys(payload: &RunRequest, repo_root: &Path) -> Result<(String, String), String> {
    let keys_file = read_keys_file(&repo_root.join("keys.txt"));

    let tone_key = payload
        .tone3000_api_key
        .as_ref()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| env::var("TONE3000_API_KEY").ok())
        .or_else(|| keys_file.get("TONE3000_API_KEY").cloned());

    let gemini_key = payload
        .gemini_api_key
        .as_ref()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| env::var("GEMINI_API_KEY").ok())
        .or_else(|| keys_file.get("GEMINI_API_KEY").cloned());

    match (tone_key, gemini_key) {
        (Some(tone), Some(gemini)) => Ok((tone, gemini)),
        _ => Err(
            "Missing API keys. Provide both TONE3000 and Gemini keys from UI, env vars, or keys.txt."
                .to_string(),
        ),
    }
}

async fn build_tone_pool(
    session: &Tone3000Session,
    analysis: &Analysis,
    max_results_to_analyze: usize,
    logs: &mut String,
) -> Result<Vec<Value>, String> {
    let mut all_tones: Vec<Value> = Vec::new();
    let mut seen_ids: HashSet<i64> = HashSet::new();

    for query in &analysis.search_queries {
        push_log(logs, format!("🔍 Searching: {query}"));
        let result = session
            .search_tones(query, analysis.gear_type.as_deref(), 25)
            .await?;

        let mut added_count = 0usize;
        for tone in result.iter().take(max_results_to_analyze) {
            let Some(id) = tone_id(tone) else {
                continue;
            };
            if seen_ids.insert(id) {
                all_tones.push(tone.clone());
                added_count += 1;
            }
        }

        push_log(
            logs,
            format!("  Found {} tones (added {} new)", result.len(), added_count),
        );
    }

    if all_tones.len() < 10 && !analysis.fallback_queries.is_empty() {
        push_log(
            logs,
            "⚠️ Not enough tones found, trying fallback searches...",
        );

        for query in &analysis.fallback_queries {
            if all_tones.len() >= max_results_to_analyze {
                break;
            }

            push_log(logs, format!("🔍 Fallback search: {query}"));
            let result = session
                .search_tones(query, analysis.gear_type.as_deref(), 25)
                .await?;

            let mut added_count = 0usize;
            for tone in result.iter().take(max_results_to_analyze) {
                let Some(id) = tone_id(tone) else {
                    continue;
                };
                if seen_ids.insert(id) {
                    all_tones.push(tone.clone());
                    added_count += 1;
                }
            }

            push_log(
                logs,
                format!("  Found {} tones (added {} new)", result.len(), added_count),
            );
        }
    }

    Ok(all_tones)
}

async fn run_download_inner(payload: RunRequest) -> Result<Value, String> {
    let request = sanitize_line(&payload.request);
    let max_tones = payload.max_tones.unwrap_or(3).clamp(1, 5) as usize;
    let max_results = payload.max_results.unwrap_or(15).clamp(5, 25) as usize;

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .ok_or_else(|| "Failed to locate repository root.".to_string())?
        .to_path_buf();

    let (tone_api_key, gemini_api_key) = resolve_keys(&payload, &repo_root)?;

    let output_dir_raw = payload
        .output_dir
        .clone()
        .unwrap_or_else(|| "./smart_downloaded_tones".to_string());
    let output_dir = if Path::new(&output_dir_raw).is_absolute() {
        PathBuf::from(&output_dir_raw)
    } else {
        repo_root.join(&output_dir_raw)
    };

    std::fs::create_dir_all(&output_dir).map_err(|e| {
        format!(
            "Failed to create output directory {}: {e}",
            output_dir.display()
        )
    })?;

    let client = Client::builder()
        .user_agent("tone3000-smart-tone-downloader-tauri")
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;

    let mut logs = String::new();
    push_log(&mut logs, format!("🎸 Smart Tone Download: {request}"));

    let session = Tone3000Session::authenticate(client.clone(), &tone_api_key).await?;
    push_log(&mut logs, "✓ TONE3000 authenticated");
    push_log(&mut logs, "✓ Gemini 2.5 Flash initialized");

    let analysis = analyze_tone_request(&client, &gemini_api_key, &request, &mut logs).await?;

    let tone_pool = build_tone_pool(&session, &analysis, max_results, &mut logs).await?;
    if tone_pool.is_empty() {
        push_log(&mut logs, "❌ No tones found!");
        return Ok(json!({
            "ok": true,
            "request": request,
            "analysis": analysis.to_json(),
            "pool_size": 0,
            "selected_tones": [],
            "downloaded_count": 0,
            "model_items": [],
            "output_dir": output_dir.to_string_lossy().to_string(),
            "logs": logs,
        }));
    }

    push_log(
        &mut logs,
        format!("📊 Total unique tones found: {}", tone_pool.len()),
    );

    let selected_tones = select_best_tones(
        &client,
        &gemini_api_key,
        &request,
        &tone_pool,
        max_tones,
        &mut logs,
    )
    .await?;

    let mut downloaded_count = 0usize;
    let mut model_items: Vec<Value> = Vec::new();

    for tone in &selected_tones {
        let id = tone_id(tone).unwrap_or_default();
        let title = value_as_string(tone.get("title"));
        let tone_dir = output_dir.join(safe_tone_dir_name(&title, id));
        std::fs::create_dir_all(&tone_dir).map_err(|e| {
            format!(
                "Failed to create tone directory {}: {e}",
                tone_dir.display()
            )
        })?;

        let info_json = serde_json::to_string_pretty(tone)
            .map_err(|e| format!("Failed to serialize tone info: {e}"))?;
        std::fs::write(tone_dir.join("info.json"), info_json)
            .map_err(|e| format!("Failed to write tone info file: {e}"))?;

        let all_models = session.get_models(id).await?;
        push_log(
            &mut logs,
            format!(
                "  Tone '{title}' total models available: {}",
                all_models.len()
            ),
        );

        let selected_models = filter_models(
            &client,
            &gemini_api_key,
            &request,
            &title,
            &value_as_string(tone.get("description")),
            &all_models,
        )
        .await?;

        push_log(
            &mut logs,
            format!("    🤖 Selected {} models", selected_models.len()),
        );

        for model in selected_models {
            let model_name = value_as_string(model.get("name"));
            let filename =
                normalize_model_filename(&model_name, tone.get("platform").and_then(Value::as_str));
            let target_path = tone_dir.join(&filename);

            if target_path.exists() {
                let size_mb = std::fs::metadata(&target_path)
                    .ok()
                    .map(|m| m.len() as f64 / (1024_f64 * 1024_f64))
                    .unwrap_or(0.0);

                model_items.push(json!({
                    "tone_id": id,
                    "tone_title": title,
                    "model_name": filename,
                    "status": "skipped_exists",
                    "path": target_path.to_string_lossy().to_string(),
                    "size_mb": (size_mb * 100.0).round() / 100.0,
                }));
                continue;
            }

            let model_url = value_as_string(model.get("model_url"));
            if model_url.is_empty() {
                push_log(
                    &mut logs,
                    format!("    ✗ Missing model_url for model: {model_name}"),
                );
                model_items.push(json!({
                    "tone_id": id,
                    "tone_title": title,
                    "model_name": filename,
                    "status": "error",
                    "path": target_path.to_string_lossy().to_string(),
                    "size_mb": 0,
                }));
                continue;
            }

            match session.download_model(&model_url, &target_path).await {
                Ok(_) => {
                    let size_mb = std::fs::metadata(&target_path)
                        .ok()
                        .map(|m| m.len() as f64 / (1024_f64 * 1024_f64))
                        .unwrap_or(0.0);
                    downloaded_count += 1;
                    model_items.push(json!({
                        "tone_id": id,
                        "tone_title": title,
                        "model_name": filename,
                        "status": "downloaded",
                        "path": target_path.to_string_lossy().to_string(),
                        "size_mb": (size_mb * 100.0).round() / 100.0,
                    }));
                }
                Err(err) => {
                    push_log(&mut logs, format!("    ✗ Download error: {err}"));
                    model_items.push(json!({
                        "tone_id": id,
                        "tone_title": title,
                        "model_name": filename,
                        "status": "error",
                        "path": target_path.to_string_lossy().to_string(),
                        "size_mb": 0,
                    }));
                }
            }
        }
    }

    Ok(json!({
        "ok": true,
        "request": request,
        "analysis": analysis.to_json(),
        "pool_size": tone_pool.len(),
        "selected_tones": selected_tones.iter().map(summarize_tone).collect::<Vec<Value>>(),
        "downloaded_count": downloaded_count,
        "model_items": model_items,
        "output_dir": output_dir.to_string_lossy().to_string(),
        "logs": logs,
    }))
}

#[tauri::command]
async fn run_download(payload: RunRequest) -> Result<Value, String> {
    if payload.request.trim().is_empty() {
        return Ok(json!({
            "ok": false,
            "error": "Request text is required."
        }));
    }

    match run_download_inner(payload).await {
        Ok(response) => Ok(response),
        Err(error) => Ok(json!({
            "ok": false,
            "error": error,
        })),
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![run_download])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
