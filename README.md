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
- Kişiler ekranı: anons butonu ekle / düzenle / sil, buton rengi, ses dosyası seç ve dinle
- Kategoriler: ekle, adını değiştir, sil, sırala
- Mola ekranı: saat ekle / düzenle / sil, her mola için ses dosyası; saatinde kendiliğinden çalar
- Ayarlar: tüm verileri yedekle, yedekten geri yükle (yeni ve eski yedekler), otomatik yedekler,
  ağ ayarları (Yerel / Sunucu / İstemci, IP, port, bağlantı testi), güncelleme
- En çok çağrılanlar ve son anonslar
- Eski uygulamadan aktarım: ilk açılışta `%APPDATA%\Fabe_Anons_Veri` klasöründen kendiliğinden,
  ya da Ayarlar > "Eski yedekten geri yükle" ile eski uygulamanın yedek ZIP'inden.
  Eski klasör değiştirilmez.

## Sıradakiler

Canlı anons (ema-lightning ana motor, Piper yedek), tüm ses biçimleri için FFmpeg dönüştürme
(WhatsApp .opus dahil), sunucu/istemci arasında anons gönderme.

## Geliştirme

```bash
npm install
npm run tauri dev      # masaüstü uygulaması
npm run dev            # yalnızca arayüz, tarayıcıda örnek verilerle
cd src-tauri && cargo test
```

Windows kurulum dosyası her gönderimde GitHub Actions ile derlenir (Actions > Windows derlemesi > FABE-Anons-kurulum).

## Yeni sürüm yayınlama (otomatik güncelleme)

1. `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` ve `package.json` içindeki sürümü artırın (ör. 4.1.0).
2. `git tag -a v4.1.0 -m "Bu sürümde neler var"` ve `git push origin v4.1.0`.
3. GitHub Actions kurulum dosyasını derler, bir GitHub sürümü açar ve `latest.json` yayınlar.
   Uygulamalar açılışta bunu görür ve "Yeni sürüm var" butonu çıkar. Butona basılınca önce tüm veriler
   `%APPDATA%\FABE Anons\yedekler` klasörüne yedeklenir, sonra yeni sürüm kurulur ve uygulama yeniden açılır.

Güncellemelerin indirilebilmesi için sürümlerin herkese açık olması gerekir.
