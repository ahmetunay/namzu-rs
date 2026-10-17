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
araçlar `read_file`, `list_files`, `search_files` ve `diff_file`.
`--allow-write` ayrıca `write_file` ve tek eşleşmeli `apply_patch` açar.
`--allow-shell` host `/bin/sh` çalıştırır: bu izin çalışma klasörü dışındaki
dosyalara ve ağa da erişim sağlar, OS sandbox'ı değildir. `--tool-allow` araç
listesini daraltır; `--tool-deny` her zaman önceliklidir. Shell'in env'i
temizlenir; yalnızca PATH/locale ve proxy/CA değişkenleri aktarılır. Anahtar komut
satırında alınmaz, kaydedilmez ve hata çıktısına eklenmez. Bu adaptör Chat
Completions API içindir; Codex aboneliği için aşağıdaki ayrı native sürücüyü kullan.
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
- TUI, Pals ve MCP henüz yeniden uygulanmadı.

## 0.3 — özel durum ve oturum devamı

Varsayılan kayıt yeri `$NAMZU_STATE_DIR`, yoksa `$XDG_STATE_HOME/namzu-rs`,
yoksa `~/.local/state/namzu-rs`. `--state-dir` açık seçicidir. Unix'te dizin
0700, dosyalar 0600 olmalı; yanlış sahiplik, symlink/hardlink ve paylaşılmış
dosyalar reddedilir. Atomik rename + dosya/dizin sync ve süreçler arası lock var.

```sh
namzu-rs config init
namzu-rs config show
namzu-rs exec 'İlk görev' --model MODEL --json
namzu-rs session list
namzu-rs session show OTURUM_ID
namzu-rs exec 'Devam et' --model MODEL --resume OTURUM_ID --json
```

CLI seçenekleri seçilen config'i ezer. `--config` açıkça güvenilen JSON dosyasını
seçer; proje config'i otomatik okunmaz. Örnek: config.example.json. Config yalnızca
ortam değişkeni adını saklar, API anahtarını saklamaz. Config araç listesini
daraltabilir; shell/yazma izni yalnızca çağrının `--allow-shell/--allow-write`
seçenekleriyle açılır. Resume eski yetkileri kendiliğinden açmaz.

Tamamlanmış session aynı canonical cwd ve sağlayıcı/model/endpoint ile devam
eder. Her araçtan ÖNCE pending kayıt, sonuçtan SONRA checkpoint yazılır. Disk
hatası etkiyi engeller. Çökme/iptal/hata sonrası session running/failed kalır ve
otomatik resume reddedilir: `session show` ile etkileri inceleyip yeni session
açılmalıdır. Bu ilk sürüm yarım kalan turn için otomatik kurtarma/rollback vaat
etmez. Özel session/config dosyaları GitHub yedeğine dâhil değildir.

## 0.4 — native Codex abonelik sürücüsü

```sh
namzu-rs auth login
namzu-rs auth status
namzu-rs models
namzu-rs exec 'Görev' --provider codex --model MODEL_LISTESINDEKI_ID \
  --effort low --allow-write --allow-shell --timeout 300 --json
namzu-rs auth logout
```

Login yalnızca açık `auth login` komutuyla başlar; cihaz kodunu ekranda verilen
resmî sayfaya girersin. Normal exec/models otomatik tarayıcı/login açmaz. Native
Rust HTTP sürücüsü kullanılır; Codex CLI/Node alt süreci veya API-key fallback yok.
Codex endpoint'leri sabittir; `--base-url` abonelik tokenını başka yere gönderemez.

Tokenlar özel state dizinindeki codex-credentials.json'da 0600 saklanır;
GitHub'a yüklenmez. Login/refresh/logout aynı süreçler arası kilidi kullanır.
Son 60 saniyeye giren token yenilenir; dönen hesap kimliği değişirse reddedilir.
Refresh token döndürülürse atomik olarak kaydedilir. Yenileme sonrası kalıcı disk
hatası gibi kurtarılamayan durumlar açık hata verir; yeniden login gerekebilir.
JWT payload'ı yalnızca hesap/expiry metadata'sı içindir; imza doğrulaması iddiası
yoktur, gerçek token kabulü HTTPS servisinindir. Status tokenları göstermez.

Responses/SSE tamamlanmadan araçlar çalıştırılmaz. Kesik/incomplete/refused
yanıtlar başarısızdır. Yerel SSE sınırları: 1 MiB olay/satır, 16 MiB akış,
4096 output item. Opaque reasoning ve native tool item'ları özel session'a
kaydedilip yalnızca aynı hesap/model/endpoint ve değişmemiş mesajla yeniden
gönderilir. Başka sağlayıcının isteklerine eklenmez. TUI'ye canlı token akışı
henüz yok; bu sürüm SSE'yi tamamlanana kadar tüketir.

Model adları `models` yanıtından seçilir; “Luna” diye sabit/tahmini bir ID yoktur.
Yerel fixture testleri login/poll/exchange, refresh, logout, model listesi,
SSE/araç döngüsü ve reasoning replay'i doğrular. Bunlar canlı abonelik erişimi
veya kota garantisi değildir; mevcut hesap doğrulaması ayrı yapılır.

0.2: Shell en fazla 300 saniye (varsayılan 60 ve turn kalan süresiyle sınırlı),
64 KiB birleşik çıktı; süreç grubu iptal/hata/çıktı sınırı ve normal lider
çıkışında kapatılır. Yeni session/process group açarak kaçan kötü niyetli
programlara karşı OS izolasyonu vaat edilmez; container sandbox sonraki aşamadır.
Arama literal ve deterministiktir: 2048 girdi, 4 MiB tarama, 64 eşleşme;
symlink ve .git/target/node_modules atlanır. Patch atomik dosya değişimi kullanır,
tam bir old_text eşleşmesi ister ve dosya modunu korur. Harici editörlere karşı
transaction/CAS garantisi yoktur.

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
