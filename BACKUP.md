# GitHub yedeği ve devam etme

## Kalıcı kullanıcı talimatı

2026-10-17: Kullanıcı “Yükle her seferinde yükle” dedi. Proje için hedef:

- Depo: https://github.com/ahmetunay/namzu-rs
- Dal: `main`
- Görünürlük: Public. Kullanıcı bu depoya herkese açık yüklemeye izin verdi.
- Her tamamlanan ve doğrulanan aşamayı commit edip bu depoya yükle.
- Yeni oturumlarda bu talimatı koru; aynı hedefe her yüklemede tekrar izin isteme.
- Anahtarlar, oturum kimlik bilgileri, .env, target, artifacts ve özel sohbetler
  yüklenmez. Mevcut kodun kaynak dosyaları ve proje belgeleri yedeklenir.

## İlk dış yedek kanıtı

- Commit: `f949239ce8483e6a95f52c4a5d5c944985ed1409`
- Kaynak ağacı: `b76453042f9d34148703167c1e9ebe145562c03e`
- 22 takip edilen kaynak/belge dosyası, 3 commit'lik geçmiş.
- GitHub'a gerçek push yapıldı; bağımsız geçici klasöre tekrar clone edildi.
- Geri indirilen HEAD, kaynak ağacı ve temiz çalışma alanı kontrol edildi;
  `git fsck --full` geçti. Bu ilk kanıtın ardından plan belgeleri de yayımlanır.

## Her aşamanın bitişi

1. Değişikliğe uygun kontrolleri çalıştır. Rust değişikliklerinde AGENTS.md'deki
   format/test/Clippy kontrollerini uygula; ilgisiz kontrolleri gereksiz tekrarlama.
2. Kaydedilecek dosyaları gözden geçir, açık dosya yollarıyla Git'e ekle ve commit et.
3. `python3 scripts/checkpoint.py` ile yerel kaynak/bundle kurtarmasını doğrula.
4. `git push origin main` çalıştır. Uzakta başka çalışma varsa önce inceleyip
   birleştir; force-push yapma.
5. `git ls-remote origin refs/heads/main` sonucundaki SHA'yı `git rev-parse HEAD`
   ile karşılaştır. Eşleşme yoksa yükleme tamamlandı deme.
6. İlk kurulumda ve kurtarma doğrulamasında temiz clone al, HEAD/ağaç eşitliğini
   denetle. Yerel checkpoint manifesti uzak doğrulamanın yerine geçmez.
7. Son yanıtta yüklenen aşamayı ve varsa gönderilemeyen commit'i açıkça belirt.

Yerel checkpoint script'i yalnızca yerel arşiv/bundle üretir. Manifestindeki
`remote_backup_verified: false`, script'in uzak doğrulama yapmadığını ifade eder;
uzak yedek durumu yukarıdaki ayrı denetimle belirlenir.

## Yeni bulut oturumunda kurtarma

```sh
git clone https://github.com/ahmetunay/namzu-rs.git
cd namzu-rs
git fsck --full
git log -5 --oneline
```

Önce AGENTS.md, ROADMAP.md ve bu dosyayı oku. Rust kuruluysa:

```sh
cargo test --workspace --locked
```

Model bağlantıları ve anahtarlar kaynak deposundan geri yüklenmez. Deterministik
testler bunlara ihtiyaç duymaz. Kaybolan eski uygulama ile bu yeniden kurulumun
durumunu birbirine karıştırma.
