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
| 2 | Alt ajanlar / Pals | 2.1–2.2 tamam; 2.3–2.6 eksik | Paralel ekip, mesaj işleme, sahip olunan Pal bilgisayarları |
| 3 | MCP | Eksik; yeniden 3.1'den | Stdio/HTTP, güven/izin, entegrasyon, OAuth ve toparlanma |
| 4 | Tarayıcı / bilgisayar kontrolü | Eksik | İzole bilgisayarda tarayıcı ve görsel eylemler |
| 5 | Dosya, ses, LSP ve sandbox paketleri | Temel dosya araçları var | Gelişmiş yetenek paketlerinin SDK/terminal entegrasyonu |
| 6 | Diğer model sağlayıcıları | OpenAI uyumlu HTTP ve Codex sürücüleri var | Ortak sözleşme testleriyle ayrı sürücüler |
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
- **2.3** — Kalıcı Pals: kimlik, çalışma alanı, görev/oturum yaşam döngüsü.
- **2.4** — Mesaj kuyruğu ve otomatik işleme: lease/ack, yeniden başlatma, yinelenen
mesaj kontrolü; belirsiz araç etkisini körlemesine tekrarlamama.
- **2.5** — Pal bilgisayarları: izole container, shell ve dosya aktarımı; durdurma,
zaman aşımı, yeniden başlatma ve kaynak temizliği.
- **2.6** — SDK/CLI/TUI bağlantıları ve uçtan uca kabul testleri.

Kabul: birbirinden izole Pals, daraltılan izinler, deterministik paralel
sonuçlar, çökme/iptal ve gerçek container temizliği. Görsel yetenekler 4'te.

## 3. MCP — sekiz küçük teslim

Yeni depoda tüm MCP parçaları eksik. Eski projede 3.6'ya kadar ilerlenmişti;
3.7 başlamamıştı. Aşağıdaki kapsam önceki ilerleme kaydından yeniden düzenlendi.

| Parça | Kapsam | Kabul ölçütü |
| --- | --- | --- |
| 3.1 | Protokol ve veri tipleri | JSON-RPC, sürüm/initialize, ID eşleştirme, sınırlar, hata/iptal |
| 3.2 | Stdio taşıma | Kontrollü env/cwd, shell'siz başlatma, sınırlı stderr, süreç ağacı/temiz kapatma |
| 3.3 | HTTP taşıma | Streamable HTTP/uyumluluk, SSE, session, deadline, TLS/proxy; belirsiz etkiyi tekrarlamama |
| 3.4 | Config, yönetim ve güven | Server manager, profil/öncelikler, proje güveni, allow/deny/onay, CLI yönetimi |
| 3.5 | Keşif ve sunucu sağlayıcıları | Tools/resources/prompts/templates, sayfalama, bildirimler, isim/şema sınırları |
| 3.6 | SDK/CLI/TUI/Pals entegrasyonu | Araç/içerik aktarımı, çocuk izin tavanı, guest stdio; sessiz host fallback yok |
| 3.7 | OAuth ve kimlik bilgileri | Metadata discovery, PKCE/state/callback, token saklama/refresh; otomatik etkileşimli login yok |
| 3.8 | Toparlanma ve birlikte çalışabilirlik | Kopma/reconnect, subscription yaşam döngüsü, farklı fixture sunucuları |

Kabul: yerel stdio/HTTP/OAuth fixture'ları; yanlış ID/sürüm/sayfa, izinsiz araç,
çocuk yetki yükseltme ve iptal sonrası etki kontrolleri. Normal testler canlı
model/hesap kotası tüketmez. Her 3.x ayrı commit ve GitHub yedeğidir.

## 4. Tarayıcı / bilgisayar kontrolü

- **4.1** — 2.5'teki yaşam döngüsünü kullan: ekran/screenshot, koordinatlar, klavye/fare.
- **4.2** — Tarayıcı başlatma, sayfa gezinme, DOM/erişilebilirlik ağacı ve ekran içeriği.
- **4.3** — Sayfa eylemleri, indirme/yükleme ve dosya aktarımı; izinler ve iptal.
- **4.4** — Pal/SDK/TUI entegrasyonu; yerel test sitesiyle görev ve temizlik doğrulaması.

## 5. Dosya, ses, LSP ve sandbox paketleri

- **5.1** — Gelişmiş dosya/içerik: büyük dosya, yapılandırılmış belge, görsel/ek işleme;
basit dosya ve patch araçlarının üzerine kurulur.
- **5.2** — Ses giriş/çıkış sürücüleri, format/süre/boyut sınırları ve SDK olayları.
- **5.3** — LSP yaşam döngüsü, diagnostics, tanım/referans ve düzenleme uygulama.
- **5.4** — Sandbox politikaları, dosya/süreç/ağ sınırları ve farklı çalışma ortamları;
2.5'teki temel container altyapısını genişletme.
- **5.5** — Yetenek kaydı, config/izin ve terminal entegrasyonu.

Kabul: dosya/format sınırları, gerçek yerel LSP fixture'ı, izinsiz erişimin reddi.
Yerel testler için ücretli ses hizmeti zorunlu değildir.

## 6. Diğer model sağlayıcıları

- **6.1** — Ortak sürücü sözleşmesi: mesaj/araç şeması, akış, kullanım/ücret ve hatalar.
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

**Sıradaki iş: 2.3 — kalıcı Pal kimliği, workspace ve görev/oturum yaşam döngüsü.**
0.2–0.5 tamamlandı. Cihaz onayıyla native Codex hesabı bağlandı; gerçek
katalogda GPT-6-Luna bulundu ve Jcode da aynı hesaba bağlandı. Ortak model ve
low effort ile seri canlı koşum, bağımsız orakllar ve 300 saniye sınırı altında
çalıştırıldı. Sonuçlar ve karşılaştırma sınırları bench/ACCEPTANCE.md'de.
Normal testler hâlâ canlı model çağırmaz. TUI 1.1–1.4 tamamlandı: 103 test,
format, Clippy, release, 25 gerçek Linux PTY kontrolü ve ayrı GPT-6-Luna read-only
canlı görev doğrulandı. Kabul ve sınırlar docs/TUI-ACCEPTANCE.md'de.
Pals 2.1–2.2 tamamlandı; 2.3–2.6 ve MCP eksik. Kullanıcı 2.1, 2.2 ve 2.3 için
sıralı çalışma ve her adımda ayrı commit/GitHub yüklemesi istedi.

## Kayıp kaynağı tanıma bilgisi

Son eski arşiv: namzu-rs-source.tar.gz; 138 kaynak dosyası; 394421 bayt.
Sohbet kaydındaki SHA-256:
`b994799c95736271078b8491efb37bb60a74cb2b7f087c95ab21a2eec69917ba`.
Eski rapor 297 test ve 9 Docker kontrolü bildiriyordu; bu sonuçlar yeni
uygulamanın doğrulaması olarak kullanılamaz. 3.7 eski projede başlamamıştı.
