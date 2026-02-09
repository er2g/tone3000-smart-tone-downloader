#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

const TONE3000_BASE_URL: &str = "https://www.tone3000.com/api/v1";
const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-pro";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunRequest {
    request: String,
    tone3000_api_key: Option<String>,
    gemini_api_key: Option<String>,
    gemini_model: Option<String>,
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
    explanation_steps: Vec<String>,
}

impl Analysis {
    fn to_json(&self) -> Value {
        json!({
            "search_queries": self.search_queries,
            "gear_type": self.gear_type,
            "description": self.description,
            "fallback_queries": self.fallback_queries,
            "explanation_steps": self.explanation_steps,
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

fn normalize_gemini_model(requested_model: Option<&str>) -> String {
    let fallback = DEFAULT_GEMINI_MODEL.to_string();
    let Some(raw_model) = requested_model.map(str::trim) else {
        return fallback;
    };
    if raw_model.is_empty() {
        return fallback;
    }

    let is_allowed = raw_model
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_');
    if !is_allowed {
        return fallback;
    }

    raw_model.to_string()
}

fn parse_explanation_lines(value: Option<&Value>, max_items: usize) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(sanitize_line)
                .filter(|line| !line.is_empty())
                .take(max_items)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
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
    gemini_model: &str,
    prompt: &str,
) -> Result<Value, String> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        gemini_model, api_key
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
    gemini_model: &str,
    user_request: &str,
    logs: &mut String,
) -> Result<Analysis, String> {
    let prompt = format!(
        r#"
User request: "{}"

Extract practical tone search terms and explain your reasoning for a beginner guitarist.
Rules:
- Choose realistic, searchable tone terms.
- `search_queries`: max 3 short queries.
- `fallback_queries`: max 3 alternate queries.
- `gear_type`: "amp", "ir", "pedal", or null.
- `description`: one-line summary of the intended tone.
- `explanation_steps`: 3-5 concise one-line steps.
- Every string must be single-line (no newline in values).

Return only JSON:
{{
  "search_queries": ["query1", "query2"],
  "gear_type": "amp",
  "description": "Short summary",
  "fallback_queries": ["alt1", "alt2"],
  "explanation_steps": ["step 1", "step 2", "step 3"]
}}
"#,
        sanitize_line(user_request)
    );

    push_log(logs, "Gemini analyzing request...");
    let raw = match gemini_generate_json(client, gemini_api_key, gemini_model, &prompt).await {
        Ok(value) => value,
        Err(err) => {
            push_log(
                logs,
                format!("  Warning: Gemini analysis fallback used: {err}"),
            );
            json!({
                "search_queries": [sanitize_line(user_request)],
                "gear_type": Value::Null,
                "description": "Fallback analysis used because Gemini response was invalid.",
                "fallback_queries": [],
                "explanation_steps": [
                    "Gemini did not return valid JSON for analysis.",
                    "Used the original user request directly as the main search query.",
                    "Continued with neutral gear filter."
                ]
            })
        }
    };

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
        .unwrap_or_else(|| "Request analysis completed".to_string());

    let explanation_steps = {
        let mut steps = parse_explanation_lines(raw.get("explanation_steps"), 5);
        if steps.is_empty() {
            steps.push(format!(
                "Target tone request: {}",
                sanitize_line(user_request)
            ));
            steps.push(format!(
                "Primary search queries: {}",
                normalized_search.join(", ")
            ));
            if let Some(gear) = &gear_type {
                steps.push(format!("Focus on gear type: {gear}"));
            }
        }
        steps
    };

    push_log(logs, format!("OK Analysis: {description}"));
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
    for (idx, step) in explanation_steps.iter().enumerate() {
        push_log(logs, format!("  Analysis step {}: {}", idx + 1, step));
    }

    Ok(Analysis {
        search_queries: normalized_search,
        gear_type,
        description,
        fallback_queries,
        explanation_steps,
    })
}

async fn select_best_tones(
    client: &Client,
    gemini_api_key: &str,
    gemini_model: &str,
    user_request: &str,
    tones: &[Value],
    max_selections: usize,
    logs: &mut String,
) -> Result<(Vec<Value>, Vec<String>), String> {
    if tones.is_empty() {
        return Ok((Vec::new(), Vec::new()));
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
User request: "{}"

Candidate tones:
{}

Choose the best {} tones.
Selection criteria:
- Relevance to requested artist/song/tone character.
- Popularity and reliability (downloads).
- Avoid redundant boost/pedal picks when amp profile already includes boost/OD.
- Use only listed indexes.

Return only JSON:
{{
  "selected_indices": [0, 2],
  "selection_reasons": [
    {{ "index": 0, "reason": "Closest match for requested mid-gain tone." }},
    {{ "index": 2, "reason": "Popular profile and similar voicing." }}
  ]
}}
"#,
        sanitize_line(user_request),
        summaries_json,
        max_selections
    );

    push_log(
        logs,
        format!(
            "Gemini selecting best tones from {} results...",
            tones.len()
        ),
    );
    let raw = match gemini_generate_json(client, gemini_api_key, gemini_model, &prompt).await {
        Ok(value) => value,
        Err(err) => {
            push_log(
                logs,
                format!("  Warning: Gemini tone selection fallback used: {err}"),
            );
            let indices = candidates
                .iter()
                .enumerate()
                .take(max_selections)
                .map(|(i, _)| i)
                .collect::<Vec<usize>>();
            let selected_tones = indices
                .iter()
                .map(|idx| candidates[*idx].clone())
                .collect::<Vec<Value>>();
            let reasons = selected_tones
                .iter()
                .map(|tone| {
                    format!(
                        "{} selected by fallback ranking (top downloads).",
                        value_as_string(tone.get("title"))
                    )
                })
                .collect::<Vec<String>>();
            return Ok((selected_tones, reasons));
        }
    };

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

    let reason_map: HashMap<usize, String> = raw
        .get("selection_reasons")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let index = item.get("index").and_then(Value::as_u64)? as usize;
                    let reason = item
                        .get("reason")
                        .and_then(Value::as_str)
                        .map(sanitize_line)?;
                    if reason.is_empty() {
                        None
                    } else {
                        Some((index, reason))
                    }
                })
                .collect::<HashMap<usize, String>>()
        })
        .unwrap_or_default();

    let indices = postprocess_selected_indices(&candidates, &raw_indices, max_selections);
    let selected_tones = indices
        .iter()
        .map(|idx| candidates[*idx].clone())
        .collect::<Vec<Value>>();

    let mut reasons = Vec::new();
    for idx in &indices {
        let fallback_reason = format!(
            "{} selected for relevance and popularity.",
            value_as_string(candidates[*idx].get("title"))
        );
        reasons.push(reason_map.get(idx).cloned().unwrap_or(fallback_reason));
    }

    push_log(logs, format!("OK Selected {} tones", selected_tones.len()));
    for (idx, reason) in reasons.iter().enumerate() {
        push_log(logs, format!("  Tone choice {}: {}", idx + 1, reason));
    }

    Ok((selected_tones, reasons))
}

async fn filter_models(
    client: &Client,
    gemini_api_key: &str,
    gemini_model: &str,
    user_request: &str,
    tone_title: &str,
    tone_description: &str,
    tone_gear: &str,
    models: &[Value],
) -> Result<(Vec<Value>, Vec<String>), String> {
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
User request: "{}"
Tone title: "{}"
Tone description: "{}"
Tone gear: "{}"

Available models:
{}

Select only useful models for this request.
Constraints:
- If tone gear is `amp`: avoid irrelevant gain channels.
- If tone gear is `ir`: prioritize practical cabinet choices for this amp context.
- Prefer practical model variants.
- Select max 5 models (for `ir`, prefer 1-2 unless multiple are clearly needed).

Return only JSON:
{{
  "selected_indices": [0, 2],
  "model_reasons": [
    {{ "index": 0, "reason": "Main channel matches requested tone." }},
    {{ "index": 2, "reason": "Alternative gain level for flexibility." }}
  ]
}}
"#,
        sanitize_line(user_request),
        sanitize_line(tone_title),
        sanitize_line(tone_description),
        sanitize_line(tone_gear),
        summaries_json
    );

    let raw = match gemini_generate_json(client, gemini_api_key, gemini_model, &prompt).await {
        Ok(value) => value,
        Err(err) => {
            let fallback_indices = models
                .iter()
                .enumerate()
                .take(2)
                .map(|(i, _)| i)
                .collect::<Vec<usize>>();
            let fallback_models = fallback_indices
                .iter()
                .map(|i| models[*i].clone())
                .collect::<Vec<Value>>();
            let fallback_reasons = fallback_models
                .iter()
                .map(|m| {
                    format!(
                        "{} selected by fallback because Gemini response was invalid: {err}",
                        value_as_string(m.get("name"))
                    )
                })
                .collect::<Vec<String>>();
            return Ok((fallback_models, fallback_reasons));
        }
    };
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

    let reason_map: HashMap<usize, String> = raw
        .get("model_reasons")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let index = item.get("index").and_then(Value::as_u64)? as usize;
                    let reason = item
                        .get("reason")
                        .and_then(Value::as_str)
                        .map(sanitize_line)?;
                    if reason.is_empty() {
                        None
                    } else {
                        Some((index, reason))
                    }
                })
                .collect::<HashMap<usize, String>>()
        })
        .unwrap_or_default();

    indices.retain(|i| *i < models.len());
    indices.truncate(5);
    if indices.is_empty() && !models.is_empty() {
        indices.push(0);
    }

    let selected_models = indices
        .iter()
        .map(|i| models[*i].clone())
        .collect::<Vec<Value>>();

    let reasons = indices
        .iter()
        .map(|i| {
            reason_map.get(i).cloned().unwrap_or_else(|| {
                format!(
                    "{} kept as a useful match for this tone.",
                    value_as_string(models[*i].get("name"))
                )
            })
        })
        .collect::<Vec<String>>();

    Ok((selected_models, reasons))
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
    let mut raw_lines: Vec<String> = Vec::new();
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
        } else {
            raw_lines.push(trimmed.to_string());
        }
    }

    if !out.contains_key("TONE3000_API_KEY") {
        if let Some(raw_tone_key) = raw_lines.first() {
            out.insert("TONE3000_API_KEY".to_string(), raw_tone_key.to_string());
        }
    }
    if !out.contains_key("GEMINI_API_KEY") {
        if let Some(raw_gemini_key) = raw_lines.get(1) {
            out.insert("GEMINI_API_KEY".to_string(), raw_gemini_key.to_string());
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

fn dedupe_non_empty_queries(queries: Vec<String>, max_items: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for query in queries {
        let normalized = sanitize_line(&query);
        if normalized.is_empty() {
            continue;
        }
        let key = normalized.to_lowercase();
        if seen.insert(key) {
            out.push(normalized);
        }
        if out.len() >= max_items {
            break;
        }
    }
    out
}

async fn build_gear_pool(
    session: &Tone3000Session,
    primary_queries: &[String],
    fallback_queries: &[String],
    gear: &str,
    max_results_to_analyze: usize,
    logs: &mut String,
) -> Result<Vec<Value>, String> {
    let mut all_tones: Vec<Value> = Vec::new();
    let mut seen_ids: HashSet<i64> = HashSet::new();

    for query in primary_queries {
        push_log(logs, format!("Searching {gear}: {query}"));
        let result = session.search_tones(query, Some(gear), 25).await?;
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
            format!(
                "  Found {} {gear} tones (added {} new)",
                result.len(),
                added_count
            ),
        );
    }

    if all_tones.len() < 10 {
        for query in fallback_queries {
            if all_tones.len() >= max_results_to_analyze {
                break;
            }
            push_log(logs, format!("Fallback {gear} search: {query}"));
            let result = session.search_tones(query, Some(gear), 25).await?;
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
                format!(
                    "  Found {} {gear} tones (added {} new)",
                    result.len(),
                    added_count
                ),
            );
        }
    }

    Ok(all_tones)
}

fn amp_description_text(amp_tone: &Value) -> String {
    format!(
        "{}\n{}",
        value_as_string(amp_tone.get("title")),
        value_as_string(amp_tone.get("description"))
    )
    .to_lowercase()
}

fn fallback_amp_needs_cab(amp_tone: &Value) -> (bool, String) {
    let text = amp_description_text(amp_tone);
    let has_cab_indicator = [
        "cab included",
        "with cab",
        "miked cab",
        "mic'd cab",
        "cab sim",
        "cabinet sim",
        "merged profile",
        "full rig",
    ]
    .iter()
    .any(|k| text.contains(k));

    if has_cab_indicator {
        return (
            false,
            "Fallback: amp description suggests a cab section is already included.".to_string(),
        );
    }

    let head_only_indicator = [
        "head only",
        "no cab",
        "without cab",
        "preamp only",
        "amp head",
    ]
    .iter()
    .any(|k| text.contains(k));

    if head_only_indicator {
        return (
            true,
            "Fallback: amp description looks head/preamp only, so cab is required.".to_string(),
        );
    }

    (
        true,
        "Fallback: cab requirement is unclear, selecting a cab for safer rig completion."
            .to_string(),
    )
}

async fn assess_amp_needs_cab(
    client: &Client,
    gemini_api_key: &str,
    gemini_model: &str,
    user_request: &str,
    amp_tone: &Value,
    logs: &mut String,
) -> Result<(bool, String), String> {
    let tone_title = value_as_string(amp_tone.get("title"));
    let tone_description = sanitize_line(&value_as_string(amp_tone.get("description")));
    let prompt = format!(
        r#"
User request: "{}"
Amp candidate title: "{}"
Amp candidate description: "{}"

Decide if this amp profile needs an external cab/IR to complete the rig.
Use natural judgement from the text (do not apply strict keyword-only logic).

Return only JSON:
{{
  "needs_cab": true,
  "reason": "Short explanation"
}}
"#,
        sanitize_line(user_request),
        sanitize_line(&tone_title),
        tone_description
    );

    let raw = match gemini_generate_json(client, gemini_api_key, gemini_model, &prompt).await {
        Ok(value) => value,
        Err(err) => {
            let fallback = fallback_amp_needs_cab(amp_tone);
            push_log(
                logs,
                format!(
                    "  Warning: cab decision fallback for '{}': {}",
                    tone_title, err
                ),
            );
            return Ok(fallback);
        }
    };

    let needs_cab = raw
        .get("needs_cab")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let reason = raw
        .get("reason")
        .and_then(Value::as_str)
        .map(sanitize_line)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if needs_cab {
                "Cab selected to complete the rig.".to_string()
            } else {
                "Amp profile appears complete without extra cab.".to_string()
            }
        });

    Ok((needs_cab, reason))
}

async fn select_best_cab_for_amp(
    client: &Client,
    gemini_api_key: &str,
    gemini_model: &str,
    user_request: &str,
    amp_tone: &Value,
    cab_candidates: &[Value],
) -> Result<Option<(Value, String)>, String> {
    if cab_candidates.is_empty() {
        return Ok(None);
    }

    let summaries: Vec<Value> = cab_candidates
        .iter()
        .enumerate()
        .map(|(i, tone)| {
            json!({
                "index": i,
                "title": value_as_string(tone.get("title")),
                "description": sanitize_line(&value_as_string(tone.get("description"))),
                "downloads": tone_downloads(tone),
                "platform": value_as_string(tone.get("platform")),
            })
        })
        .collect();

    let summaries_json = serde_json::to_string(&summaries)
        .map_err(|e| format!("Failed to serialize cab candidates: {e}"))?;
    let prompt = format!(
        r#"
User request: "{}"
Selected amp: "{}" / "{}"

Choose the best matching cab/IR from these candidates:
{}

Return only JSON:
{{
  "selected_index": 0,
  "reason": "Short explanation"
}}
"#,
        sanitize_line(user_request),
        sanitize_line(&value_as_string(amp_tone.get("title"))),
        sanitize_line(&value_as_string(amp_tone.get("description"))),
        summaries_json
    );

    let raw = gemini_generate_json(client, gemini_api_key, gemini_model, &prompt).await;
    let (selected_index, reason) = match raw {
        Ok(value) => {
            let idx = value
                .get("selected_index")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .filter(|i| *i < cab_candidates.len())
                .unwrap_or(0);
            let reason = value
                .get("reason")
                .and_then(Value::as_str)
                .map(sanitize_line)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Selected as best cab/IR match for the amp.".to_string());
            (idx, reason)
        }
        Err(err) => (
            0,
            format!("Fallback cab selection by popularity (Gemini issue: {err})"),
        ),
    };

    Ok(Some((cab_candidates[selected_index].clone(), reason)))
}

async fn download_models_for_tone_component(
    session: &Tone3000Session,
    client: &Client,
    gemini_api_key: &str,
    gemini_model: &str,
    user_request: &str,
    tone: &Value,
    component_role: &str,
    preset_label: &str,
    preset_dir: &Path,
    ai_steps: &mut Vec<Value>,
    model_items: &mut Vec<Value>,
    downloaded_count: &mut usize,
    logs: &mut String,
) -> Result<(), String> {
    let id = tone_id(tone).unwrap_or_default();
    let title = value_as_string(tone.get("title"));
    let gear = value_as_string(tone.get("gear"));
    let component_dir = preset_dir.join(format!(
        "{}_{}",
        component_role,
        safe_tone_dir_name(&title, id)
    ));
    std::fs::create_dir_all(&component_dir).map_err(|e| {
        format!(
            "Failed to create component directory {}: {e}",
            component_dir.display()
        )
    })?;

    let info_json = serde_json::to_string_pretty(tone)
        .map_err(|e| format!("Failed to serialize tone info: {e}"))?;
    std::fs::write(component_dir.join("info.json"), info_json)
        .map_err(|e| format!("Failed to write tone info file: {e}"))?;

    let all_models = session.get_models(id).await?;
    push_log(
        logs,
        format!(
            "  [{preset_label}] {component_role} '{title}' total models available: {}",
            all_models.len()
        ),
    );

    let (selected_models, model_reasons) = filter_models(
        client,
        gemini_api_key,
        gemini_model,
        user_request,
        &title,
        &value_as_string(tone.get("description")),
        &gear,
        &all_models,
    )
    .await?;

    ai_steps.push(json!({
        "step": ai_steps.len() + 1,
        "title": format!("{preset_label} {component_role} model filtering: {title}"),
        "details": model_reasons,
    }));

    for model in selected_models {
        let model_name = value_as_string(model.get("name"));
        let filename =
            normalize_model_filename(&model_name, tone.get("platform").and_then(Value::as_str));
        let target_path = component_dir.join(&filename);

        if target_path.exists() {
            let size_mb = std::fs::metadata(&target_path)
                .ok()
                .map(|m| m.len() as f64 / (1024_f64 * 1024_f64))
                .unwrap_or(0.0);
            model_items.push(json!({
                "preset": preset_label,
                "component_role": component_role,
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
                logs,
                format!("    [{preset_label}] Missing model_url for model: {model_name}"),
            );
            model_items.push(json!({
                "preset": preset_label,
                "component_role": component_role,
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
                *downloaded_count += 1;
                model_items.push(json!({
                    "preset": preset_label,
                    "component_role": component_role,
                    "tone_id": id,
                    "tone_title": title,
                    "model_name": filename,
                    "status": "downloaded",
                    "path": target_path.to_string_lossy().to_string(),
                    "size_mb": (size_mb * 100.0).round() / 100.0,
                }));
            }
            Err(err) => {
                push_log(logs, format!("    [{preset_label}] Download error: {err}"));
                model_items.push(json!({
                    "preset": preset_label,
                    "component_role": component_role,
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

    Ok(())
}

async fn run_download_inner(payload: RunRequest) -> Result<Value, String> {
    let request = sanitize_line(&payload.request);
    let max_tones = payload.max_tones.unwrap_or(3).clamp(1, 5) as usize;
    let max_results = payload.max_results.unwrap_or(15).clamp(5, 25) as usize;
    let gemini_model = normalize_gemini_model(payload.gemini_model.as_deref());

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
    let mut ai_steps: Vec<Value> = Vec::new();

    push_log(&mut logs, format!("Smart Tone Rig Download: {request}"));

    let session = Tone3000Session::authenticate(client.clone(), &tone_api_key).await?;
    push_log(&mut logs, "OK TONE3000 authenticated");
    push_log(
        &mut logs,
        format!("OK Gemini model initialized: {gemini_model}"),
    );

    let analysis =
        analyze_tone_request(&client, &gemini_api_key, &gemini_model, &request, &mut logs).await?;

    ai_steps.push(json!({
        "step": 1,
        "title": "Request analysis",
        "details": analysis.explanation_steps,
    }));

    let amp_primary_queries = dedupe_non_empty_queries(
        {
            let mut queries = analysis.search_queries.clone();
            queries.push(request.clone());
            queries
        },
        6,
    );
    let amp_fallback_queries = dedupe_non_empty_queries(
        {
            let mut queries = analysis.fallback_queries.clone();
            queries.push(format!("{} amp", request));
            queries
        },
        6,
    );

    let mut amp_pool = build_gear_pool(
        &session,
        &amp_primary_queries,
        &amp_fallback_queries,
        "amp",
        max_results,
        &mut logs,
    )
    .await?;

    if amp_pool.is_empty() {
        push_log(
            &mut logs,
            "No amp found with strict amp filter, trying relaxed search...",
        );
        let relaxed_pool = build_tone_pool(&session, &analysis, max_results, &mut logs).await?;
        amp_pool = relaxed_pool
            .into_iter()
            .filter(|tone| value_as_string(tone.get("gear")).eq_ignore_ascii_case("amp"))
            .collect::<Vec<Value>>();
    }

    ai_steps.push(json!({
        "step": 2,
        "title": "Amp search and pooling",
        "details": [
            format!("Amp queries used: {}", amp_primary_queries.join(", ")),
            format!("Amp pool size: {}", amp_pool.len()),
            format!("Target preset count: {}", max_tones),
        ],
    }));

    if amp_pool.is_empty() {
        push_log(&mut logs, "No amp tones found");
        ai_steps.push(json!({
            "step": 3,
            "title": "No result",
            "details": ["No amp candidate found. Try broader artist/song keywords."],
        }));

        return Ok(json!({
            "ok": true,
            "request": request,
            "analysis": analysis.to_json(),
            "gemini_model": gemini_model,
            "pool_size": 0,
            "selected_tones": [],
            "rig_presets": [],
            "downloaded_count": 0,
            "model_items": [],
            "ai_steps": ai_steps,
            "output_dir": output_dir.to_string_lossy().to_string(),
            "logs": logs,
        }));
    }

    let (selected_amps, amp_reasons) = select_best_tones(
        &client,
        &gemini_api_key,
        &gemini_model,
        &request,
        &amp_pool,
        max_tones,
        &mut logs,
    )
    .await?;

    ai_steps.push(json!({
        "step": 3,
        "title": "Amp selection",
        "details": amp_reasons,
    }));

    let mut downloaded_count = 0usize;
    let mut model_items: Vec<Value> = Vec::new();
    let mut rig_presets: Vec<Value> = Vec::new();
    let mut used_cab_ids: HashSet<i64> = HashSet::new();

    for (index, amp_tone) in selected_amps.iter().enumerate() {
        let preset_label = format!("Preset {}", index + 1);
        let amp_title = value_as_string(amp_tone.get("title"));
        let (needs_cab, cab_decision_reason) = assess_amp_needs_cab(
            &client,
            &gemini_api_key,
            &gemini_model,
            &request,
            amp_tone,
            &mut logs,
        )
        .await?;

        let mut selected_cab: Option<Value> = None;
        let mut cab_selection_reason = if needs_cab {
            "Cab search pending".to_string()
        } else {
            "Amp profile judged complete without extra cab.".to_string()
        };

        if needs_cab {
            let cab_primary_queries = dedupe_non_empty_queries(
                {
                    let mut queries = vec![
                        format!("{} cab ir", request),
                        format!("{} ir", amp_title),
                        format!("{} cab", amp_title),
                    ];
                    queries.extend(analysis.search_queries.clone());
                    queries
                },
                8,
            );
            let cab_fallback_queries = dedupe_non_empty_queries(
                {
                    let mut queries = analysis.fallback_queries.clone();
                    queries.push(format!("{} guitar cabinet", request));
                    queries.push("guitar cab ir".to_string());
                    queries
                },
                8,
            );

            let mut cab_pool = build_gear_pool(
                &session,
                &cab_primary_queries,
                &cab_fallback_queries,
                "ir",
                max_results,
                &mut logs,
            )
            .await?;

            let filtered_cab_pool = cab_pool
                .iter()
                .filter(|tone| {
                    tone_id(tone)
                        .map(|id| !used_cab_ids.contains(&id))
                        .unwrap_or(true)
                })
                .cloned()
                .collect::<Vec<Value>>();
            if !filtered_cab_pool.is_empty() {
                cab_pool = filtered_cab_pool;
            }

            if let Some((cab_tone, reason)) = select_best_cab_for_amp(
                &client,
                &gemini_api_key,
                &gemini_model,
                &request,
                amp_tone,
                &cab_pool,
            )
            .await?
            {
                if let Some(cab_id) = tone_id(&cab_tone) {
                    used_cab_ids.insert(cab_id);
                }
                cab_selection_reason = reason;
                selected_cab = Some(cab_tone);
            } else {
                cab_selection_reason = "No cab candidate found for this amp.".to_string();
            }
        }

        ai_steps.push(json!({
            "step": ai_steps.len() + 1,
            "title": format!("{} rig decision", preset_label),
            "details": [
                format!("Amp: {}", amp_title),
                format!("Amp selection reason: {}", amp_reasons.get(index).cloned().unwrap_or_else(|| "Selected by relevance and popularity.".to_string())),
                format!("Cab needed: {}", if needs_cab { "yes" } else { "no" }),
                format!("Cab decision reason: {}", cab_decision_reason),
                format!("Cab selection reason: {}", cab_selection_reason),
            ],
        }));

        let preset_dir = output_dir.join(format!("preset_{}", index + 1));
        std::fs::create_dir_all(&preset_dir).map_err(|e| {
            format!(
                "Failed to create preset directory {}: {e}",
                preset_dir.display()
            )
        })?;

        let cab_summary = selected_cab.as_ref().map(summarize_tone);
        let rig_info = json!({
            "preset": preset_label.clone(),
            "request": request.clone(),
            "amp": summarize_tone(amp_tone),
            "cab": cab_summary,
            "needs_cab": needs_cab,
            "amp_selection_reason": amp_reasons.get(index).cloned().unwrap_or_default(),
            "cab_decision_reason": cab_decision_reason,
            "cab_selection_reason": cab_selection_reason,
        });
        std::fs::write(
            preset_dir.join("rig.json"),
            serde_json::to_string_pretty(&rig_info)
                .map_err(|e| format!("Failed to serialize rig info: {e}"))?,
        )
        .map_err(|e| format!("Failed to write rig info file: {e}"))?;

        rig_presets.push(rig_info);

        download_models_for_tone_component(
            &session,
            &client,
            &gemini_api_key,
            &gemini_model,
            &request,
            amp_tone,
            "amp",
            &preset_label,
            &preset_dir,
            &mut ai_steps,
            &mut model_items,
            &mut downloaded_count,
            &mut logs,
        )
        .await?;

        if let Some(cab_tone) = selected_cab.as_ref() {
            download_models_for_tone_component(
                &session,
                &client,
                &gemini_api_key,
                &gemini_model,
                &request,
                cab_tone,
                "cab",
                &preset_label,
                &preset_dir,
                &mut ai_steps,
                &mut model_items,
                &mut downloaded_count,
                &mut logs,
            )
            .await?;
        }
    }

    ai_steps.push(json!({
        "step": ai_steps.len() + 1,
        "title": "Download summary",
        "details": [
            format!("Selected amp presets: {}", selected_amps.len()),
            format!("Final rig count: {}", rig_presets.len()),
            format!("Downloaded models: {}", downloaded_count),
            format!("Output directory: {}", output_dir.to_string_lossy()),
        ],
    }));

    Ok(json!({
        "ok": true,
        "request": request,
        "analysis": analysis.to_json(),
        "gemini_model": gemini_model,
        "pool_size": amp_pool.len(),
        "selected_tones": selected_amps.iter().map(summarize_tone).collect::<Vec<Value>>(),
        "rig_presets": rig_presets,
        "downloaded_count": downloaded_count,
        "model_items": model_items,
        "ai_steps": ai_steps,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_unix_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn qa_payload(request: &str, case_name: &str) -> RunRequest {
        RunRequest {
            request: request.to_string(),
            tone3000_api_key: None,
            gemini_api_key: None,
            gemini_model: Some("gemini-2.5-pro".to_string()),
            output_dir: Some(format!(
                "./smart_downloaded_tones/qa_runs/{}_{}",
                case_name,
                now_unix_secs()
            )),
            max_tones: Some(1),
            max_results: Some(10),
        }
    }

    fn assert_keys_file_ready() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .expect("Repository root not found for QA tests");
        let check_payload = RunRequest {
            request: "qa".to_string(),
            tone3000_api_key: None,
            gemini_api_key: None,
            gemini_model: None,
            output_dir: None,
            max_tones: None,
            max_results: None,
        };
        assert!(
            resolve_keys(&check_payload, repo_root).is_ok(),
            "Missing keys. Provide keys in UI/env or keys.txt for QA tests."
        );
    }

    fn load_gemini_key_for_ai_tests() -> String {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .expect("Repository root not found for AI QA tests");
        let keys = read_keys_file(&repo_root.join("keys.txt"));
        if let Some(key) = keys
            .get("GEMINI_API_KEY")
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
        {
            return key.to_string();
        }
        if let Ok(env_key) = env::var("GEMINI_API_KEY") {
            if !env_key.trim().is_empty() {
                return env_key;
            }
        }
        panic!("Missing GEMINI_API_KEY in keys.txt or environment for AI QA tests");
    }

    fn sample_tones_for_artist_tests() -> Vec<Value> {
        vec![
            json!({
                "id": 101,
                "title": "Metallica Enter Sandman Rhythm",
                "description": "Tight mid-gain rhythm amp for 90s metal riffs.",
                "gear": "amp",
                "platform": "nam",
                "downloads_count": 9800
            }),
            json!({
                "id": 102,
                "title": "John Mayer Dumble Clean",
                "description": "Smooth clean blues edge with light breakup.",
                "gear": "amp",
                "platform": "nam",
                "downloads_count": 8700
            }),
            json!({
                "id": 103,
                "title": "Nirvana Teen Spirit Grunge",
                "description": "Raw crunchy distortion with scooped mids.",
                "gear": "amp",
                "platform": "nam",
                "downloads_count": 9100
            }),
            json!({
                "id": 104,
                "title": "Generic Jazz Clean",
                "description": "Very clean warm jazz amp tone.",
                "gear": "amp",
                "platform": "nam",
                "downloads_count": 3200
            }),
        ]
    }

    #[test]
    fn cab_fallback_detects_included_cab() {
        let amp = json!({
            "title": "5150 Studio Merged",
            "description": "High gain amp with cab included and miked cab chain."
        });
        let (needs_cab, _) = fallback_amp_needs_cab(&amp);
        assert!(
            !needs_cab,
            "Merged/cab-included amp should not force extra cab"
        );
    }

    #[test]
    fn cab_fallback_detects_head_only_amp() {
        let amp = json!({
            "title": "JCM800 Head",
            "description": "Amp head only profile, no cab, preamp focused."
        });
        let (needs_cab, _) = fallback_amp_needs_cab(&amp);
        assert!(needs_cab, "Head-only amp should require cab");
    }

    async fn run_quality_case(request: &str, case_name: &str) {
        assert_keys_file_ready();

        let response = run_download_inner(qa_payload(request, case_name))
            .await
            .expect("QA run should complete without internal error");

        assert!(
            response.get("ok").and_then(Value::as_bool).unwrap_or(false),
            "Run should return ok=true"
        );
        assert_eq!(
            response
                .get("gemini_model")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            "gemini-2.5-pro",
            "Run should use gemini-2.5-pro"
        );
        assert!(
            response
                .get("pool_size")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0,
            "Tone pool should not be empty"
        );
        assert!(
            response
                .get("selected_tones")
                .and_then(Value::as_array)
                .map(|arr| !arr.is_empty())
                .unwrap_or(false),
            "At least one tone should be selected"
        );
        assert!(
            response
                .get("ai_steps")
                .and_then(Value::as_array)
                .map(|arr| arr.len() >= 4)
                .unwrap_or(false),
            "AI should return step-by-step explanations"
        );
    }

    #[tokio::test]
    #[ignore = "Network QA test using real keys from keys.txt"]
    async fn qa_beginner_metallica_rhythm() {
        run_quality_case(
            "I am a beginner guitarist and want a Metallica Enter Sandman style rhythm tone.",
            "metallica_beginner",
        )
        .await;
    }

    #[tokio::test]
    #[ignore = "Network QA test using real keys from keys.txt"]
    async fn qa_beginner_john_mayer_clean() {
        run_quality_case(
            "I just started guitar and want a John Mayer inspired clean blues tone that is easy to play.",
            "john_mayer_beginner",
        )
        .await;
    }

    #[tokio::test]
    #[ignore = "Network QA test using real keys from keys.txt"]
    async fn qa_beginner_nirvana_grunge() {
        run_quality_case(
            "I am new to guitar and want a Nirvana Smells Like Teen Spirit style crunchy tone.",
            "nirvana_beginner",
        )
        .await;
    }

    #[tokio::test]
    #[ignore = "Network AI QA test using Gemini key from keys.txt"]
    async fn qa_ai_reasoning_metallica_beginner() {
        let client = Client::builder()
            .build()
            .expect("HTTP client should initialize");
        let gemini_key = load_gemini_key_for_ai_tests();
        let mut logs = String::new();
        let request = "I am new to guitar and want Metallica Enter Sandman rhythm tone.";

        let analysis =
            analyze_tone_request(&client, &gemini_key, "gemini-2.5-pro", request, &mut logs)
                .await
                .expect("Analysis should complete");
        assert!(
            !analysis.search_queries.is_empty(),
            "Search queries should exist"
        );
        assert!(
            !analysis.explanation_steps.is_empty(),
            "AI explanation steps should exist"
        );

        let (selected, reasons) = select_best_tones(
            &client,
            &gemini_key,
            "gemini-2.5-pro",
            request,
            &sample_tones_for_artist_tests(),
            1,
            &mut logs,
        )
        .await
        .expect("Tone selection should complete");
        assert_eq!(selected.len(), 1, "Exactly one tone should be selected");
        assert_eq!(reasons.len(), 1, "One selection reason should be returned");
    }

    #[tokio::test]
    #[ignore = "Network AI QA test using Gemini key from keys.txt"]
    async fn qa_ai_reasoning_john_mayer_beginner() {
        let client = Client::builder()
            .build()
            .expect("HTTP client should initialize");
        let gemini_key = load_gemini_key_for_ai_tests();
        let mut logs = String::new();
        let request = "I just started guitar and want a John Mayer clean blues tone.";

        let (selected, reasons) = select_best_tones(
            &client,
            &gemini_key,
            "gemini-2.5-pro",
            request,
            &sample_tones_for_artist_tests(),
            1,
            &mut logs,
        )
        .await
        .expect("Tone selection should complete");
        assert_eq!(selected.len(), 1, "Exactly one tone should be selected");
        assert_eq!(reasons.len(), 1, "One selection reason should be returned");
    }

    #[tokio::test]
    #[ignore = "Network AI QA test using Gemini key from keys.txt"]
    async fn qa_ai_reasoning_nirvana_beginner() {
        let client = Client::builder()
            .build()
            .expect("HTTP client should initialize");
        let gemini_key = load_gemini_key_for_ai_tests();
        let request = "I am beginner and want Nirvana Smells Like Teen Spirit grunge tone.";

        let model_candidates = vec![
            json!({"name": "Clean Channel Standard", "size": "standard"}),
            json!({"name": "Crunch Channel Standard", "size": "standard"}),
            json!({"name": "High Gain Red Channel", "size": "standard"}),
        ];

        let (selected_models, model_reasons) = filter_models(
            &client,
            &gemini_key,
            "gemini-2.5-pro",
            request,
            "Nirvana Teen Spirit Grunge",
            "Raw crunchy distortion",
            "amp",
            &model_candidates,
        )
        .await
        .expect("Model filtering should complete");
        assert!(
            !selected_models.is_empty(),
            "At least one model should be selected"
        );
        assert_eq!(
            selected_models.len(),
            model_reasons.len(),
            "Each selected model should have a reason"
        );
    }

    #[tokio::test]
    #[ignore = "Network AI QA test using Gemini key from keys.txt"]
    async fn qa_ai_reasoning_dimebag_darrell() {
        let client = Client::builder()
            .build()
            .expect("HTTP client should initialize");
        let gemini_key = load_gemini_key_for_ai_tests();
        let mut logs = String::new();
        let request =
            "I am a beginner guitarist and want a Dimebag Darrell style aggressive metal rhythm tone.";

        let analysis =
            analyze_tone_request(&client, &gemini_key, "gemini-2.5-pro", request, &mut logs)
                .await
                .expect("Analysis should complete");
        assert!(
            !analysis.search_queries.is_empty(),
            "Search queries should exist"
        );

        let (selected, reasons) = select_best_tones(
            &client,
            &gemini_key,
            "gemini-2.5-pro",
            request,
            &sample_tones_for_artist_tests(),
            1,
            &mut logs,
        )
        .await
        .expect("Tone selection should complete");
        assert_eq!(selected.len(), 1, "Exactly one tone should be selected");
        assert_eq!(reasons.len(), 1, "One selection reason should be returned");
    }

    #[tokio::test]
    #[ignore = "Network AI QA test using Gemini key from keys.txt"]
    async fn qa_ai_reasoning_synyster_gates() {
        let client = Client::builder()
            .build()
            .expect("HTTP client should initialize");
        let gemini_key = load_gemini_key_for_ai_tests();
        let mut logs = String::new();
        let request =
            "I just started guitar and want a Synyster Gates lead tone from Avenged Sevenfold.";

        let analysis =
            analyze_tone_request(&client, &gemini_key, "gemini-2.5-pro", request, &mut logs)
                .await
                .expect("Analysis should complete");
        assert!(
            !analysis.explanation_steps.is_empty(),
            "AI explanation steps should exist"
        );

        let (selected, reasons) = select_best_tones(
            &client,
            &gemini_key,
            "gemini-2.5-pro",
            request,
            &sample_tones_for_artist_tests(),
            1,
            &mut logs,
        )
        .await
        .expect("Tone selection should complete");
        assert_eq!(selected.len(), 1, "Exactly one tone should be selected");
        assert_eq!(reasons.len(), 1, "One selection reason should be returned");
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![run_download])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
