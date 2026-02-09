# TONE3000 Smart Tone Downloader (Rust + Tauri)

Bu proje Rust + Tauri ile calisir.

- Backend akis: `src-tauri/src/main.rs`
- Desktop arayuz: `ui/`
- Varsayilan AI modeli: `gemini-2.5-pro`
- UI uzerinden esnek model secimi: `Gemini modeli` alani
- Rig odakli akis: her preset icin amp + (gerekiyorsa) cab/IR secimi

## Kurulum

```bash
npm install --include=dev
```

## API key saglama yontemleri

1. UI alanlarindan girerek
2. Ortam degiskenleri:
   - `TONE3000_API_KEY`
   - `GEMINI_API_KEY`
3. `keys.txt` dosyasindan (repo koku):
   - `TONE3000_API_KEY=...`
   - `GEMINI_API_KEY=...`

`keys.txt` varsa ve UI alanlari bos birakilirsa otomatik kullanilir.

## Calistirma

```bash
npm run tauri:dev
```

## Build / Check

```bash
npm run tauri:check
npm run tauri:build
```

## AI adim aciklamalari

Guncel surumde AI karar akisi adim adim doner:

1. Istek analizi
2. Amp arama ve preset bazli secim
3. Her amp icin cab/IR gereksinimi karari
4. Gerekiyorsa amp + cab eslestirmesi
5. Bilesen bazli model filtreleme ve indirme ozeti

Bu adimlar UI'da `AI Adimlari` panelinde gorunur.

## Ciktilar

Varsayilan indirme dizini: `./smart_downloaded_tones/`

- `nam` platformundaki model dosyalari `.nam` uzantisiyla kaydedilir.
- Her secilen tone klasoru icinde `info.json` olusur.
