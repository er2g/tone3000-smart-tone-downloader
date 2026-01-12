# TONE3000 Smart Tone Downloader (Gemini)

Python CLI that searches Tone3000 and uses Gemini to:
- analyze a free-text tone request,
- pick the best matching tones/models,
- download selected model files into a local folder.

## Setup

Install deps:

```bash
python -m pip install -r requirements.txt
```

Set API keys (recommended):

- `TONE3000_API_KEY`
- `GEMINI_API_KEY`

## Run

```bash
python allah.py
```

Downloads go to `./smart_downloaded_tones/` by default (ignored by git).

