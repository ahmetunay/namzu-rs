# Diğer sağlayıcı sürücüleri — 6.3

Upstream `cogitave/namzu` snapshot `a8a751ab157ba5392db107ee6c589575336b1711`
içindeki kalan altı sağlayıcı paketi incelendi. Bu teslim, aşağıdaki sınırlı
transport sözleşmesini Rust SDK ve exec/TUI/Pal'a uygular; upstream'in bütün
opsiyonlarının veya canlı hesap/model erişiminin eşdeğeri değildir.

| Provider | Varsayılan base URL | Anahtar değişkeni | Transport |
|---|---|---|---|
| `anthropic` | `https://api.anthropic.com/v1` | `ANTHROPIC_API_KEY` | `/messages`, buffered JSON |
| `google` | `https://generativelanguage.googleapis.com/v1beta` | `GOOGLE_API_KEY` | `/models/ID:generateContent`, buffered JSON |
| `deepseek` | `https://api.deepseek.com` | `DEEPSEEK_API_KEY` | Chat Completions, buffered veya SSE |
| `bedrock` | `https://bedrock-runtime.us-east-1.amazonaws.com` | `AWS_BEARER_TOKEN_BEDROCK` | `/model/ID/converse`, buffered JSON |
| `lmstudio` | `http://localhost:1234/v1` | `LMSTUDIO_API_KEY`, isteğe bağlı | OpenAI uyumlu HTTP Chat, buffered veya SSE |
| `ollama` | `http://localhost:11434` | `OLLAMA_API_KEY`, isteğe bağlı | `/api/chat`, buffered JSON |

LM Studio burada upstream'in WebSocket SDK'sı üzerinden çalışmaz: LM Studio'nun
OpenAI uyumlu HTTP sunucusu seçilir. Ollama kendi native JSON biçimini kullanır.
Yerel modellerin yüklü olması gerekir; otomatik indirme, başlatma veya host env
keşfi yapılmaz. Açık `--base-url` kullanılır; HTTP yalnızca loopback'te kabul
edilir. Proxy/CA/TLS doğrulaması korunur, yönlendirme/retry/fallback yoktur.

## Açık seçim ve ayarlar

Model adı zorunludur; otomatik model keşfi ve model başına kapasite kataloğu
6.4'tedir. Anahtarı komut satırına koyma; seçilen ortam değişkenini kendi
ortamında ayarla. Aşağıdaki komut gerçek hizmet isteği yapar:

```sh
namzu-rs exec 'Projeyi incele' --provider anthropic --model MODEL_ID \
  --cwd ./proje --max-output-tokens 4096 --json
namzu-rs tui --provider google --model MODEL_ID
namzu-rs pal --provider ollama --model YUKLU_MODEL create Reader
```

[`native.config.example.json`](../native.config.example.json) bir başlangıç
ayar dosyasıdır. Açık `--config /dosya/yolu.json` ile seçilir. Yerel iki serviste
anahtar değişkeni tanımlı değilse Authorization başlığı gönderilmez. Değişken
bilerek tanımlanmışsa seçilen anahtar kullanılır; sahte anahtar üretilmez.

`api_options.format` ve `--api-format` değerlerine `messages`, `google`,
`converse`, `ollama` eklendi. Varsayılan `auto`, servise uygun biçimi seçer;
uyuşmayan açık format reddedilir. `--api-stream true` native dört sürücüde
istek gönderilmeden reddedilir. DeepSeek/LM Studio, 6.2'nin bounded Chat SSE
parser'ını kullanır. Native sürücülerin descriptor'ı `Buffered` bildirir;
TUI tamamlanan JSON yanıtını gösterir.

`--max-output-tokens` 1–1_000_000 arasındadır; servisin model limitini keşfetmez.
Native sürücülerde verilmezse 4096 gönderilir: Anthropic `max_tokens`, Google
`generationConfig.maxOutputTokens`, Bedrock `inferenceConfig.maxTokens`,
Ollama `options.num_predict`. Ollama'ya `think:false` gönderilir.

DeepSeek'in Chat ucu effort alanını taşımadığı için `--effort` reddedilir.
`--thinking adaptive|enabled|disabled` veya `api_options.thinking` gerçek
`thinking.type` alanını gönderir. Diğer yeni sürücülerde reasoning effort ve
thinking isteği desteklenmez; sessizce yok sayılmaz. Response'un kendisinin
ürettiği private reasoning/signature verisi aşağıdaki replay yoluyla korunur.

Zen/Zen Go `claude-*` ve `gemini-*` auto seçimleri de native Messages/Google'a
bağlanır. `zen/`, `go/`, Google'da `models/` öneki normalize edilir. Native
Zen istekleri upstream'deki gibi `x-api-key` / `x-goog-api-key` kullanır;
Chat/Responses bearer yolu değişmez. Bu modellerin Zen Go'da gerçekten
sunulduğu veya bir hesapta erişilebilir olduğu keşfedilmiş sayılmaz.

## AWS Bedrock kimlik seçimi

Varsayılan auth, caller'ın açık seçtiği bearer API anahtarıdır. SigV4 açıkça
seçilir; `--bedrock-auth sigv4 --aws-region us-east-1` veya config karşılıkları
`api_options.bedrock_auth` / `aws_region` gerekir. Region yalnızca SigV4 içindir.

```sh
namzu-rs exec 'Projeyi incele' --provider bedrock --model MODEL_ID \
  --bedrock-auth sigv4 --aws-region us-east-1 \
  --api-key-env AWS_ACCESS_KEY_ID --cwd ./proje --json
```

SigV4'te `api_key_env` access-key ID seçicisidir; varsayılan bearer seçicisi
kullanılmışsa CLI bunu `AWS_ACCESS_KEY_ID` olarak çözer. Secret yalnızca
`AWS_SECRET_ACCESS_KEY`, isteğe bağlı session token `AWS_SESSION_TOKEN` üzerinden
okunur. Profil/SSO/IMDS/SDK credential chain veya otomatik refresh yoktur.
Başka region için o region'ın endpoint'ini `--base-url` ile de seç; AWS host'u
ile imzalama region'ı farklıysa factory reddeder. Custom/loopback endpoint açık
seçimdir. Model ID/ARN tek URL segmenti olarak encode edilir; query yoktur.

SDK mevcut `OpenAiCompatible::for_service(Service::..., base, model, key,
Options { ... })` girişini korur. SigV4 için `aws::Credentials::new(...)` ve
`OpenAiCompatible::for_bedrock(...)` gerekir. HMAC-SHA256 OpenSSL ile hesaplanır;
host, payload hash, UTC tarih ve varsa session token imzalanır. Credentials
Debug çıktısı redaktedir; Authorization/session header'ları sensitive işaretlidir.

## Mesajlar, kullanım ve hata sınırları

- System talimatı native system alanına; user/assistant/tool ve PNG görseli
  ilgili native bloklara çevrilir. Ardışık aynı roller sırayla birleştirilir.
  Araç sonuçları isim/ID ile önceki çağrıya bağlıdır; error sonuçları ayrı işaretlenir.
  Google'ın native call ID vermediği durumda SDK ID üretir, native result'a
  uydurma bir ID göndermez. PNG, kullanıcı kaynaklı güvenilmeyen görsel veridir.
- Anthropic thinking/redacted-thinking, Google thoughtSignature ve Bedrock
  reasoningContent private ProviderState olarak saklanır. Completion/TUI'de
  reasoning gösterilmez. Replay servis/endpoint/model/options, assistant
  imzası ve credential hash'ine bağlıdır. Yabancı state wire'a gönderilmez;
  aynı rotada anahtar değişmişse private replay reddedilir. Ollama'nın native
  thinking replay'i desteklenmez; descriptor bunu `Unsupported` bildirir.
- Ortak SDK request sınırı 4 MiB, native encoded wire 8 MiB, response ve
  serialized replay ayrı ayrı 4 MiB, native blok sayısı en fazla 4096'dır.
  Completion/tool batch, kullanım ve replay limitleri ilk tool etkisinden önce
  doğrulanır. Kesik/incomplete/refused/bozuk yanıt başarıya çevrilmez.
  HTTP gövdesi/başlığı beklemesi ortak iptal/deadline transport'unu kullanır.
- Anthropic input'a raporlanan cache read/write eklenir; cached read input'un
  içinde kalır. Bedrock cache sayaçlarının `Tokens` ve `TokenCount` adları
  desteklenir; cache input'a katılır, total ham base+output veya normalize
  toplamla uyuşmalıdır. Google output'a thoughts eklenir; reasoning ayrıca
  breakdown olarak saklanır. Ollama prompt/eval sayaçları normalize edilir.
  Eksik sayaç bilinmiyor kalır; bilinen bileşenlerden türetilen toplam taşma
  kontrolüyle hesaplanır. Cache-write için ayrı ortak breakdown alanı yoktur.
- Bu altı sürücü gerçek ücret uydurmaz. SDK'de açık `with_pricing` tahmini,
  metadata yeterliyse `Estimated` olarak hesaplanabilir. Local server ücretsiz
  sayılmaz. HTTP hataları ortak typed/redakte sözleşmeyi kullanır; sunucu hata
  metni loglanmaz. Başarısız isteğin olası faturası stats ile ölçülmez.

## Kabul ve kalan upstream farkları

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --workspace --locked
python3 scripts/native_providers_smoke.py
python3 scripts/services_smoke.py
python3 scripts/provider_contract_smoke.py
python3 scripts/tui_smoke.py --binary target/release/namzu-rs
```

19 yeni Rust testiyle toplam 568 test; 15 yeni release CLI/Pal/PTY kabul grubu,
11 servis, 6 provider-contract ve 25 mevcut TUI regresyon kontrolü geçti.
SDK ve bağımsız Python fixture'ı gerçek wire/header/path/tool/usage/replay,
kesik finish, onay ret/kabul ve key/profile drift reddini sınar. SigV4 sabit
vektörü, RFC 4231 HMAC ve gerçek CLI'nin ARN path/payload/header imzası kontrol
edilir. Testler loopback ve sahte anahtarlarla çalışır; canlı model çağrısı 0.

Native SSE/Bedrock EventStream, Google OAuth/Vertex, Anthropic OAuth, AWS
profile/SSO/IMDS, LM Studio WebSocket, hosted search, documents/citations,
structured-output seçenekleri, prompt-cache marker ayarları ve bütün upstream
opsiyonları bu teslimde uygulanmadı. Bunlar karşılaştırma için açık farklardır;
6.4 model keşfi, 6.5 ortak conformance/canlı smoke planı bu kapsamı tamamlanmış
özellikler gibi göstermemelidir.
