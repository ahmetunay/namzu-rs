# İçeriksiz yerel telemetri — 7.1

`namzu-telemetry` bağımsız Rust paketidir: ortak sürümlü olay şeması, turn/provider/
araç span'ları ve sayısal ölçümleri sağlar. Core paketi bu pakete bağımlı değildir.
Varsayılan kapalıdır; açık seçim yalnızca özel yerel state'e kayıt yapar. Exporter,
ağ istemcisi veya global telemetry singleton kurulmaz.

Referans upstream snapshot `a8a751ab157ba5392db107ee6c589575336b1711` içindeki
`packages/telemetry` README/types/metrics incelendi. Upstream OpenTelemetry
collector/OTLP, console exporter, runtime instrument registry ve session export
sunar. Bu teslim yerel typed trace kapsamındadır; OTLP/OpenTelemetry uyumluluğu
veya bu upstream seçeneklerinin tamamının taşındığı iddia edilmez. AG-UI 7.2,
eval ve performans karşılaştırması 7.3–7.4 kapsamındadır.

## Açık seçim ve inceleme

```sh
namzu-rs exec 'Görev' --provider openrouter --model MODEL_ID --telemetry --json
namzu-rs tui --provider codex --model MODEL_ID --telemetry
namzu-rs pal --provider openrouter --model MODEL_ID --telemetry create Reader
namzu-rs pal --provider openrouter --model MODEL_ID --telemetry ask PAL_ID 'Görev'

namzu-rs telemetry list
namzu-rs telemetry show t-64_KUCUK_HEX_HANE
# Farklı özel state seçicisi:
namzu-rs telemetry --state-dir /ozel/state list
```

Config alanı `"telemetry": true` aynı kaydı açar. `--telemetry=false`, config'teki
açık tercihi o çağrı için kapatır; bayrağın değeri `=` ile verilir. Yeni örnek
[`telemetry.config.example.json`](../telemetry.config.example.json) içindedir.
`exec --json` sonucu `telemetry_id` alanı içerir; kapalıysa veya kayıt başlatılamadıysa
`null` olur. Pal/TUI kayıtları `telemetry list` üzerinden source/zaman/trace ID
ile bulunur. Pal create gibi yönetim işlemleri trace oluşturmaz; sonraki gerçek
görev çağrısında da bayrağı/config'i kullan. Tercih model/izin route pin'ine dahil
edilmez; resume'da açıp kapatılabilir ve her turn yeni bir trace alır.

Normal exec, normal TUI ve CLI Pal ask/run/team/mailbox/TUI görevleri aynı
projeksiyonu kullanır. Pal'da her çalıştırma bağımsız trace'tir; ekip/parent-child
trace grafiği bu teslimde yoktur. Demo TUI, auth/models/MCP yönetimi veya geçmiş
session'ların geriye dönük export'u otomatik trace üretmez. SDK'yi kendi kullanan
host, `Observed` veya `Tap` adaptörünü kendisi bağlar.

İnceleme komutları offline'dır: model discovery/login/credential okuma çağrısı
yapmaz. Eksik telemetry dizini listede boş döner ve dizin yaratılmaz. Liste en
fazla 4096 trace gösterir; daha fazla kayıt varsa açık trace ID ile incele.
Kayıtlar otomatik silinmez; global retention/rotasyon/GC bu kapsamda yoktur.

## Şema ve ölçüm anlamları

Şema `schema_version: 1` taşır. Trace ID, runtime'ın ürettiği iç kimliğin SHA-256
özetidir; prompt/session içeriğinin hash'i değildir. Span kimlikleri turn içi
artan sayılardır. Root span `1`, provider ve tool span'larının parent'ı `1`dir.
Sıralı record numaraları ve monotonic başlangıçtan geçen `at_ms` kullanılır.
`started_unix_ms` yalnızca duvar saati metadata'sıdır; süre hesabında kullanılmaz.

Üç güvenli record tipi vardır: `started`, `ended`, `usage`. Span türü turn,
provider veya genel araç kategorisidir. Root outcome `running`, `ok` veya typed
`failed` olur. Tool-error sonucu kendi span'ında `tool_error` olur; model bu
sonuçla devam edip başarılı turn tamamlayabilir. Kapanışı görülmeyen ara span
`interrupted` olarak işaretlenir. Host'un gerçek `AgentError` sonucu root'a
verilir; `TurnFailed.error` string'i kopyalanmaz. Bu yüzden agent sonrası session/
cleanup hatası da root outcome'a yansıyabilir.

Ölçümler:

- Başlatılan/tamamlanan sağlayıcı ve araç sayıları, tool-error adedi.
- Approval hook'undan dönen allow/deny adedi. Bunlar her zaman insan onayı
  değildir: noninteractive host'un mevcut izin politikası da allow döndürebilir.
- Text delta olay sayısı ve UTF-8 byte adedi; metnin kendisi saklanmaz.
- Tamamlanan provider/tool span süreleri: samples, sum_ms ve max_ms.
  Başarısız/yarım sağlayıcı çağrısı successful latency sayacına eklenmez; kendi
  kapanış record'unda süresi ve sonucu bulunur. Tool süresi onay bekleyişinden
  sonra başlar. Root süresi host setup/cleanup'ını da içerebilir; config yükleme
  ve son telemetry publication/fsync süresi bu ölçümün dışındadır. Dosya sistemi
  çekirdek çağrılarının deadline ile kesilmesi garanti edilmez.
- Bildirilen token/ücret için mevcut `UsageTotals` ve kısmi örnek kapsaması.
  Eksik metadata sıfır veya ücretsiz sayılmaz; fiyat tahmin edilmez. Provider
  metadata bildirmiyorsa `AssistantMessage` provider span'ını yine kapatır.
- Düşürülen record ve geçersiz olay sırası/sayaç adedi. Geri giden host saati
  clamp edilir ve sayılır; agent döngüsünü bozmaz.

Her trace en fazla **1024 record / 1 MiB serialize edilmiş JSON** taşır. Delta'lar
record olarak biriktirilmez. Limitte ara record'lar düşürülür, sayısal sayaçlar
saturating olarak işlemeye devam eder; root'un son record'u için yer ayrılır.
Düşürülen record varsa ara span grafiği eksik olabilir; eksiksiz görünümü iddia
edilmez. Sınırsız raw event/argument buffer veya background export thread'i yoktur.

## İçerik ve kimlik bilgisi koruması

Ham `namzu_core::Event` **presentation** içindir; hâlâ görev metni/araç sonucu
barındırır. Telemetriye serialize edilmez. Güvenli şema doğrudan typed numeric/
enum alanlardan kurulur; sonradan regex ile sırları temizleme yöntemi kullanılmaz.

Prompt/yanıt/delta metni, system talimatı, tool ID/adı/argüman/sonucu, dosya yolu,
PNG/base64, private reasoning, model adı, endpoint URL, hesap, session/Pal/task
kimliği, key selector veya credential trace'e alınmaz. Araçlar files/shell/MCP/
audio/LSP/computer/browser/other enum kategorisine çevrilir. Serbest attribute
map'i, arbitrary string error alanı ve içerik hash'i yoktur. Hata sınıfı enum,
HTTP status sayıdır. Boyut/zaman/token/ücret ölçümleri ve çalıştırma zamanı hâlâ
metadata'dır; içerik gizliliği bunların anonimleştirildiği anlamına gelmez.

Trace ve özel session farklı sözleşmelerdir: session dosyası görev geçmişi ve
private replay içerir; telemetry bu dosyayı okuyup export etmez. Trace gizliliği,
normal görev stdout'unun veya session kayıtlarının içeriksiz olduğu iddiası değildir.

## Kalıcılık ve hata davranışı

`$STATE/telemetry/t-<64hex>.json`, owned 0700 child / 0600 dosya kullanır.
PrivateStore'un symlink/hardlink/owner denetimi, atomik yayın ve sync kuralları
korunur. Yeni ID için create-new marker, ilk provider/tool etkisinden önce yazılır.
Normal kapanışta son snapshot atomik değiştirilir; SIGKILL/çökmede Drop flush yoktur.
`running`, **kapanışı doğrulanmamış** demektir; işlemin hâlâ canlı olduğunun
kanıtı değildir. Pending kayıt otomatik replay/recovery izni vermez.

Marker yazılamazsa çalıştırma başlamaz. Son publication hata verirse başarılı
turn bile açık state hatasıyla raporlanır: araçlar zaten çalışmış olabilir.
Oturum/görev gerçek durumu ayrı kalır; telemetry hatası etkileri geri almaz ve
tekrar deneme başlatmaz. Önceden var olan turn hatası, telemetry kayıt hatasıyla
ezilmez; pending marker ve asıl hata incelenir. TUI de hata durumunu gösterir.
Katalog/credential veya tool journal doğrulamasının yerine geçmez. Trace dosyası
kriptografik audit log değildir; OS hesabı sahibi içeriği değiştirebilir.

## SDK ve doğrulama

```sh
cargo run -p namzu-telemetry --example offline --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo build --release --workspace --locked
python3 scripts/telemetry_smoke.py
python3 scripts/provider_conformance.py
python3 scripts/pals_smoke.py --out artifacts/telemetry-pals-regression.json
```

`Recorder` bellekte projeksiyon yapar; `Observed` gerçek Observer checkpoint/
onay/event hatalarını, `Tap` EventSink onay/iptal bağını aynen iletir. `finish`
gerçek typed host sonucunu alır, idempotent kapanır; `LocalTrace` bunu özel yerel
kayda bağlar. `observe_at`/`finish_at` host clock/test adaptörüdür; milisaniye alır.

19 yeni Rust testi; toplam 615. SDK testleri içerik sentinel'ları, bilinmeyen
usage, sayaç/clock/record sınırı, span süreleri, onay/event/checkpoint hatası,
pending/private kayıt ve final publication hatasını sınar. 13 release CLI/PTY
kabul grubu; gerçek dosya oraklı, offline inceleme, config override, resume,
429/no retry, güvensiz dizinde etkisiz ret, süreç ölümü, normal/Pal TUI onay-ret
ve HTTP sırasında Ctrl-C ile typed cancelled trace kontrol edilir. Ayrıca önceki
75 provider/servis/katalog/TUI kabulü ve 4 Pal PTY regresyonu korunur.

Canlı model çağrısı 0. Orijinalle tam özellik/performans eşitliği ölçülmedi.
Sıradaki iş **7.2 — AG-UI olay eşlemesi ve istemci entegrasyonu**.
