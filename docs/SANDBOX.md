# Sandbox — 5.4

`namzu-sandbox` bağımsız politika SDK'sıdır. `namzu-computer` politikayı owned
**yerel Linux Docker** container'ına uygular. Model/hesap çağrısı yoktur.
Normal `exec --allow-shell` hâlâ host shell'dir; bu yeni politikayı otomatik
kullanmaz. Genel exec/TUI/Pal yetenek/config entegrasyonu **5.5** kapsamındadır.

## Açık CLI

```sh
python3 scripts/build_pal_image.py
# artifacts/pal-image.json içindeki image alanını seç:
namzu-rs sandbox capabilities
namzu-rs sandbox policy --profile build > generated.json
python3 -c 'import json; print(json.dumps(json.load(open("generated.json"))["output"]["policy"]))' > policy.json
namzu-rs sandbox check policy.json
namzu-rs sandbox --state-dir /private/state run \
  --policy policy.json --image sha256:LOCAL_IMAGE_ID 'printf hello > result; cat result'
```

Image ID tam 64 haneli SHA-256 olmalı ve yerelde guest protocol 1 etiketiyle
bulunmalıdır. Runtime pull/build yapmaz. CLI `read` ve `list` aynı `--policy` /
`--image` seçimini alır. `read` ikili veriyi base64 döndürür; relative workspace
path ister. Çalışma alanı yeni private named volume'dur: host proje klasörü,
Docker socket'i veya credential dosyaları mount edilmez. Başlangıç dosyaları
önceden seçilen image'ın `/workspace` içeriğiyle Docker volume copy-up üzerinden
verilebilir; SDK yazılabilir profilde `Computer::upload` da sunar. CLI otomatik
host proje içe aktarması yapmaz.

| Profil | Workspace | Kullanıcı komutları | Ağ |
|---|---|---|---|
| `inspect` | salt okunur | kapalı; dosya read/list | kapalı |
| `readonly` | salt okunur | açık | kapalı |
| `build` | yazılabilir | açık | kapalı |

`readonly` komutları `/tmp` içinde yazabilir; profil çalışma alanını korur,
komutu etkisiz/pure hale getirmez. `tools_only` kullanıcı shell ve MCP stdio
başlatmayı reddeder; güvenilen guest köprüsünün read/list süreçleri çalışır.
`build` profili derleyici kurmaz; gerekli programlar seçilen image içinde olmalıdır.
ToolPolicy shell/yazma izni sandbox politikasını aşamaz. Batch doğrulaması,
policy dosya boyutunu da ilk araç etkisinden önce kontrol eder.

Her komut standart JSON envelope üretir. Tamamlanmış shell'in nonzero exit'i
CLI exit 1'dir; sağlıklı cleanup yine yapılır. Hata/acquire/timeout raporlarında
`sandbox_id` ve `computer` kaydı bulunur (kayıt deposunun kendisi okunamıyorsa
normal redakte hata döner). Temizlenen kayıtta `phase=closed`, `cleaned=true`.

## Gerçek sınırlar

Politika zorunlu tüm alanlarıyla strict JSON'dur; unknown alanlar, sürümler,
network seçenekleri ve aşırı limitler reddedilir. Dosya en fazla 64 KiB.
`Policy::parse`, `validate`, `fingerprint` Docker başlatmaz. SHA-256 canonical
serde temsiline bağlıdır; pretty JSON/alan sırası kimliği değiştirmez.

| Sınır | Varsayılan | Kabul aralığı / uygulama |
|---|---|---|
| RAM / swap | 256 MiB, ek swap yok | 32 MiB–2 GiB, Docker cgroup |
| CPU | 1000 millicpu | 100–4000, Docker cgroup |
| PID | 64 | 16–256, Docker pids controller |
| Tek dosya | 16 MiB | 1 KiB–1 GiB, `RLIMIT_FSIZE` + SDK write preflight |
| Açık dosya / süreç | 256 | 32–4096, `RLIMIT_NOFILE` |
| `/tmp` | 16 MiB | 1–256 MiB tmpfs, rw/nosuid/nodev/**noexec** |
| Komut süresi | 300 s | 1–300 s, çağrı ve parent kalan süresiyle minimum |
| Birleşik çıktı | 64 KiB | 1–65536 byte, supervised guest stdout+stderr |

Çıktı sınırı ham byte içindir; invalid UTF-8 lossily çevrilince JSON metni en
fazla üç kat byte olabilir. Süre dolunca veya çıktı taşınca bütün owned
container durdurulur. Normal lider çıkışında da guest süreç grubu/torunları
kapatılır. Shell/download/upload köprüsünün mevcut 64 KiB transfer sınırı
politika tek dosya limitinden bağımsız üst sınırdır.

Container root salt okunur; nonroot UID/GID 65532, cap-drop ALL,
no-new-privileges, private IPC/cgroup/PID ve network none kullanılır. Ek host
bind/device/capability verilmez. Read-only workspace kernel volume mount ile
uygulanır; salt okunur politikada SDK upload da reddedilir. Ağ için izinli
hostname listesi veya online profil yoktur. Docker control-plane istemcisinin
mevcut proxy/CA/config ayarları korunur; bunlar guest'e host ağı vermez.

Her işlem ve resume/start öncesi sahiplik/nesil/policy hash etiketleri ile
image, RAM/swap/CPU/PID, root/user/network/capability/namespace ve mount ayarları
kontrol edilir. Policy seçildiğinde tmpfs, fsize/nofile/core limitleri ve
security option kümesi de denetlenir. ComputerSpec resource alanları policy
ile farklıysa acquisition öncesi reddedilir. Daha düşük çıktı bütçesi için güncel guest köprüsü gerekir; eski köprü ek wire
alanını reddederse container durdurulur, sınırsız komutla tekrar denenmez.
Eski `sandbox: null/absent`
ComputerSpec kayıtları eski wire/default davranışıyla çalışır.

Bu bir VM veya kötü niyetli Docker daemon/host yöneticisine karşı sınır
değildir. Docker daemon güven sınırındadır. RW volume için toplam disk kotası
yoktur; fsize tek dosyayı, tmpfs kotası yalnızca `/tmp` alanını sınırlar.
`noexec`, bir dosyanın interpreter'a girdi verilmesini engellemez. Image içeriği
kullanıcının seçtiği güvenilen guest uygulamasını içermelidir. Politika bu
image'ın içinde önceden bulunan verileri/env değerlerini silmez.

## Ortam seçimi ve belirsiz işlem

`backend: docker` uygulanmıştır. `backend: native` açıkça unsupported hatası
verir; bu sürüm native Landlock/seccomp sandbox sağlamaz. Remote/macOS/Windows
ve masaüstü/tarayıcı policy profilleri uygulanmamıştır. Hatalı/eksik Docker,
unsupported backend veya imajda eksik köprü durumunda host shell fallback yok.
Mevcut ayrı desktop/browser ComputerSpec akışları sandbox policy'siz kendi
önceki açık yetkileriyle çalışmaya devam eder.

Policy seçilmiş işte timeout/çıktı taşması veya belirsiz etki sonrası pending
kaydı ve durmuş container/volume tutulur. Komut otomatik tekrarlanmaz.
`show` kayıt incelemesidir; `read-retained` dosyayı incelemek için güvenilen
headless PID1'i başlatır ve yeniden durdurur, pending'i onaylamaz/silmez.
Cleanup container ve volume'u siler; kullanıcının açık recovery komutudur.

```sh
namzu-rs sandbox --state-dir /private/state show SANDBOX_ID
namzu-rs sandbox --state-dir /private/state read-retained SANDBOX_ID result
namzu-rs sandbox --state-dir /private/state cleanup SANDBOX_ID
```

SDK: `ComputerSpec::new(image).with_sandbox(Policy::build())?` ardından mevcut
`ComputerManager::acquire`. Drop sağlıklı kaynakları temizler; belirsiz işte
ayrı cleanup context'iyle durdurur ve kaydı tutar. `recover_cleanup` yalnızca
sahiplik etiketleri eşleşen kaynakları kaldırır.

## Kabul

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
python3 scripts/accept_sandbox.py
```

Normal Rust testleri Docker/model gerektirmez. Açık kabul script'i release
workspace/guest/fixture derler ve local headless imajı `--network=none` ile
kurar; model/anahtar kullanmaz. `artifacts/sandbox-acceptance.json` yerel
rapordur; kaynak deposuna özel state/artifact yüklenmez. Gerçek kabulde RO
mount, RW transfer, fsize, nofile, PID, noexec, tmpfs dolması, host loopback ve
network route izolasyonu, root RO, tools-only/MCP reddi, çıktı/süre durdurma,
retained read/cleanup ve native seçiminin host komutunu çalıştırmaması sınanır.
CPU/RAM değerleri daemon inspect ile denetlenir; stres/performance benchmark
ve native OS güvenlik sertifikasyonu iddiası yoktur.

2026-10-19 bu çalışma alanı kabulü: **503 Rust testi** (5.4 ile 8 yeni),
workspace fmt/Clippy ve release geçti. **14 gerçek Docker SDK**, **8 release CLI**
kabul grubu ve mevcut bilgisayar akışının **15 Docker regresyon kontrolü** geçti.
Yerel oracle image: `sha256:16ee0f3f39b2c770f2a83a57515c1a0f87985993bae1c01694fd12399812d261`.
Guest binary SHA-256: `193b8c95314b6ca71c63549f581c11d2b571d292e56e8b4ad8b98e757fbc0721`.
Bunlar canlı model performansı veya farklı bir native backend sonucu değildir.
