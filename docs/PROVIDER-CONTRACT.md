# Ortak sağlayıcı sözleşmesi — 6.1

Sözleşme `namzu_core::provider` içindedir; core ağ/sağlayıcı paketlerine bağımlı
olmaz. `Provider` trait'i yeni default metotlarla genişletilmiştir:

- `info() -> ProviderInfo`: `contract_version=1`, provider slug, seçilen model
  ve transport yetenekleri. Varsayılan custom descriptor eski SDK sürücülerini
  korur; `Unknown` yetenek bir model/hesap erişim garantisi değildir.
- `complete` ve `complete_stream`: mevcut ortak Message / ToolDefinition /
  Completion tipleri; final yanıt başarılı dönmeden tool etkisi başlatılmaz.
- `take_metadata() -> Option<ResponseMetadata>`: son başarılı completion'a ait
  tek kullanımlık usage/cost; yeni denemeden önce eski metadata silinir.
- `take_state()`: ayrı opaque replay; metadata'dan bağımsızdır ve kullanıcıya
  system instruction olarak taşınmaz. Codex replay aynı route/signature ister.

| Sürücü | Akış | Araç / PNG wire desteği | Opaque replay | Usage |
|---|---|---|---|---|
| OpenAI-compatible / Zen / OpenRouter | buffered veya opt-in SSE | var | route + credential bağlı; yabancı state gönderilmez | Chat/Responses usage varsa |
| Native Codex | incremental SSE | var | route bağlı | completed Responses usage varsa |
| ScriptedProvider | buffered fixture | SDK tipleri | yok | raporlanmaz |

Bunlar **transport** yetenekleridir. Seçilen hesap/modelin tool/vision desteği,
kota veya faturalama koşulları keşfedilmiş sayılmaz. 6.2 API sürücüsüne
opt-in Chat/Responses SSE ve servis profillerini ekler; varsayılan API
`stream:false` kalır. [Servis davranışları](COMPATIBLE-SERVICES.md).
SSE delta'ları geçicidir: tamamlanmış final response tek doğruluk
kaynağıdır. Kesik, incomplete, refused veya bozuk response tool etkisi üretemez.

## Mesaj, araç ve byte sınırları

`validate_request` tüm sürücüler için ortak shape/budget kontrolüdür. Core bunu
provider çağrısından önce, mevcut iki HTTP sürücüsü de doğrudan SDK kullanımında
transport/auth etkisinden önce çalıştırır. Ayrı `route_identity/models/auth`
operatör işlemleri bu completion sözleşmesinden bağımsızdır.

- En fazla 16384 message, 1024 tool definition ve encoded SDK request için
  4 MiB; JSON writer bütçeyi aşınca durur. Sessiz truncation yok.
- Tool isimleri 1–128 ASCII harf/rakam/`_.-`; duplicate definition reddedilir.
  Description en fazla 64 KiB, JSON Schema kökü object olmalıdır.
- `validate_completion`: en fazla 1024 call; dolu, 256 byte bounded ve tekil
  call ID, object arguments ve geçerli isim; final text en fazla 4 MiB.
- PNG sınırları/byte doğrulaması 5.1'den devralınır. Sürücü açıkça image/tool
  desteklemiyorsa istek gönderilmez. Unknown descriptor eski custom SDK
  sürücüsüyle uyumu korur; yeni sürücüler açık yetenek bildirmelidir.

Bu validator tam JSON Schema instance motoru değildir. Tool admission,
argüman instance kontrolü, geçmiş tool ID eşlemesi, cancellation/budgets ve
bütün batch'in ilk etkiden önce doğrulanması mevcut core/Tools katmanında
kalır; MCP kendi tam schema doğrulamasını uygular.

## Kullanım ve maliyet

`TokenUsage` alanları `Option<u64>`: input, output, total, cached_input ve
reasoning. Eksik/null usage bilinmiyordur; **0 token veya ücretsiz** sayılmaz.
Negative/fractional counters, cached > input, reasoning > output veya mevcut
input+output ile tutarsız total reddedilir. Cached input, input toplamının;
reasoning, output toplamının içindedir; tekrar eklenmez.

Chat `prompt_tokens` / `completion_tokens` ve Responses `input_tokens` /
`output_tokens` ortak alanlara normalize edilir. Usage details'in cache ve
reasoning sayıları da desteklenir. Subscription token kullanımı API faturası
olarak fiyatlandırılmaz.

`Stats.usage` raporlanmış toplamları ve kapsama sayısını taşır:

```json
{
  "responses": 3,
  "input_tokens": { "sum": 30, "samples": 2 },
  "output_tokens": { "sum": 6, "samples": 2 },
  "reported_cost_nano_usd": null,
  "estimated_cost_nano_usd": null
}
```

Bu örnekte üçüncü yanıtın tokenları bilinmiyor; sum turn'ün eksiksiz toplamı
olarak yorumlanmaz. Hiç raporlanmış ölçüm yoksa usage alanı JSON'dan çıkarılır;
eski session JSON'u default ile okunur. Stats turn bazlıdır; resume yeni turn
sayacını başlatır, Session.stats en son turn'ü saklar. Başarısız/yarım isteklerin
ücretini bu sayılar ölçemez. Genel provider_calls önceki gibi başarılı
completion sayısıdır; bütün HTTP denemelerinin faturası değildir.

`Cost` fixed-point USD kullanır: 1 USD = 1_000_000_000 nano_usd. `Reported`
ve `Estimated` ayrı sayaçlarda tutulur. Float hesap ve tahmini otomatik canlı
fiyat/model kataloğu yoktur. `Pricing::estimate` yalnızca gereken token
bileşenleri biliniyorsa değer üretir; overflow kontrollüdür. Cache için ayrı
fiyat seçilmiş ama cache sayısı bilinmiyorsa tahmin de bilinmiyor kalır.

SDK'de API sürücüsüne açık `.with_pricing(Pricing { ... })` verilebilir. Caller
bu fiyatın hangi model/sürüm için geçerli olduğunu kendisi doğrular. Varsayılan
API/Codex sürücüleri gerçek ücret uydurmaz; 6.2 OpenRouter `usage.cost` alanını
exact ondalık olarak okur ve pozitif nano altı kalıntıyı yukarı yuvarlar.
Eksik ücret bilinmiyor kalır; diğer billing alanları sonraki sürücü işlerindedir. Bu bir ödeme veya bütçe rezervasyon sistemi
değildir; ücret güvenliği yalnızca bu tahmine dayandırılmamalıdır.

Başarılı metadata doğrulanıp Stats'a atomik katılır, `ProviderFinished` olayı
ve durable checkpoint üzerinden host'a aktarılır. Event prompt, key, HTTP body
ve opaque reasoning içermez. TUI raporlanmış input/output tokenlarını gösterir;
`?` bilinmiyor, `+` kısmi kapsama demektir. Normal CLI ve Pal result.stats aynı
Stats sözleşmesini kullanır.

## Typed hatalar ve tekrar davranışı

`AgentError::Remote(ProviderFailure)` sabit şemadır: FailureKind, isteğe bağlı
HTTP status ve Retry-After saniye sayısı. 401 authentication, 403 permission,
404 not_found, 429 rate_limited, 5xx unavailable; kalan non-success HTTP
invalid_request olarak sınıflanır. Transport bağlantı/gövde hataları transport;
Codex incomplete/failed SSE incomplete'dir. Deadline ve cancellation ayrı
AgentError olmaya devam eder; malformed wire/schema Protocol'tür.

Retry-After yalnızca 0–86400 arası integer delta-seconds ise aktarılır;
HTTP-date veya geçersiz/uzun değer okunabilir raw string olarak taşınmaz.
HTTP hata gövdesi okunmaz veya loglanmaz. Uzak URL, credential/header değerleri
ve sunucunun serbest error mesajları typed hata içine eklenmez.

Exec JSON `error` insan mesajını korur ve `provider_error` nesnesini ekler;
Pal görev hatası da bu nesneyi taşır. Yerel/protocol hata için değer null'dır.
Bu sınıflandırma **otomatik retry, login, endpoint/model değişimi veya ücretli
fallback başlatmaz**. Ortak SDK bunu host politikasına bırakır; bu sürümde
mevcut driver davranışı tek denemedir.

## Upstream envanteri ve sonraki aşamalar

İncelenen yerel upstream snapshot: `cogitave/namzu`, commit
`a8a751ab157ba5392db107ee6c589575336b1711`; son remote HEAD olduğu iddia edilmez.
Kaynak: `packages/providers/*/package.json` (10 paket).

| Upstream paket | Rust planı |
|---|---|
| `@namzu/openai` | mevcut API + native Codex; 6.2 Chat/Responses/SSE |
| `@namzu/openrouter` | 6.2 tamam; API-key Chat/SSE ve raporlanan maliyet |
| `@namzu/zen` | 6.2 Chat/Responses; 6.3 buffered native Messages/Google |
| `@namzu/anthropic` | 6.3 buffered native Messages |
| `@namzu/google` | 6.3 API-key buffered GenerateContent |
| `@namzu/deepseek` | 6.3 Chat/SSE ve explicit thinking.type |
| `@namzu/bedrock` | 6.3 buffered Converse, bearer/static-session SigV4 |
| `@namzu/lmstudio` | 6.3 OpenAI uyumlu HTTP; WebSocket SDK uygulanmadı |
| `@namzu/ollama` | 6.3 buffered native Chat; thinking kapalı |
| `@namzu/http` | ortak HTTP yardımcı paketi; ayrı model sağlayıcısı sayılmaz |

6.3 sürücülerinin sınırları ve uygulanmamış upstream opsiyonları
[Native sağlayıcılar](NATIVE-PROVIDERS.md) belgesindedir. Native sürücüler
Buffered bildirir; streaming isteği reddedilir.
[6.4 model keşfi ve limit seçimi](MODEL-CATALOG.md) metadata/açık katalog
kapsamında tamamlandı; otomatik giriş token sayımı eklenmedi. Bütün yeni
sürücülerde kapsamlı ortak conformance matrisi ve açık istekle canlı smoke
6.5'tedir. Yeni provider/model
için canlı API erişimi bu teslimde kanıtlanmış sayılmaz.

## Deterministik doğrulama

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo build --release --workspace --bins --examples --locked
python3 scripts/provider_contract_smoke.py
```

Yeni testler typed hata, partial/unknown usage, fixed-point/cache/overflow,
one-shot metadata reset, wire sınırları, gerçek loopback HTTP ve Codex SSE
normalizasyonunu sınar. Kabul script'i release exec/Pal, disk oraklı, failed
session, usage/cost unknown, tek HTTP denemesi ve gerçek Linux PTY token
sunumu/terminal restore kullanır. Fixture verileri deterministiktir; canlı
model/hesap, gerçek ücret veya sağlayıcı performansı ölçülmez.

2026-10-19 bu workspace kabulü: **16 yeni test, toplam 532 Rust testi** geçti.
Workspace format/Clippy `-D warnings` ve release; **6 gerçek release CLI/Pal/PTY
kabul grubu** ve **25 mevcut TUI regresyon kontrolü** geçti. Native Codex HTTP/SSE
fixture'ı ortak Stats içindeki kısmi kullanım raporunu da doğruladı. Canlı model
ve gerçek sağlayıcı hesabı çağrısı 0. Yerel kanıtlar `artifacts/provider-contract-*`
dosyalarındadır; artifact/credential/session verileri yayımlanmaz.
