const presets = [
  {
    id: "tight-metal",
    title: "Tight Metal Wall",
    vibe: "Palm mute net, baskin ritim metal",
    request: "Modern tight high gain rhythm tone with aggressive mids for palm-muted riffs",
    maxTones: 3,
    maxResults: 16,
  },
  {
    id: "jazz-clean",
    title: "Glass Jazz Clean",
    vibe: "Parlak ama kontrollü clean ton",
    request: "Hi-fi clean guitar tone with airy top-end, soft compression, and lush sustain for jazz fusion",
    maxTones: 2,
    maxResults: 12,
  },
  {
    id: "vintage-crunch",
    title: "Vintage Crunch 70s",
    vibe: "Klasik rock crunch + orta gain",
    request: "70s British crunchy amp tone with warm mids and dynamic response",
    maxTones: 3,
    maxResults: 15,
  },
  {
    id: "lead-shred",
    title: "Arena Lead Hero",
    vibe: "Lead odakli, singing sustain",
    request: "Singing high-gain lead tone with smooth top end and long sustain for melodic solos",
    maxTones: 4,
    maxResults: 20,
  },
];

const el = {
  presetGrid: document.getElementById("presetGrid"),
  selectedPresetLabel: document.getElementById("selectedPresetLabel"),
  tone3000Key: document.getElementById("tone3000Key"),
  geminiKey: document.getElementById("geminiKey"),
  toneRequest: document.getElementById("toneRequest"),
  outputDir: document.getElementById("outputDir"),
  maxTones: document.getElementById("maxTones"),
  maxResults: document.getElementById("maxResults"),
  runButton: document.getElementById("runButton"),
  clearLogsButton: document.getElementById("clearLogsButton"),
  statusText: document.getElementById("statusText"),
  runState: document.getElementById("runState"),
  analysisSummary: document.getElementById("analysisSummary"),
  analysisMeta: document.getElementById("analysisMeta"),
  selectedToneList: document.getElementById("selectedToneList"),
  modelList: document.getElementById("modelList"),
  logOutput: document.getElementById("logOutput"),
};

let selectedPresetId = null;
let isRunning = false;

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function getPresetById(id) {
  return presets.find((preset) => preset.id === id);
}

function setRunState(kind, text) {
  el.runState.className = `run-state ${kind}`;
  el.runState.textContent = kind === "running" ? "Running" : kind.charAt(0).toUpperCase() + kind.slice(1);
  el.statusText.textContent = text;
}

function renderPresetCards() {
  el.presetGrid.innerHTML = presets
    .map(
      (preset) => `
      <button type="button" class="preset-card ${preset.id === selectedPresetId ? "active" : ""}" data-id="${preset.id}">
        <div class="title">${escapeHtml(preset.title)}</div>
        <div class="vibe">${escapeHtml(preset.vibe)}</div>
      </button>
    `
    )
    .join("");

  for (const node of el.presetGrid.querySelectorAll(".preset-card")) {
    node.addEventListener("click", () => {
      const id = node.dataset.id;
      const preset = getPresetById(id);
      if (!preset) return;
      selectedPresetId = id;
      el.selectedPresetLabel.textContent = `Preset: ${preset.title}`;
      el.toneRequest.value = preset.request;
      el.maxTones.value = String(preset.maxTones);
      el.maxResults.value = String(preset.maxResults);
      renderPresetCards();
    });
  }
}

function renderAnalysis(analysis, poolSize) {
  if (!analysis) {
    el.analysisSummary.textContent = "Henüz analiz yapılmadı.";
    el.analysisMeta.innerHTML = "";
    return;
  }
  el.analysisSummary.textContent = analysis.description || "Analiz tamamlandı.";
  const chips = [];
  if (analysis.gear_type) chips.push(`Gear: ${analysis.gear_type}`);
  if (poolSize !== undefined) chips.push(`Pool: ${poolSize}`);
  for (const query of analysis.search_queries || []) chips.push(`Q: ${query}`);
  for (const query of analysis.fallback_queries || []) chips.push(`Fallback: ${query}`);
  el.analysisMeta.innerHTML = chips.map((chip) => `<span class="meta-chip">${escapeHtml(chip)}</span>`).join("");
}

function renderTones(tones) {
  if (!tones || tones.length === 0) {
    el.selectedToneList.className = "tone-list empty";
    el.selectedToneList.textContent = "Seçilen ton yok.";
    return;
  }

  el.selectedToneList.className = "tone-list";
  el.selectedToneList.innerHTML = tones
    .map(
      (tone) => `
      <article class="tone-item">
        <div class="name">${escapeHtml(tone.title || "Untitled Tone")}</div>
        <div class="meta">
          ${escapeHtml(tone.gear || "unknown")} • ${escapeHtml(tone.platform || "unknown")} • ${
            tone.downloads_count ?? 0
          } indirime sahip
        </div>
      </article>
    `
    )
    .join("");
}

function renderModels(models) {
  if (!models || models.length === 0) {
    el.modelList.className = "model-list empty";
    el.modelList.textContent = "İndirilen model bulunmuyor.";
    return;
  }

  el.modelList.className = "model-list";
  el.modelList.innerHTML = models
    .map(
      (item) => `
      <article class="model-item ${escapeHtml(item.status || "")}">
        <div class="name">${escapeHtml(item.model_name || "model")}</div>
        <div class="meta">
          ${escapeHtml(item.tone_title || "tone")} • ${escapeHtml(item.status || "unknown")} • ${item.size_mb ?? 0} MB
        </div>
      </article>
    `
    )
    .join("");
}

function setRunningState(running) {
  isRunning = running;
  el.runButton.disabled = running;
}

function getInvoke() {
  return window.__TAURI__?.core?.invoke;
}

function collectPayload() {
  const request = el.toneRequest.value.trim();
  if (!request) throw new Error("Tone isteği boş olamaz.");

  const maxTones = Number(el.maxTones.value || 3);
  const maxResults = Number(el.maxResults.value || 15);
  if (Number.isNaN(maxTones) || maxTones < 1 || maxTones > 5) {
    throw new Error("Maks ton sayısı 1-5 aralığında olmalı.");
  }
  if (Number.isNaN(maxResults) || maxResults < 5 || maxResults > 25) {
    throw new Error("Aday ton limiti 5-25 aralığında olmalı.");
  }

  return {
    request,
    outputDir: (el.outputDir.value || "./smart_downloaded_tones").trim(),
    maxTones,
    maxResults,
    tone3000ApiKey: el.tone3000Key.value.trim() || null,
    geminiApiKey: el.geminiKey.value.trim() || null,
  };
}

async function onRun() {
  if (isRunning) return;

  const invoke = getInvoke();
  if (!invoke) {
    setRunState("error", "Tauri runtime bulunamadı. Bu ekranı Tauri uygulaması üzerinden aç.");
    return;
  }

  let payload;
  try {
    payload = collectPayload();
  } catch (err) {
    setRunState("error", err.message);
    return;
  }

  setRunningState(true);
  setRunState("running", "AI analiz ve indirme akışı çalışıyor...");

  try {
    const response = await invoke("run_download", { payload });
    if (!response?.ok) {
      const msg = response?.error || "İşlem başarısız oldu.";
      setRunState("error", msg);
      renderAnalysis(null);
      renderTones([]);
      renderModels([]);
      el.logOutput.textContent = response?.logs || msg;
      return;
    }

    renderAnalysis(response.analysis, response.pool_size);
    renderTones(response.selected_tones);
    renderModels(response.model_items);
    el.logOutput.textContent = response.logs || "Log alınamadı.";
    setRunState(
      "done",
      `Tamamlandı. ${response.downloaded_count} model indirildi. Çıktı: ${response.output_dir}`
    );
  } catch (err) {
    const msg = typeof err === "string" ? err : err?.message || "Bilinmeyen hata";
    setRunState("error", msg);
    el.logOutput.textContent = msg;
  } finally {
    setRunningState(false);
  }
}

function onClearLogs() {
  el.logOutput.textContent = "Log temizlendi.";
}

function init() {
  renderPresetCards();
  renderAnalysis(null);
  renderTones([]);
  renderModels([]);
  el.runButton.addEventListener("click", onRun);
  el.clearLogsButton.addEventListener("click", onClearLogs);
}

init();
