# Tone Sherpa

AI-assisted Tone3000 downloader that interprets a free-text tone request, discovers matching profiles, and downloads selected model files.

## Features

- Query parsing with Gemini (`gemini-2.5-flash`)
- Tone3000 API search and filtering
- Interactive candidate selection flow
- Automatic file download into local output folder

## Requirements

- Python 3.9+
- Tone3000 API key
- Gemini API key

## Install

```bash
python -m pip install -r requirements.txt
```

## Environment

- `TONE3000_API_KEY`
- `GEMINI_API_KEY`

## Run

```bash
python allah.py
```

Default download location:

```text
./smart_downloaded_tones/
```

## Project Layout

- `allah.py`: CLI entrypoint
- `scripts/`: helper utilities
