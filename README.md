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
araçlar `read_file`, `list_files`, `search_files`, `diff_file` ve 5.1 ile
`inspect_file`, `read_file_chunk`, `read_document`, `read_image`.
`--allow-write` ayrıca `write_file` ve tek eşleşmeli `apply_patch` açar.
`--allow-shell` host `/bin/sh` çalıştırır: bu izin çalışma klasörü dışındaki
dosyalara ve ağa da erişim sağlar, OS sandbox'ı değildir. `--tool-allow` araç
listesini daraltır; `--tool-deny` her zaman önceliklidir. Shell'in env'i
temizlenir; yalnızca PATH/locale ve proxy/CA değişkenleri aktarılır. Anahtar komut
satırında alınmaz, kaydedilmez ve hata çıktısına eklenmez. API adaptörü Chat
Completions ve açık seçimle Responses destekler; Zen/OpenRouter profilleri için
[servis ayarlarını](docs/COMPATIBLE-SERVICES.md) oku. Codex aboneliği için
ayrı native sürücüyü kullan.
Önceki oturumdan kimlik bilgileri taşınmadı. Yeni bir anahtar paylaşman gerekmez.

SDK giriş noktası `namzu_core::run_turn`; sağlayıcılar `Provider`, araç kümeleri
`Tools` trait'ini uygular. `ScriptedProvider` dış hizmet olmadan aynı döngüyü
sınar. CLI bu SDK'yi kullanır; çekirdek CLI'ye bağımlı değildir.

## Sınırlar

- Varsayılan turn bütçesi: 300 saniye, 64 sağlayıcı adımı, 128 araç çağrısı,
  4 MiB konuşma verisi (görseller dahil). CLI süreyi 1–86400 saniyeyle sınırlar.
- Eski tam dosya okuma/yazma 64 KiB, listeleme 512 girdiye sınırlıdır. 5.1
  parçalı okumada 16 MiB, JSON/JSONL kaynağında 2 MiB destekler;
  [ayrıntılar](docs/FILES.md). Mutlak ve `..`
  yollar reddedilir. `cap-std` dosya erişimini kök dizin yeteneğiyle sınırlar;
  bu bir süreç/Docker sandbox'ı değildir.
- Ctrl-C iptali etki sınırlarında denetlenir. HTTP başlık/gövde/SSE beklemeleri async transport
  üzerinden iptal/deadline ile kesilir.
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
- TUI 1.1–1.4, Pals 2.1–2.6, MCP 3.1–3.8 ve bilgisayar/tarayıcı 4.1–4.4
  tamamlandı. Gelişmiş dosya/ek 5.1, ses 5.2, LSP 5.3, Docker sandbox 5.4 ve
  capability/config/exec/TUI/Pal entegrasyonu 5.5 tamam. Ortak sağlayıcı sözleşmesi
  6.1 ve Zen/Zen Go/OpenRouter servis profilleri 6.2 tamam. API Chat/Responses,
  opt-in SSE, private reasoning replay ve raporlanan ücret desteği var;
  [kapsam ve kullanım](docs/COMPATIBLE-SERVICES.md). 6.3 ile Anthropic/Google/
  Bedrock/Ollama native JSON, DeepSeek Chat ve LM Studio HTTP profilleri eklendi;
  [desteklenen yollar ve eksikler](docs/NATIVE-PROVIDERS.md). 6.4 model keşfi,
  özel katalog seçimi ve raporlanan limit/yetenek doğrulaması eklendi;
  [kullanım ve giriş token sayımı sınırı](docs/MODEL-CATALOG.md).
  6.5 ortak sağlayıcı kabulü: 28 kombinasyonda aynı senaryolar ve açık canlı smoke
  koşucusu; [matris ve sınırlar](docs/PROVIDER-CONFORMANCE.md).
  7.1 ile opt-in içeriksiz yerel trace/ölçümler ve exec/TUI/Pal entegrasyonu eklendi;
  [kullanım, gizlilik ve kalıcılık sınırları](docs/TELEMETRY.md).
  Sırada AG-UI olay eşlemesi/istemci entegrasyonu 7.2 var.

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

Önkoşul: OpenAI'nin [resmî giriş belgesine](https://developers.openai.com/codex/auth)
göre cihaz koduyla giriş, kişisel hesabın ChatGPT güvenlik ayarlarında etkin
olmalıdır. Yönetilen çalışma alanında bu izin workspace yöneticisine bağlıdır.
Kod reddedilirse önce bu izni ve tarayıcıda doğru ChatGPT hesabında olduğunu
kontrol et; süre dolmadan aynı kodu kullanabilirsin. Kod reddi tek başına bu
ayarın kapalı olduğunu kanıtlamaz. Kodlar 15 dakika içinde tamamlanmalıdır.

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

Yanıtın Content-Type alanından bağımsız olarak gerçek SSE içeriği doğrulanır;
Responses/SSE tamamlanmadan araçlar çalıştırılmaz. Kesik/incomplete/refused
yanıtlar başarısızdır. Yerel SSE sınırları: 1 MiB olay/satır, 16 MiB akış,
4096 output item. Opaque reasoning ve native tool item'ları özel session'a
kaydedilip yalnızca aynı hesap/model/endpoint ve değişmemiş mesajla yeniden
gönderilir. Başka sağlayıcının isteklerine eklenmez. Codex metin delta'ları
TUI'ye geçici görünüm olarak aktarılır; araçlar hâlâ completed yanıtı bekler.

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

## 0.5 — deterministik benchmark

```sh
namzu-rs bench demo --out artifacts/bench
namzu-rs bench init --model ORTAK_MODEL_ID --jcode /kurulu/jcode --file artifacts/live-suite.json
namzu-rs bench validate artifacts/live-suite.json
namzu-rs bench run artifacts/live-suite.json --out artifacts/bench
```

Native Rust koşucu aynı girdi/modelle ajanları sırayla, en fazla 300 saniyede
çalıştırır. Bağımsız dosya/test oraklları, JSON raporları ve wait4 max-RSS ölçümü
vardır. Offline demo model testi değildir: doğru fixture 3/3, no-op 0/3 almalıdır.
2026-10-17 canlı kabulünde GPT-6-Luna ile iki ajan da 3/3 yaptı.
Hesap/model erişimi, araç döngüsü ve session resume doğrulandı;
ölçümler ve sınırlar [kabul kaydında](bench/ACCEPTANCE.md).
Tam sözleşme ve RAM/izin karşılaştırmasının sınırları [bench/README.md](bench/README.md).

## 1 — etkileşimli terminal / TUI

```sh
./target/release/namzu-rs tui --demo
./target/release/namzu-rs tui --provider codex --model gpt-6-luna --effort low
```

Native Rust ekranında çok satırlı Unicode editör, açılabilir araç/diff kartları, Codex yanıt akışı, araç durumları ve
başarılı session'da devam vardır. Enter gönderir, Ctrl-C turn'ü iptal eder,
Ctrl-N açıkça yeni konuşma başlatır, Esc çıkar. Demo geçici klasörde scripted
araç döngüsüdür; model/hesap kullanmaz. Pipe için `exec --json` kullanılır.
İzinler exec ile ortaktır; shell/yazma açık CLI bayrağı ve her çağrıda TUI onayı
ister. HTTP beklemeleri iptal edilebilir. F5 session/geçmiş seçer; F6 model,
effort ve süre/adım/araç/konuşma byte bütçesini düzenler. Ayar uygulamak açıkça
yeni konuşma başlatır; başarısız session yalnızca görüntülenir. 103 Rust testi,
25 Linux PTY senaryosu ve ayrı canlı Codex görevi geçti.
[Kullanım ve sınırlar](docs/TUI.md), [kabul kaydı](docs/TUI-ACCEPTANCE.md).

## 2 — Pals

Çocuk görevler için `namzu-pals` SDK paketi eklendi (2.1–2.6). Dar araç yetkileri,
ortak rezervasyon bütçesi, parent deadline/iptali, ayrı görev sonuçları ve
sınırlı paralel ekipler var. Kalıcı kimlik/profil revizyonları, özel workspace,
görev/session yaşam döngüsü ve çökme sonrası replay reddi de çalışır.
`cargo run -p namzu-pals --example offline --locked` ile hesap kullanmadan
çocuk/ekip/kalıcı Pal örneği çalışır.
`cargo run -p namzu-pals --example mailbox --locked` ile kalıcı mesaj kuyruğu,
otomatik işleme ve duplicate/ack kontrolü hesap kullanmadan denenir. Süreli lease,
restart uzlaştırması ve belirsiz etkide replay reddi var.
`namzu-computer` native Rust köprüyle ayrı ağsız Docker bilgisayarları sunar:
CPU/RAM/PID sınırı, ikili dosya aktarımı, guest shell, restart ve sahip olunan
container/volume temizliği. `python3 scripts/build_pal_image.py` ile image kurulur;
gerçek kabul ve sınırlar [docs/COMPUTERS.md](docs/COMPUTERS.md) içindedir.
187 Rust testi, 24 gerçek Docker kontrolü ve 30 terminal senaryosu geçti.
`pal create/ask/team/send/work/computer/tui` komutları artık aynı SDK çalışma
akışına bağlıdır; model/hesap ve izin tavanları yeniden kontrol edilir.

```sh
namzu-rs pal --provider codex --model MODEL_ID create Reader
namzu-rs pal ask PAL_ID 'Çalışma alanını listele'
namzu-rs pal tui PAL_ID
```

[CLI/TUI kullanımı](docs/PALS-CLI.md), [kabul ve sınırlar](docs/PALS-ACCEPTANCE.md),
[SDK sözleşmesi](docs/PALS.md).

## 3.1 — MCP protokol temeli

Bağımsız `namzu-mcp` paketi JSON-RPC, initialize/sürüm anlaşması, capability
kontrolü, ID eşleştirme, mesaj sınırları ve parent deadline/iptali uygular.
İptal edilen isteğin geç yanıtı başka bir görevin sonucu sayılmaz; belirsiz
istek otomatik tekrarlanmaz. Stdio bağlantısı 3.2'de eklendi; HTTP taşıması ve CLI MCP araçları sonraki parçalardadır.

```sh
cargo run --release -p namzu-mcp --example protocol --locked
```

26 yeni protokol testi; toplam 213 Rust testi, format/Clippy/release ve offline
örnek geçti. Canlı model veya gerçek MCP server çağrısı 0.
[Protokol sözleşmesi ve kabul](docs/MCP.md).

## 3.2 — MCP stdio

Native Rust stdio istemcisi açık sunucu executable/argv/cwd/env ile gerçek
MCP bağlantısı kurar. Bounded nonblocking stdin/stdout/stderr, JSON satır framing,
pipeline ID eşleştirme, iptal/timeout ve process group cleanup uygular.
Belirsiz işler yeniden oynatılmaz. Client aynı kalıcı thread'de kullanılır.

```sh
cargo run --release -p namzu-mcp --example stdio -- /absolute/server literal-args
```

25 yeni test, toplam 238 Rust testi; format/Clippy/release ve gerçek yerel fixture
subprocess örneği geçti. Model kotası kullanılmadı. Test server'ı Python 3 stdlib'dir;
üretim stdio istemcisi Rust'tır. [Kullanım, kabul ve kapsam](docs/MCP-STDIO.md).

## 3.3 — MCP Streamable HTTP/SSE

Native Rust HTTP istemcisi JSON/SSE, session ve sürüm başlıkları, korunmuş GET
akışı, açık DELETE, parent iptal/deadline ve belirsiz etkide replay reddi sunar.
TLS doğrulaması ve ortam proxy/CA ayarları korunur. 30 yeni test, toplam 268
Rust testi; yerel gerçek TLS/CONNECT proxy kabulü ve release HTTP örneği geçti.
Model çağrısı 0. [Kullanım ve sınırlar](docs/MCP-HTTP.md).

```sh
cargo run --release -p namzu-mcp --example http -- https://server.example/mcp
```

OAuth 3.7, yeniden bağlanma ve recovery 3.8'dedir.
Eski 2024 HTTP+SSE endpoint keşfi desteklenmez.

## 3.4 — MCP config, yönetim ve güven

Özel kullanıcı profilleri ve açıkça seçilen proje config'i, içerik kimliğine bağlı
trust, deny/allow/approval birleşimi ve tek kullanımlık işlem izni eklendi.
Manager gerçek stdio/HTTP bağlantılarını aynı thread'de yönetir; belirsiz etkiler
saklanır ve otomatik replay yapılmaz. 27 yeni test, toplam 295 Rust testi.

```sh
namzu-rs mcp init
namzu-rs mcp import mcp.config.example.json
namzu-rs mcp list
namzu-rs mcp check SERVER --approve --timeout 300
```

Örnek sunucular disabled ve yollar yer tutucudur; gerçek endpoint/program
seçilmelidir. Model kotası kullanılmadı. [CLI/SDK, kabul ve sınırlar](docs/MCP-MANAGEMENT.md).

## 3.5 — MCP keşif ve sunucu sağlayıcıları

Tools/resources/prompts/templates için atomic katalog, opaque cursor sayfalama,
isim/şema metadata sınırları, typed içerik ayrıştırma ve list_changed invalidation
var. Her sayfa mevcut güven/izin kontrolünden geçer; eski katalog grant'ı yeniden
kullanılamaz. 40 yeni test; toplam 335 Rust testi. Model çağrısı 0.

```sh
namzu-rs mcp discover SERVER tools --approve
namzu-rs mcp discover SERVER resources --approve
namzu-rs mcp discover SERVER prompts --approve
namzu-rs mcp discover SERVER templates --approve
```

[SDK/CLI, kabul ve sınırlar](docs/MCP-DISCOVERY.md). Tam JSON Schema instance
kontrolü ve normal exec/TUI/Pal model araç entegrasyonu **3.6**'dadır.
3.6 entegrasyonu ve 3.8 toparlanma tamamlandı. Bilgisayar ekran/girdi **4.1** ve
tarayıcı/DOM **4.2** ve sayfa eylemleri/dosya **4.3** hazır; sıradaki iş
**4.4 — Pal/SDK/TUI görsel araç entegrasyonu**.

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

## MCP ile ajan çalıştırma

MCP 3.1–3.8 hazır: açık `--mcp-server`, normal exec/TUI ve Pal entegrasyonu,
input/output JSON Schema doğrulaması ve owned guest stdio. Kullanım, izin
davranışı ve mevcut sınırlar [MCP ajan belgesinde](docs/MCP-AGENT.md).
OAuth 3.7 ve açık reconnect/toparlanma 3.8 tamamlandı.

## MCP OAuth

HTTP sunucularına açık `mcp auth login SERVER --approve` ile bağlanılabilir.
PKCE/S256, loopback callback, özel token deposu ve bağlantı öncesi refresh
uygulanır. `mcp auth status SERVER` durumu gösterir; `mcp auth logout SERVER`
yerel credential'ı siler. Ajan giriş başlatmaz ve belirsiz refresh/araç
çağrısını tekrar etmez. Örnek config ve sağlayıcı uyumluluk sınırları
[OAuth belgesinde](docs/MCP-OAUTH.md).

## MCP toparlanma ve abonelikler

3.8 tamam: kalıcı belirsiz tool-call fence, açık reconnect/rebind, resource
subscribe/update/unsubscribe ve generation sınırları. `mcp recovery list`
inceleme kayıtlarını verir; `acknowledge ID --inspected` yalnızca fence'i
kaldırır. `mcp watch` sınırlı abonelik akışıdır. Eski araç çağrısı veya abonelik
otomatik tekrarlanmaz. [Kabul ve sınırlar](docs/MCP-RECOVERY.md).

## Pal masaüstü kontrolü

4.1 tamam: açıkça seçilen desktop image ile owned Linux Xvfb ekranı,
PNG screenshot, koordinat, fare ve klavye işlemleri kullanılabilir.
`pal --allow-input computer PAL_ID desktop request.json` girdi iznini yalnızca
o komuta verir; screenshot `--output NEW_FILE.png` ile özel dosyaya yazılır.
Masaüstü klavye metni US-layout ASCII ile sınırlıdır; 4.3 DOM fill Unicode destekler.
Model/TUI görsel araç entegrasyonu 4.4 kapsamındadır.
[İmaj oluşturma, SDK/CLI kullanımı ve kabul](docs/DESKTOP.md).

## Pal tarayıcı kontrolü

4.2 tamam: açık `create --desktop --browser` seçimiyle owned Chromium başlar.
`pal --allow-navigate computer PAL_ID browser request.json` gezinme yapar;
`info` ve `snapshot` istekleri sayfa metadata, sınırlı DOM ve erişilebilirlik
ağacını verir. Aynı Xvfb ekranının PNG'si desktop API'sinden alınır.
Tarayıcı mevcut ağsız container içinde çalışır; guest yerel HTTP sayfalarıyla
doğrulandı, internet erişimi açılmaz. Gezinme hatası/iptali stop/pending ve açık
inspect/ack korumasını kullanır. [SDK/CLI ve sınırlar](docs/BROWSER.md).

4.3 tamam: snapshot belgesine bağlı click/fill/select/press/scroll işlemleri,
ayrı `--allow-browser-actions`, `--allow-upload`, `--allow-download` izinleri,
64 KiB guest dosyaları ve create-new host export. Fill Unicode destekler;
eski belge/düğüm ve belirsiz etki otomatik tekrarlanmaz. Başarılı host dosya
aktarımı çalışan browser oturumunu korur. [Eylem/dosya kullanımı ve kabul](docs/BROWSER-ACTIONS.md).

## Görsel ajan araçları

4.4 ile owned masaüstü/tarayıcı araçları SDK, normal exec/TUI ve Pals üzerinden
kullanılabilir. Görsel izinler açıkça verilir; TUI F7 ile son PNG'yi gösterir ve
etki araçları tek kullanımlık onay ister. Kullanım, izinler, model görüntü
sözleşmesi, kalıcı Pal ve temizlik: [VISUAL-AGENT.md](docs/VISUAL-AGENT.md).

## Gelişmiş dosya ve ek araçları

5.1: SHA-256 kimliğiyle büyük UTF-8 dosyalarını parça parça oku, JSON/JSONL
alanlarını ve kayıtlarını seç, PNG dosyasını modele gerçek görsel olarak aktar.
Normal TUI ve Pal TUI'de F7/özel geçmiş aynı ek yolunu kullanır.
[Kullanım, SDK ve sınırlar](docs/FILES.md).

## Ses SDK ve sürücüler

5.2: PCM16 WAV doğrulama, ses→metin ve metin→ses sürücüleri, SDK olayları ve
açık `audio inspect/transcribe/synthesize` komutları. Kaynak/çıktı en çok 12 MiB
ve 60 saniye; çıktı yeni private WAV dosyasıdır. Mikrofon/playback ve TUI'den
otomatik ses aktarımı bu aşamada yoktur. [Kullanım ve kabul](docs/AUDIO.md).

## LSP ve kod zekâsı

5.3: açık seçilen Unix stdio LSP sunucusu, typed diagnostics, tanım/referans
bulma, rename önizlemesi ve hash/sürüm bağlı tek dosya apply. `lsp` komutları
model kullanmaz; sunucu host programıdır, sandbox değildir.
[SDK, CLI, izinler ve kabul sınırları](docs/LSP.md).

## Sandbox politikaları

5.4: bağımsız `namzu-sandbox` SDK, inspect/readonly/build profilleri ve açık
`sandbox policy/check/run/read/list/show/read-retained/cleanup` komutları var.
Owned Docker üzerinde dosya/süreç/ağ sınırları uygulanır; native backend açıkça
reddedilir, host fallback yoktur. Normal exec/TUI/Pal capability entegrasyonu
5.5 ile hazırdır. [Kullanım, kabul ve sınırlar](docs/SANDBOX.md).

## Yetenek kaydı ve izinler

5.5: `namzu-capabilities` SDK ve `capabilities list` araç/izin kataloğu var.
`--capability audio,lsp`, `--allow-audio`, `--allow-lsp` ve gereken yerde
`--allow-write` açıkça seçilir; config kendiliğinden servis/program açmaz.
Exec, normal/Pal TUI ve Pal görevlerinde aynı izin ve pinned route kuralları
uygulanır. `--computer-image` + `--sandbox-policy` seçilirse dosya/shell guest'te
kalır ve sandbox workspace'i turn'ler arasında korunur. [Kullanım ve kabul](docs/CAPABILITIES.md).

## Ortak model sağlayıcı sözleşmesi

6.1: `Provider::info/take_metadata`, ortak request/completion sınırları,
opsiyonel token kullanımı, ayrı bildirilmiş/tahmini fixed-point maliyet ve typed
HTTP hataları API/Codex sürücülerine bağlandı. Eksik kullanım ücretsiz sayılmaz;
otomatik retry veya model fallback yoktur. CLI/Pal stats ve TUI token gösterimi
ortaktır. [SDK, upstream envanteri ve kabul](docs/PROVIDER-CONTRACT.md).
