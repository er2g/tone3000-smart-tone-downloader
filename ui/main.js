const el = {
  tone3000Key: document.getElementById("tone3000Key"),
  geminiKey: document.getElementById("geminiKey"),
  geminiModel: document.getElementById("geminiModel"),
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
  aiStepList: document.getElementById("aiStepList"),
  selectedToneList: document.getElementById("selectedToneList"),
  modelList: document.getElementById("modelList"),
  logOutput: document.getElementById("logOutput"),
};

let isRunning = false;

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function setRunState(kind, text) {
  el.runState.className = `run-state ${kind}`;
  el.runState.textContent = kind === "running" ? "Running" : kind.charAt(0).toUpperCase() + kind.slice(1);
  el.statusText.textContent = text;
}

function renderAnalysis(analysis, poolSize, modelName) {
  if (!analysis) {
    el.analysisSummary.textContent = "Henuz analiz yapilmadi.";
    el.analysisMeta.innerHTML = "";
    return;
  }
  el.analysisSummary.textContent = analysis.description || "Analiz tamamlandi.";
  const chips = [];
  if (modelName) chips.push(`Model: ${modelName}`);
  if (analysis.gear_type) chips.push(`Gear: ${analysis.gear_type}`);
  if (poolSize !== undefined) chips.push(`Pool: ${poolSize}`);
  for (const query of analysis.search_queries || []) chips.push(`Q: ${query}`);
  for (const query of analysis.fallback_queries || []) chips.push(`Fallback: ${query}`);
  el.analysisMeta.innerHTML = chips.map((chip) => `<span class="meta-chip">${escapeHtml(chip)}</span>`).join("");
}

function renderAiSteps(steps) {
  if (!steps || steps.length === 0) {
    el.aiStepList.className = "ai-steps empty";
    el.aiStepList.textContent = "AI adimlari bulunamadi.";
    return;
  }

  el.aiStepList.className = "ai-steps";
  el.aiStepList.innerHTML = steps
    .map((step) => {
      const details = (step.details || []).map((line) => `<li>${escapeHtml(line)}</li>`).join("");
      return `
      <article class="ai-step-item">
        <div class="name">Adim ${escapeHtml(step.step ?? "?")} - ${escapeHtml(step.title || "AI step")}</div>
        <ul class="meta">${details || "<li>Detay yok.</li>"}</ul>
      </article>
    `;
    })
    .join("");
}

function renderTones(rigs, fallbackTones) {
  if (rigs && rigs.length > 0) {
    el.selectedToneList.className = "tone-list";
    el.selectedToneList.innerHTML = rigs
      .map(
        (rig) => `
      <article class="tone-item">
        <div class="name">${escapeHtml(rig.preset || "Preset")} - ${escapeHtml(rig.amp?.title || "Amp")}</div>
        <div class="meta">
          Amp: ${escapeHtml(rig.amp?.gear || "amp")} - ${escapeHtml(rig.amp?.platform || "unknown")}
        </div>
        <div class="meta">
          Cab: ${rig.cab ? escapeHtml(rig.cab.title || "Cab/IR") : "Gerekmiyor"}
        </div>
      </article>
    `
      )
      .join("");
    return;
  }

  const tones = fallbackTones || [];
  if (tones.length === 0) {
    el.selectedToneList.className = "tone-list empty";
    el.selectedToneList.textContent = "Secilen rig yok.";
    return;
  }

  el.selectedToneList.className = "tone-list";
  el.selectedToneList.innerHTML = tones
    .map(
      (tone) => `
      <article class="tone-item">
        <div class="name">${escapeHtml(tone.title || "Untitled Tone")}</div>
        <div class="meta">
          ${escapeHtml(tone.gear || "unknown")} - ${escapeHtml(tone.platform || "unknown")} - ${
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
    el.modelList.textContent = "Indirilen model bulunmuyor.";
    return;
  }

  el.modelList.className = "model-list";
  el.modelList.innerHTML = models
    .map(
      (item) => `
      <article class="model-item ${escapeHtml(item.status || "")}">
        <div class="name">${escapeHtml(item.model_name || "model")}</div>
        <div class="meta">
          ${escapeHtml(item.tone_title || "tone")} - ${escapeHtml(item.status || "unknown")} - ${item.size_mb ?? 0} MB
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
  if (!request) throw new Error("Tone istegi bos olamaz.");

  const maxTones = Number(el.maxTones.value || 3);
  const maxResults = Number(el.maxResults.value || 15);
  if (Number.isNaN(maxTones) || maxTones < 1 || maxTones > 5) {
    throw new Error("Maks ton sayisi 1-5 araliginda olmali.");
  }
  if (Number.isNaN(maxResults) || maxResults < 5 || maxResults > 25) {
    throw new Error("Aday ton limiti 5-25 araliginda olmali.");
  }

  const geminiModel = (el.geminiModel.value || "gemini-2.5-pro").trim();
  if (!geminiModel) {
    throw new Error("Gemini modeli bos birakilamaz.");
  }

  return {
    request,
    outputDir: (el.outputDir.value || "./smart_downloaded_tones").trim(),
    maxTones,
    maxResults,
    geminiModel,
    tone3000ApiKey: el.tone3000Key.value.trim() || null,
    geminiApiKey: el.geminiKey.value.trim() || null,
  };
}

async function onRun() {
  if (isRunning) return;

  const invoke = getInvoke();
  if (!invoke) {
    setRunState("error", "Tauri runtime bulunamadi. Bu ekrani Tauri uygulamasi uzerinden ac.");
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
  setRunState("running", "AI analiz ve indirme akisi calisiyor...");

  try {
    const response = await invoke("run_download", { payload });
    if (!response?.ok) {
      const msg = response?.error || "Islem basarisiz oldu.";
      setRunState("error", msg);
      renderAnalysis(null);
      renderAiSteps([]);
      renderTones([], []);
      renderModels([]);
      el.logOutput.textContent = response?.logs || msg;
      return;
    }

    renderAnalysis(response.analysis, response.pool_size, response.gemini_model);
    renderAiSteps(response.ai_steps);
    renderTones(response.rig_presets, response.selected_tones);
    renderModels(response.model_items);
    el.logOutput.textContent = response.logs || "Log alinamadi.";
    setRunState("done", `Tamamlandi. ${response.downloaded_count} model indirildi. Cikti: ${response.output_dir}`);
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
  renderAnalysis(null);
  renderAiSteps([]);
  renderTones([], []);
  renderModels([]);
  el.runButton.addEventListener("click", onRun);
  el.clearLogsButton.addEventListener("click", onClearLogs);
}

init();
