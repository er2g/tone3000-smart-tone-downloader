# TONE3000 Smart Tone Downloader (Rust + Tauri)

Bu proje artık tamamen Rust tabanlıdır.

- Backend akışı: Rust (`src-tauri/src/main.rs`)
- Desktop arayüz: Tauri + vanilla UI (`ui/`)
- AI modeli: `gemini-2.5-flash` (JSON yanıt)

## Kurulum

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

## Çalıştırma

```bash
npm run tauri:dev
```

## Build / Check

```bash
npm run tauri:check
npm run tauri:build
```

Linux release build (`.deb`) çıktısı:

- `src-tauri/target/release/bundle/deb/Tone3000 Smart Tone Downloader_<version>_amd64.deb`

## Akış

Genel işleyiş korunmuştur:

1. Gemini ile tone isteği analizi (`search_queries`, `fallback_queries`, `gear_type`)
2. Tone3000 araması ve aday havuz oluşturma
3. Gemini ile en iyi tone seçimi
4. Her tone için model listesi çekme
5. Gemini ile model filtreleme
6. Model dosyalarını indirme + sonuçları UI’da gösterme

## Çıktılar

Varsayılan indirme dizini `./smart_downloaded_tones/` klasörüdür (gitignore).

- `nam` platformundaki model dosyaları `.nam` uzantısıyla kaydedilir.
- Her seçilen tone klasörü içinde `info.json` oluşturulur.
