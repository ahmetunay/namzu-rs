# Namzu Rust yeniden kurulum

Önceki bulut çalışma alanındaki kaynak kod bu çalışma alanında bulunamadı.
Bu depo, eski sürümün kurtarılmış kopyası değildir; yeniden uygulamadır.

İlk hedef: Rust SDK, sınırlı ajan döngüsü, dosya araçları, terminal ve
model kotası kullanmayan deterministik doğrulama. Ayrıntılı durum ROADMAP.md'de.

Her çalışan aşama Git commit'i ile kaydedilir ve kullanıcının talimatıyla
[ahmetunay/namzu-rs](https://github.com/ahmetunay/namzu-rs) deposuna gönderilir.
Bu depo Public'tir; kullanıcı herkese açık yüklemeye izin verdi. İlk üç commit
GitHub'dan temiz clone ile geri indirilip doğrulandı. Yedek akışı BACKUP.md'de,
sekiz ana modül ve uygulanacak küçük adımlar ROADMAP.md'de tutulur.

Upstream kaynak: https://github.com/cogitave/namzu

Lisans ve sürüm kaydı: LICENSE.md ve NOTICE.md.

## Çalıştırma

Rust stable, sistem C derleyicisi ve OpenSSL geliştirme dosyaları gerekir.

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo build --release --workspace --locked
./target/release/namzu-rs demo --json
```

Demo geçici klasörde `answer.txt` dosyasına `12\n` yazar ve geri okur.
Gerçek dosya içeriği ve araç sonucu oraklı kontrol edilir. Yanıt, scripted
sağlayıcı tarafından verilir; demo bir modelin akıl yürütme testi değildir.

Gerçek model için API anahtarını kendin seçtiğin ortam değişkenine koy:

```sh
./target/release/namzu-rs exec 'Projeyi incele' \
  --cwd ./proje --model MODEL_ADI \
  --base-url https://api.openai.com/v1 \
  --api-key-env OPENAI_API_KEY --timeout 300 --json
```

`--allow-write` verilirse çalışma klasöründeki dosyalara yazabilir. Varsayılan
araçlar yalnızca `read_file` ve `list_files`; shell henüz yok. Anahtar komut
satırında alınmaz, kaydedilmez ve hata çıktısına eklenmez. Bu adaptör Chat
Completions API içindir; ChatGPT/Codex abonelik girişi henüz uygulanmadı.
Önceki oturumdan kimlik bilgileri taşınmadı. Yeni bir anahtar paylaşman gerekmez.

SDK giriş noktası `namzu_core::run_turn`; sağlayıcılar `Provider`, araç kümeleri
`Tools` trait'ini uygular. `ScriptedProvider` dış hizmet olmadan aynı döngüyü
sınar. CLI bu SDK'yi kullanır; çekirdek CLI'ye bağımlı değildir.

## Sınırlar

- Varsayılan turn bütçesi: 300 saniye, 64 sağlayıcı adımı, 128 araç çağrısı,
  4 MiB konuşma metni. CLI süreyi 1–86400 saniyeyle sınırlar.
- Dosya okuma/yazma 64 KiB, listeleme 512 girdiye sınırlıdır. Mutlak ve `..`
  yollar reddedilir. `cap-std` dosya erişimini kök dizin yeteneğiyle sınırlar;
  bu bir süreç/Docker sandbox'ı değildir.
- Ctrl-C iptali etki sınırlarında denetlenir. Devam eden bloklayan HTTP isteği
  hemen kesilmez; kalan turn süresine bağlı HTTP timeout'u ile sonlanır.
  Dosya sistemi çekirdek çağrılarının kesilmesi garanti edilmez.
- Araç çağrısı grubu bütçe/ID/argüman yapısı açısından ilk etkiden önce
  denetlenir. Tamamlanan dosya etkileri iptal veya sonraki hatada geri alınmaz.
- HTTP yönlendirme/otomatik tekrar/login yok. TLS doğrulaması ve ortam proxy/CA
  ayarları korunur. Sunucu hata gövdeleri loglanmaz.
- `--json` başarı/hata stdout sözleşmesi `format_version`, `status`,
  `output` veya `error`, `elapsed_ms`, `offline` alanlarını içerir. Başarıda
  `stats.provider_calls` ve `stats.tool_calls` vardır. Hata exit code'u 1;
  CLI sözdizimi hataları Clap tarafından stderr'e yazılır (exit 2).
- Kayıp eski benchmark dosyaları bu ortamda yok; onlarla uyum henüz doğrulanmadı.
  Yerel HTTP fixture testi gerçek CLI/sağlayıcı/araç zincirini sınar.
- TUI, session kalıcılığı, Pals ve MCP henüz yeniden uygulanmadı.

## Yerel checkpoint ve kurtarma

```sh
git add Cargo.toml Cargo.lock crates scripts README.md ROADMAP.md
git commit -m 'Save validated milestone'
python3 scripts/checkpoint.py
```

Script yalnızca commit edilmiş dosyalardan kaynak arşivi ve tüm Git geçmişi
için bundle oluşturur; geçici dizine clone ederek kaynak ağacını doğrular.
Sonuç ve SHA-256 değerleri `artifacts/checkpoints/COMMIT/manifest.json` içindedir.
Yeni izlenmeyen dosyaları checkpoint'e eklemek için önce Git'e eklemek gerekir.

Bundle'dan geri yükleme:

```sh
git clone /yedek/namzu-rs.bundle namzu-rs
cd namzu-rs
cargo test --workspace --locked
```

**Bu dosyaların hepsi aynı bulut diskinde kalırsa dış yedek değildir.** Uzak
depo hedefi ve gerçek yükleme/geri okuma doğrulanana kadar dış yedek kurulmuş
sayılmaz. GitHub hesabı bağlantısı tek başına dosyaları yedeklemez.

Bu projede dış yedek artık yukarıdaki GitHub deposudur. Yerel checkpoint script'i
GitHub'a yüklemez; manifestindeki `remote_backup_verified: false` yalnızca o
script'in uzak doğrulama yapmadığını ifade eder. Her aşamadan sonra ayrıca
BACKUP.md'deki push ve uzak commit doğrulaması yapılır.
