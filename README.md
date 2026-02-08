# TONE3000 Smart Tone Downloader (Gemini + Tauri UI)

Bu repo artık iki kullanım modu içeriyor:

- `allah.py`: klasik Python CLI
- `Tauri desktop UI`: preset seçimi + API key girişi + akıllı indirme akışı

Gemini modeli: `gemini-2.5-flash` (`application/json` yanıt formatı).

## Kurulum

Python bağımlılıkları:

```bash
python -m pip install -r requirements.txt
```

Tauri CLI bağımlılığı:

```bash
npm install --include=dev
```

API key sağlama yöntemleri:

1. Arayüzden alanlara girerek (önerilen)
2. Ortam değişkenleri:
   - `TONE3000_API_KEY`
   - `GEMINI_API_KEY`
3. İsteğe bağlı `keys.txt` içinde:
   - `TONE3000_API_KEY=...`
   - `GEMINI_API_KEY=...`

## Python CLI Çalıştırma

```bash
python allah.py
```

## Tauri Arayüzü Çalıştırma

```bash
npm run tauri:dev
```

Arayüz özellikleri:

- 4 hazır preset:
  - Tight Metal Wall
  - Glass Jazz Clean
  - Vintage Crunch 70s
  - Arena Lead Hero
- Preset seçimi sonrası prompt + ayarların otomatik dolması
- `max tones` ve `candidate limit` ayarı
- Seçilen tonlar / indirilen modeller / log çıktısı paneli

## Tauri Build / Check

```bash
npm run tauri:check
npm run tauri:build
```

Linux release build (`.deb`) çıktısı:

- `src-tauri/target/release/bundle/deb/Tone3000 Smart Tone Downloader_<version>_amd64.deb`

## Çıktılar

Varsayılan indirme dizini `./smart_downloaded_tones/` klasörüdür (gitignore).

- `nam` platformundaki model dosyaları `.nam` uzantısıyla kaydedilir.
- Her seçilen tone klasörü içinde `info.json` oluşturulur.
