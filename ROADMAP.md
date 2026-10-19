# Yeniden kurulum durumu

2026-10-17: Eski kaynaklar mevcut ortamda bulunamadı. Aşağıdaki tüm modüller
yeniden doğrulanana kadar eksik sayılır. Önceki sohbetin tamamlandı kayıtları
bu yeni depoda tamamlanmış uygulama anlamına gelmez.

## İlk aşama

- [x] Rust workspace ve SDK/ajan çekirdeğinin ilk sürümü
- [x] Deterministik sağlayıcı ile araç çağrısı döngüsü
- [x] Sınırlı dosya araçları, terminal komutu ve JSON çıktı sözleşmesi
- [x] Süre (varsayılan 300 saniye), adım ve araç çağrısı bütçeleri
- [x] 34 test; Clippy (uyarılar hata); format; release derlemesi ve release demo
- [x] Kullanıcının seçtiği Public GitHub deposuna doğrulanmış yedek

İlk sürüm OpenAI uyumlu Chat Completions adaptörü içerir. Yerel HTTP fixture
ile CLI → sağlayıcı → dosya aracı → sağlayıcı sonucu zinciri sınandı.
Codex abonelik sürücüsü ve yeni benchmark koşucusu aşağıdaki 0.4/0.5'te eklendi.
Canlı hesap/model erişimi ve ilk ortak-model koşumu doğrulandı; bench/ACCEPTANCE.md.
Kayıp eski benchmark script'leriyle birebir uyum
iddiası yoktur; yeni sürümlü sözleşme bench/README.md'dedir.

## Doğrulanan ilk aşama — 2026-10-17

- CLI: 3 süreç testi + 2 gerçek loopback HTTP testi.
- Core: 1 deterministik deadline testi + 13 ajan döngüsü/bütçe/iptal testi.
- Sağlayıcı: 6 protokol/URL/anahtar redaksiyonu testi.
- Dosyalar: 9 yetenek sınırı/symlink/readonly/boyut/FIFO testi.
- `cargo test --workspace --locked`: 34 geçti, 0 başarısız, 0 atlandı.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: geçti.
- `cargo fmt --all -- --check`: geçti.
- `cargo build --release --workspace --locked`: geçti.
- Release `demo --json`: çıktı 12, 3 scripted sağlayıcı adımı, 2 dosya çağrısı.
- Harici model çağrısı: 0. Gerçek sağlayıcı hesabı/kota doğrulanmadı.
- Checkpoint bundle'ından geçici dizine clone sonrası Git kaynak ağacı
  eşitliği doğrulandı. Ardından ilk üç commit GitHub'a yüklendi; bağımsız temiz
  clone ile HEAD, kaynak ağacı ve geçmiş doğrulandı. Kanıt ve akış: BACKUP.md.

## Önceki kapsam

1. Etkileşimli terminal / TUI.
2. Alt ajanlar / Pals: paralel ekipler, otomatik mesaj işleme, Pal bilgisayarları.
3. MCP: 3.1 protokol, 3.2 stdio, 3.3 HTTP, 3.4 yönetim/izinler,
   3.5 keşif/sağlayıcılar, 3.6 SDK/CLI/Pal entegrasyonu,
   3.7 OAuth/kimlik bilgileri, 3.8 toparlanma ve birlikte çalışabilirlik.
   Bu alt başlıklar sohbetten yeniden çıkarılmış uygulama planıdır.
4. Tarayıcı / bilgisayar kontrolü.
5. Dosya, ses, LSP ve sandbox paketleri.
6. Model sağlayıcıları; Codex aboneliği entegrasyonu ve benchmark uyumu.
7. Telemetri, AG-UI ve değerlendirme araçları.
8. Masaüstü uygulaması.

## Önerilen uygulama sırası

Eski sekiz modülün numaralarını koruyoruz. Önlerine ortak temeli tamamlayacak
0. aşamayı koyuyoruz. Alt parçalar yeniden planlama önerisidir; eski ayrıntılı
planın birebir arşivi değildir. Yüzdelik tamamlanma iddiası yoktur.

| Sıra | Aşama | Bugünkü durum | Bitirme ölçütü |
| --- | --- | --- | --- |
| 0 | SDK, terminal araçları, session, Codex ve benchmark temeli | 0.1–0.5 tamam | Gerçek görev, oturum devamı, deterministik benchmark |
| 1 | Etkileşimli terminal / TUI | 1.1–1.4 tamam | Akış, onay, iptal, oturum devamı |
| 2 | Alt ajanlar / Pals | 2.1–2.6 tamam | Paralel ekip, mesaj işleme, sahip olunan Pal bilgisayarları |
| 3 | MCP | 3.1–3.8 tamam | Stdio/HTTP, güven/izin, entegrasyon, OAuth ve toparlanma |
| 4 | Tarayıcı / bilgisayar kontrolü | 4.1–4.4 tamam | İzole bilgisayarda tarayıcı ve görsel eylemler |
| 5 | Dosya, ses, LSP ve sandbox paketleri | 5.1–5.5 tamam | Gelişmiş yetenek paketlerinin SDK/terminal entegrasyonu |
| 6 | Diğer model sağlayıcıları | 6.1 tamam; 6.2–6.5 sırada | Ortak sözleşme testleriyle ayrı sürücüler |
| 7 | Telemetri, AG-UI ve değerlendirme | Eksik | Redakte olaylar ve tekrar üretilebilir raporlar |
| 8 | Masaüstü uygulaması | Eksik | SDK üstünde çalışan masaüstü akışları |

Bağımlılıklar nedeniyle tek Codex sağlayıcısı 0'da, temel container/sandbox
altyapısı 2'de erken kurulur. 5 ve 6 bunları genişletir. İleri ses/LSP ve ek
sağlayıcılar ilk TUI/Pals/MCP sürümlerini bekletmez.

## 0. Ortak temel — önce kullanılabilir terminal ajanı

| Parça | İş | Durum ve kabul ölçütü |
| --- | --- | --- |
| 0.1 | Workspace, SDK, ajan döngüsü | İlk sürüm tamam: scripted ve yerel HTTP akışında araç sonucu aktarımı |
| 0.2 | Terminal araçları ve izin politikası | Tamamlandı: shell, literal arama, patch/diff; izin/çıktı sınırları, süreç grubu temizliği; 45 test geçti |
| 0.3 | Config ve kalıcı oturumlar | Tamamlandı: özel/atomik kayıt, süreç lock'u, geçmiş/resume, açık config seçimi; belirsiz etki otomatik tekrarlanmaz; toplam 55 test |
| 0.4 | Codex/OpenAI abonelik bağlantısı | Tamamlandı: native device login, özel token/refresh/logout, gerçek model keşfi, SSE/reasoning replay. Canlı abonelik, model kataloğu, refresh ve araç çağrılı görevler doğrulandı |
| 0.5 | Benchmark uyumu ve ilk karşılaştırma | Tamamlandı: seri/300 saniye/bağımsız orakllar/RSS/hash/JSON; toplam 79 test. GPT-6-Luna ile ilk canlı Namzu/Jcode koşumu: bench/ACCEPTANCE.md |

0.2 tamamlandı; shell açık host yürütme iznidir, OS sandbox'ı değildir. 0.4, mevcut API-key
Chat Completions adaptöründen ayrı abonelik sürücüsüdür. Eski benchmark dosyaları
geri gelirse sözleşmesi incelenir; gelmezse yeni sözleşme açıkça belgelenir.

## 1. Etkileşimli terminal / TUI

- **1.1 — Tamamlandı:** SDK olay sözleşmesi, native Codex delta akışı ve Ratatui mesaj ekranı;
  ortak exec/journal/izinler, tek satırlı giriş ve başarılı session devamı.
  93 Rust testi, 5 gerçek PTY senaryosu ve GPT-6-Luna read-only canlı kabul;
  [kabul kaydı](docs/TUI-ACCEPTANCE.md).
- **1.2 — Tamamlandı:** Unicode çok satırlı editör, araç kartları/ayrıntıları, diff renkleri;
  96 Rust testi ve altı gerçek PTY senaryosu.
- **1.3 — Tamamlandı:** CLI izin tavanı içinde tek çağrı onayı/ret, beklerken iptal,
  async HTTP başlık/gövde/SSE iptali ve worker temizliği; 99 test, 13 PTY senaryosu.
- **1.4 — Tamamlandı:** Workspace session seçicisi ve geçmiş, uyumlu/complete
  session devamı, model/effort ve gerçek SDK bütçeleri, resize/termios/sinyal yaşam
  döngüsü; 103 Rust testi, 25 PTY senaryosu ve ayrı canlı Codex kabulü.

Kabul: deterministik olay/etkileşim kontrolleri ve gerçek terminal smoke testi;
terminal kapandığında artık çalışan alt süreç bırakmama.

## 2. Alt ajanlar / Pals

- **2.1 — Tamamlandı:** Tek kullanımlık çocuk görev, typed sonuç, dar araç kabulü,
  ortak rezervasyon bütçesi ve kesin parent deadline/iptal aktarımı; 112 Rust testi.
  SDK sözleşmesi ve sınırlar [docs/PALS.md](docs/PALS.md).
- **2.2 — Tamamlandı:** Sınırlı worker havuzu, sıralı sonuçlar, scope/owner kabulü,
  ekip/çocuk iptali, kuyruk ve join/panic yaşam döngüsü; 119 Rust testi.
- **2.3 — Tamamlandı:** Özel kalıcı kimlik/revizyon/çalışma alanları, görev-session
  bağı, tek süreç sahibi, pause/archive, checkpoint, açık followup ve çökme sonrası
  replay reddi; toplam 135 Rust testi. Offline çalışan SDK örneği ve 25 TUI PTY
  regresyonu. Sözleşme/sınırlar [docs/PALS.md](docs/PALS.md).
- **2.4 — Tamamlandı:** Atomik mesaj/task yayını, sender/payload dedup, FIFO,
  süreli lease/generation/ack, bütçeli otomatik drain, restart uzlaştırması ve
  belirsiz model/araç çalışmasını tekrar etmeme; 157 Rust testi ve offline
  mailbox örneği. 22 yeni test, beş gerçek süreç çökme sınırı; docs/PALS.md.
- **2.5 — Tamamlandı:** Native Rust guest köprüsü, sahibi/generation bağlı ağsız
  container, CPU/RAM/PID sınırları, binary dosya/shell araçları, kalıcı volume,
  start/stop/restart/cleanup ve çökme sonrası pending koruması; 174 Rust testi,
  15 gerçek Docker kontrolü ve 25 TUI PTY regresyonu. Disk kotası ve normal CLI-TUI
  akışı bu 2.5 tesliminde yok; kapsam [docs/COMPUTERS.md](docs/COMPUTERS.md).
- **2.6 — Tamamlandı:** Açık `pal` CLI grubu; model/hesap ve izin bağlı görev,
  followup, sınırlı paralel ekip, mesaj pump'ı ve bilgisayar yönetimi; ortak
  host/guest adaptörü ve Pal TUI geçmiş/onay/iptali. 187 Rust testi, 30 gerçek
  Linux PTY ve 24 gerçek Docker kontrolü; canlı model çağrısı 0.
  [Kullanım](docs/PALS-CLI.md), [kabul/kapsam](docs/PALS-ACCEPTANCE.md).

Kabul: birbirinden izole Pals, daraltılan izinler, deterministik paralel
sonuçlar, çökme/iptal ve gerçek container temizliği. Görsel yetenekler 4'te.

## 3. MCP — sekiz küçük teslim

Yeni depoda MCP 3.1–3.8, 2026-10-18 tarihinde yeniden uygulandı.
Eski projede 3.6'ya kadar ilerlenmişti; 3.7 başlamamıştı. Aşağıdaki kapsam önceki ilerleme kaydından yeniden düzenlendi.

| Parça | Kapsam | Kabul ölçütü |
| --- | --- | --- |
| **3.1 — tamam** | Protokol ve veri tipleri | JSON-RPC, sürüm/initialize, ID eşleştirme, sınırlar, hata/iptal; 26 yeni test |
| **3.2 — tamam** | Stdio taşıma | Kontrollü env/cwd, shell'siz başlatma, bounded framing/I/O, stderr kotası, süreç grubu/temiz kapatma; 25 yeni test |
| **3.3 — tamam** | HTTP taşıma | İki Streamable HTTP sürümü, bounded JSON/SSE, session/GET/DELETE, deadline/iptal, TLS/CONNECT proxy; belirsiz etkiyi tekrarlamama; 30 yeni test |
| **3.4 — tamam** | Config, yönetim ve güven | Özel/atomik profiller, içerik bağlı proje trust, layered allow/deny/onay, tek kullanımlık grant, stdio/HTTP manager ve CLI; 27 yeni test |
| **3.5 — tamam** | Keşif ve sunucu sağlayıcıları | Atomic tools/resources/prompts/templates katalogları, bounded sayfalama, bildirim invalidation, isim/şema metadata sınırları ve typed sonuçlar; 40 yeni test |
| **3.6 — tamam** | SDK/CLI/TUI/Pals entegrasyonu | Araç/içerik aktarımı ve JSON Schema instance doğrulaması, çocuk izin tavanı, guest stdio; sessiz host fallback yok |
| **3.7 — tamam** | OAuth ve kimlik bilgileri | Metadata discovery, PKCE/state/callback, token saklama/refresh; otomatik etkileşimli login yok |
| **3.8 — tamam** | Toparlanma ve birlikte çalışabilirlik | Kopma/reconnect, subscription yaşam döngüsü, farklı fixture sunucuları |

Kabul: yerel stdio/HTTP/OAuth fixture'ları; yanlış ID/sürüm/sayfa, izinsiz araç,
çocuk yetki yükseltme ve iptal sonrası etki kontrolleri. Normal testler canlı
model/hesap kotası tüketmez. Her 3.x ayrı commit ve GitHub yedeğidir.

## 4. Tarayıcı / bilgisayar kontrolü

- **4.1 — tamam** — Owned Xvfb masaüstü, native Rust screenshot/koordinat/klavye/fare,
  açık CLI girdi izni, PNG doğrulaması ve iptal/pending koruması. Gerçek Docker
  GUI/CLI kabulü ve Pal regresyonu geçti. US-layout ASCII; Unicode henüz yok.
  [Kullanım ve sınırlar](docs/DESKTOP.md).
- **4.2 — tamam** — Owned Chromium yaşam döngüsü, native Rust CDP worker,
  açık gezinme izni, redirect/fragment, sınırlı DOM/AX ve aynı Xvfb ekranı.
  Gerçek Docker yerel HTTP oracle'ıyla SDK/CLI kabulü; ağsız tek sekme.
  [Kullanım ve sınırlar](docs/BROWSER.md).
- **4.3 — tamam** — Belge/düğüm bağlı click/fill/select/press/scroll, Unicode DOM
  fill, ayrı action/upload/download izinleri, bounded dosya aktarımı ve iptal.
  Gerçek tarayıcıda binary upload/download, stale hedef, boyut ve iptal kabulü.
  [Kullanım ve sınırlar](docs/BROWSER-ACTIONS.md).
- **4.4 — tamam** — SDK görsel araçları, typed PNG/history ve Chat/Codex image wire,
  exec/TUI ephemeral bilgisayar, F7 renkli önizleme/tek kullanımlık onay, Pal
  izin tavanı ve kalıcı tarayıcı devamı. Gerçek Chromium/HTTP ajan, PTY onay/ret/
  iptal ve pending/owned temizlik doğrulandı. [Kullanım ve sınırlar](docs/VISUAL-AGENT.md).

## 5. Dosya, ses, LSP ve sandbox paketleri

- **5.1 — tamam** — SHA-256 bağlı UTF-8 sayfaları (16 MiB kaynak), JSON/JSONL
  pointer/sayfalama (2 MiB), PNG ekleri ve typed model/TUI history. 11 yeni
  test, toplam 450; gerçek release CLI/HTTP ve normal/Pal PTY kabulü.
  [Kullanım ve sınırlar](docs/FILES.md).
- **5.2 — tamam** — Bağımsız ses SDK, bounded PCM16 WAV, transkripsiyon/sentez
  sürücüleri ve payload içermeyen olaylar; OpenAI uyumlu iptal edilebilir HTTP,
  açık CLI ve private/create-new WAV çıktısı. 23 yeni test, toplam 473; sekiz
  gerçek release CLI/HTTP kabulü. [Kullanım ve sınırlar](docs/AUDIO.md).
- **5.3 — tamam** — Bounded Unix stdio LSP, initialize/sync/shutdown, UTF-16,
  push/pull diagnostics, definition/references ve hash/version bağlı tek dosya
  edit planı/atomik apply. Açık CLI, 22 yeni test, toplam 495 ve sekiz release
  CLI/subprocess kabul grubu. [Kullanım ve sınırlar](docs/LSP.md).
- **5.4 — tamam** — Strict/hash-bound sandbox SDK, inspect/readonly/build profilleri,
  RO/RW owned Docker workspace, offline ağ, fsize/nofile/tmpfs ve RAM/CPU/PID
  sınırları; açık CLI, timeout/output fence ve recovery. Native/remote/visual
  policy backend'leri desteklenmez; host fallback yok. [Kabul ve sınırlar](docs/SANDBOX.md).
- **5.5 — tamam** — Bağımsız capability SDK/katalog, typed config, opt-in ses/LSP
  model araçları, normal/Pal TUI çağrı onayı ve profile/session kimliği. Sandbox
  exec/Pal devamı, owned cleanup ve host fallback reddi. 13 yeni Rust testi;
  toplam 516, fmt/Clippy/release. 15 CLI/PTY/Docker kabul grubu ve 25 mevcut
  TUI regresyon kontrolü. [Kullanım ve sınırlar](docs/CAPABILITIES.md).

Kabul: dosya/format sınırları, gerçek yerel LSP fixture'ı, izinsiz erişimin reddi.
Yerel testler için ücretli ses hizmeti zorunlu değildir.

## 6. Diğer model sağlayıcıları

- **6.1 — tamam** — Core descriptor/request/completion sözleşmesi; API/Codex
  normalize kullanım, kısmi kapsama, explicit fiyatla fixed-point tahmin ve
  redakte typed HTTP hataları. CLI/Pal stats ve TUI token sunumu. Upstream
  snapshot envanteri: dokuz sağlayıcı paketi + HTTP helper.
  16 yeni test, toplam 532; fmt/Clippy/release, 6 CLI/Pal/PTY kabul grubu ve
  25 mevcut TUI regresyon kontrolü. [Sözleşme ve kapsam](docs/PROVIDER-CONTRACT.md).
- **6.2** — Zen/OpenRouter dâhil OpenAI uyumlu servislerin özel davranışları.
- **6.3** — Anthropic, Google, DeepSeek ve upstream envanterindeki diğer sürücüler.
- **6.4** — Model keşfi, bağlam sınırları, açık sağlayıcı/model seçimi; sessiz ücretli fallback yok.
- **6.5** — Her sürücüye ortak sözleşme testleri; açık istekle canlı smoke testi.

Başlamadan upstream paket envanteriyle tam sağlayıcı listesi netleştirilir.
Anahtarın varlığı servis/model erişimini kanıtlamaz.

## 7. Telemetri, AG-UI ve değerlendirme araçları

- **7.1** — Ortak olay şeması, trace/ölçümler, kimlik bilgisi ve içerik redaksiyonu.
- **7.2** — AG-UI: mesaj/araç/ilerleme olayları, iptal ve istemci entegrasyonu.
- **7.3** — Deterministik eval görevleri, dosya/test oraklları ve başarısızlık sınıfları.
- **7.4** — Jcode/Namzu karşılaştırması: aynı model/görev/izin/300 saniye bütçe,
sıralı koşum, tekrarlar; doğruluk/süre/peak RSS/araç ve model çağrıları.

Kabul: tekrar üretilebilir yerel rapor; model başarımı, araçlar ve çalışma zamanı
maliyeti ayrı ölçülür. Gizli içerik telemetriye taşınmaz.

## 8. Masaüstü uygulaması

- **8.1** — Rust çekirdeğine bağlanan masaüstü kabuğu ve IPC/olay sözleşmesi.
- **8.2** — Sohbet, proje, oturum, araç onayları ve model ayarları.
- **8.3** — Pals/ekip/bilgisayar görünümleri; tarayıcı ve dosya işlemleri.
- **8.4** — Paketleme, güncelleme, işletim sistemi anahtar kasası ve kabul testleri.

Teknoloji seçimi bu aşamada yapılır. SDK ve CLI masaüstüne bağımlı olmaz.

## Bir parçanın tamamlanma koşulu ve sıradaki iş

Çalışan kod, anlamlı doğrulama, belgelenmiş sınırlar, güncel durum; ardından
commit, GitHub push ve uzak commit kontrolü. Gönderilemeyen aşama açıkça
bildirilir. Büyük parçalar çalışır ara teslimlere bölünür; ana modül erken
tamamlandı sayılmaz. Her teslimden sonra aynı depoya yükleme talimatı kalıcıdır.

**Sıradaki iş: 6.2 — Zen/OpenRouter dahil OpenAI uyumlu servis davranışları.**
6.1 tamamlandı: Provider capability/metadata kontratı, mevcut API + Codex
sürücülerinde bounded request/response, typed hata ve optional usage/cost.
Eksik veri sıfır/ücretsiz sayılmaz; retry/fallback/login eklenmedi.
[Upstream envanteri, kullanım ve sınırlar](docs/PROVIDER-CONTRACT.md).

5.5 tamamlandı: seçilen paketlerin SDK/exec/TUI/Pal araçları, config/CLI/profil
izin kesişimi, canonical route pin ve sandbox workspace devamı. Native sandbox,
guest üzerinde ses/LSP ve büyük ek protokolü bu teslimde uygulanmadı;
[mevcut kapsam ve kabul](docs/CAPABILITIES.md). Canlı model çağrısı 0.

5.4 tamamlandı: ayrı politika SDK ve açık sandbox CLI, gerçek Docker sınır
kontrolleri, policy kimliğine bağlı owned resource doğrulaması ve belirsiz işte
stop/retain/no replay. 503 Rust testi; 14 Docker SDK + 8 CLI kabul grubu ve
15 mevcut Docker regresyon kontrolü, fmt/Clippy/release geçti. Normal exec/TUI/Pal entegrasyonu 5.5'tedir; native sandbox
henüz uygulanmadı. Canlı model çağrısı 0. [Ayrıntılar](docs/SANDBOX.md).

5.3 tamamlandı: explicit host server, bounded framing/I/O, process-group
cleanup, belge kimliği/UTF-16, typed sonuçlar ve preview/ayrı write izni.
Çok dosyalı/resource edit ve otomatik server apply reddedilir; TUI/Pal tool
entegrasyonu 5.5'tedir. Canlı model çağrısı 0; gerçek bağımsız yerel LSP
fixture'ı kullanıldı, rust-analyzer bu ortamda kurulu değildi.
[Kabul ve SDK/CLI kullanımı](docs/LSP.md).

5.2 tamamlandı: format/süre/byte/UTF-8 sınırları, typed/redakte SDK olayları,
HTTP iptal/deadline, no replay, canonical WAV ve unknown-size HTTP WAV desteği.
Mikrofon/playback ve TUI/Pal ses araçları bu aşamada yok; capability entegrasyonu
5.5'tedir. Canlı model çağrısı 0. [Kabul ve kullanım](docs/AUDIO.md).

5.1 tamamlandı: dört salt okunur dosya/ek aracı, açık kaynak/boyut/bütçe
sınırları, değişen hash reddi ve PNG normalizasyonu. JSON/JSONL desteklenir;
PDF/Office/JPEG, URL fetch ve kalıcı blob kayıt sistemi bu kapsamda yoktur.
Canlı model çağrısı 0. [Kabul ve SDK/CLI kullanımı](docs/FILES.md).

4.4 tamamlandı: 13 yeni test, toplam 439 Rust testi; format/Clippy/release,
6 gerçek Docker/Chromium/HTTP ve PTY kabul senaryosu; 4.2/4.3 SDK/CLI regresyonu.
PNG görsel veri olarak iki sağlayıcıya aktarılır, konuşma bütçesine dahildir.
Canlı model çağrısı 0. Offline tek sekme ve açık izin sınırları sürer;
[ayrıntılı kabul ve kullanım](docs/VISUAL-AGENT.md).

4.3 tamamlandı: isolated-world sabit DOM işlevleri, main-document/frame/loader/URL
hedef kontrolü, gerçek CDP mouse/key, Unicode value setter ve ayrı dosya izinleri.
Upload private staging/lazy form read; download GUID/progress/completion, 64 KiB
kontrolü, deny restore ve create-new atomik publication. Başarılı host aktarımı
browser oturumunu korur; action/upload/download için ayrı pending ve replay reddi.
7 yeni Rust testi; toplam 426. Format/Clippy/release ve gerçek Chromium SDK/CLI
eylem, binary dosya, stale/disabled hedef, oversize ve in-flight iptal kabulü;
4.2 gezinme/DOM/AX/ekran regresyonu geçti. Canlı model çağrısı 0.
Model/TUI araç entegrasyonu henüz yok. [Kabul ve sınırlar](docs/BROWSER-ACTIONS.md).

4.2 tamamlandı: opt-in Chromium image/spec, PID1 yaşam döngüsü ve sabit
info/navigate/snapshot Rust worker. URL/izin preflight, bounded HTTP/WebSocket
CDP, loader-bound load bekleme, redirect/fragment URL doğruluğu, DOM/AX sınırları
ve stop/pending/replay reddi uygulandı. 13 yeni Rust testi; toplam 419.
Format/Clippy/release ve gerçek Chromium SDK/CLI gezinme, DOM/AX, PNG pikseli,
büyük DOM kırpma, hata sonrası inspect/ack ve owned cleanup geçti. Canlı model
çağrısı 0. Browser ağsız; internet/çoklu sekme, sayfa eylemleri ve model/TUI
görsel entegrasyonu tamamlanmış sayılmaz. [Kabul ve sınırlar](docs/BROWSER.md).

4.1 tamamlandı: açık opt-in masaüstü image/spec, X11/XTEST Rust worker,
ekran PNG'si, koordinat, click/drag/scroll ve key/type. 10 yeni Rust testi;
toplam 406. Format/Clippy/release ve gerçek native GUI piksel/girdi oraklları,
CLI izin/PNG dosyası ve 4 Pal PTY + 9 Docker regresyon kontrolü geçti.
Canlı model çağrısı 0. Browser ve model/TUI görsel araç entegrasyonu bu aşamada
tamamlanmış sayılmaz. [Kabul ve sınırlar](docs/DESKTOP.md).

3.8 tamamlandı: özel durable tool-effect fence, crash owner tespiti ve açık
inspect/ack; generation-bound reconnect ve SDK rebind; kaynak subscription,
coalesced update, unsubscribe ve kopma temizliği. Eski RPC/izin/abonelik otomatik
tekrarlanmaz. CLI recovery/reconnect/watch, bağımsız stdio/HTTP/SSE fixture'ları.
25 yeni Rust testi; toplam 396. Format/Clippy/release ve gerçek CLI kabulü;
canlı model/OAuth kotası 0. [Toparlanma/kabul ve sınırlar](docs/MCP-RECOVERY.md).
MCP bölümü 3.1–3.8'in bu plandaki kapsamı tamamlandı; tüm opsiyonel MCP uzantıları
ve üçüncü taraf sunucular için genel uyumluluk iddiası değildir.

3.7 tamamlandı: protected resource/issuer metadata keşfi, pre-registered client
ile PKCE/S256/state ve loopback callback; özel/atomik credential dosyaları,
resource/client-bound token, rotation/expiry refresh ve uncertain grant replay
reddi. Açık CLI auth login/status/logout, HTTP manager ve normal ajan akışı
bağlandı. 18 yeni Rust testi (5 unit, 12 SDK integration, 1 CLI); toplam 371.
Ayrı Python fixture ile release CLI'de 11 uçtan uca kontrol; format/Clippy/release.
Canlı model/OAuth hesabı çağrısı 0. Otomatik interaktif login/DCR ve remote
revocation yok; uyumluluk ve plaintext OS koruma sınırları
[OAuth belgesinde](docs/MCP-OAUTH.md).

3.6 tamamlandı: SDK Tools adaptörü, açık sunucu seçimi/kararlı alias'lar,
Draft 2020-12 offline input/output instance doğrulaması, bütün tool batch'inin
ilk etkiden önce kontrolü, tool verisi olarak typed içerik, gerçek TUI onayı,
oturum/config pin, Pal alias izin tavanı ve owned guest stdio. 18 yeni test,
toplam 353 Rust testi; fmt/Clippy/release, 6 MCP PTY kontrolü, 25 mevcut TUI ve
4 Pal PTY regresyon kontrolü. Gerçek Docker'da guest sonucu/temizliği ve
belirsiz MCP kaydının resume sırasında yetki vermemesi ve gerçek CLI Pal guest
zinciri doğrulandı.
Canlı model çağrısı 0. [Kullanım ve sınırlar](docs/MCP-AGENT.md).

3.5 tamamlandı: dört katalog türü, atomic/bounded sayfalama, yerel şema metadata
sınırları, typed tool/read/prompt içerikleri, stdio/HTTP bildirim kuyruğu ve
list_changed invalidation, stale katalog grant reddi, izinli CLI discover.
40 yeni test, toplam 335 Rust testi; fmt/Clippy/release ve release keşif kabulü.
Model çağrısı 0. Tam schema instance kontrolü/model içerik dönüşümü 3.6'dadır.
[Kabul ve sınırlar](docs/MCP-DISCOVERY.md).

3.4 tamamlandı: özel kullanıcı profilleri ve server CRUD/CLI, açık proje seçimi,
SHA-256 içerik trust/revoke, user/project/host policy birleşimi, işlem başına
tek kullanımlık snapshot-bound permit ve stdio/HTTP manager. 27 yeni test,
toplam 295 Rust testi; fmt/Clippy/release, gerçek yerel stdio/HTTP ve release CLI
check kabulü. Model çağrısı 0; normal ajan entegrasyonu 3.6'dadır.
[Kabul ve sınırlar](docs/MCP-MANAGEMENT.md).

3.3 tamamlandı: Streamable HTTP JSON/SSE, session ve anlaşılmış sürüm başlıkları,
korunan GET akışı, açık DELETE, bounded framing/kuyruk, deadline/iptal ve
belirsiz araç isteğinde replay reddi. 30 yeni test, toplam 268 Rust testi;
fmt/Clippy/release, gerçek yerel TLS/CONNECT proxy ve release HTTP örneği geçti.
Canlı model çağrısı 0. Legacy 2024 endpoint keşfi yok; OAuth 3.7, reconnect 3.8.
[Kabul ve sınırlar](docs/MCP-HTTP.md).

3.2 tamamlandı: native Rust Unix stdio; açık program/argv/cwd/env; bounded
nonblocking stdin/stdout/stderr; initialization write ack; pipeline yanıtları;
iptal/deadline ve process group EOF/TERM/KILL/reap. Thread-affine lifecycle ve
Linux direct-child parent-death koruması var. 25 yeni test, toplam 238 Rust testi;
fmt/Clippy/release ve gerçek yerel subprocess örneği geçti. Model çağrısı 0.
[Kabul ve sınırlar](docs/MCP-STDIO.md).

3.1 tamamlandı: bağımsız `namzu-mcp`, 2025-11-25 / 2025-06-18 sürüm anlaşması,
strict/bounded JSON-RPC, initialization write ack, request ID/bütçe rezervasyonu,
context deadline/iptali ve replay reddi. 26 yeni test, toplam 213 Rust testi;
workspace format/Clippy/release ve offline protokol örneği geçti. Canlı model ve
MCP sunucusu çağrısı 0. [SDK ve kapsam](docs/MCP.md).
0.2–0.5 tamamlandı. Cihaz onayıyla native Codex hesabı bağlandı; gerçek
katalogda GPT-6-Luna bulundu ve Jcode da aynı hesaba bağlandı. Ortak model ve
low effort ile seri canlı koşum, bağımsız orakllar ve 300 saniye sınırı altında
çalıştırıldı. Sonuçlar ve karşılaştırma sınırları bench/ACCEPTANCE.md'de.
Normal testler hâlâ canlı model çağırmaz. TUI 1.1–1.4 tamamlandı: 103 test,
format, Clippy, release, 25 gerçek Linux PTY kontrolü ve ayrı GPT-6-Luna read-only
canlı görev doğrulandı. Kabul ve sınırlar docs/TUI-ACCEPTANCE.md'de.
Pals 2.1–2.6 ve MCP 3.1–3.8 tamamlandı. Kullanıcı kaldığı yerden devam
edilmesini ve her aşamada ayrı commit/GitHub yüklemesini istedi.

## Kayıp kaynağı tanıma bilgisi

Son eski arşiv: namzu-rs-source.tar.gz; 138 kaynak dosyası; 394421 bayt.
Sohbet kaydındaki SHA-256:
`b994799c95736271078b8491efb37bb60a74cb2b7f087c95ab21a2eec69917ba`.
Eski rapor 297 test ve 9 Docker kontrolü bildiriyordu; bu sonuçlar yeni
uygulamanın doğrulaması olarak kullanılamaz. 3.7 eski projede başlamamıştı.
