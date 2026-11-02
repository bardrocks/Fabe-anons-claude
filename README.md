# FABE Anons (v4)

Mağaza ve fabrika anons sistemi. Eski Python sürümünün (fabe-anons-sistemi) baştan yazılmış hali.
Bu sürüm Claude ile yapıldı.

- **Masaüstü:** Tauri 2 (arayüz React + TypeScript, çekirdek Rust)
- **Tasarım:** "Konsol" teması. Renkler ve yazı tipleri yalnızca `src/theme.css` dosyasında.
- **Veriler:** `%APPDATA%\FABE Anons` (fabe.db, sesler/, hata.log)

## Bu sürümde olanlar

- Personel pedleri, arama (`/` ile odaklanır, Enter ile ilk sonucu çağırır)
- Kullanıcının ekleyebildiği kategoriler (Personeller, Firmalar, Depo...)
- Tek anons sırası: aynı anda basılan anonslar birbirini kesmez; isimler 2 kez okunur
- Anonstan önce gong; anons sırasında diğer programların sesi %10'a kısılır
- DURDUR (Esc) her şeyi keser
- Zamanlı mola anonsları
- En çok çağrılanlar ve son anonslar
- Eski uygulamadan aktarım: ilk açılışta `%APPDATA%\Fabe_Anons_Veri` klasöründen kendiliğinden,
  ya da Ayarlar > "Eski yedekten geri yükle" ile eski uygulamanın yedek ZIP'inden.
  Eski klasör değiştirilmez.

## Sıradakiler

Canlı anons (ema-lightning ana motor, Piper yedek), tüm ses biçimleri için FFmpeg dönüştürme,
çoklu bilgisayar (sunucu/istemci), kişi ekleme/düzenleme, mola düzenleme, yedekleme, güncelleme.

## Geliştirme

```bash
npm install
npm run tauri dev      # masaüstü uygulaması
npm run dev            # yalnızca arayüz, tarayıcıda örnek verilerle
cd src-tauri && cargo test
```

Windows kurulum dosyası her gönderimde GitHub Actions ile derlenir (Actions > Windows derlemesi > FABE-Anons-kurulum).
