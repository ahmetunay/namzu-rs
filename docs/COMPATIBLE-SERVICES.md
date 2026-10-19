# Zen, Zen Go ve OpenRouter — 6.2

Bu teslim, upstream `a8a751ab157ba5392db107ee6c589575336b1711` içindeki
Zen/OpenRouter istemcilerinin OpenAI uyumlu davranışlarını Rust SDK ve
exec/TUI/Pal akışına uygular. Canlı servis erişimi veya bütün upstream
özellikleriyle eşdeğerlik iddiası değildir.

| `--provider` | Varsayılan base URL | Anahtar değişkeni | Wire formatı |
|---|---|---|---|
| `openai-compatible` | `https://api.openai.com/v1` | `OPENAI_API_KEY` | Chat; açık seçimle Responses |
| `openrouter` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | Chat |
| `zen` | `https://opencode.ai/zen/v1` | `OPENCODE_API_KEY` | Model adına göre Chat/Responses |
| `zen-go` | `https://opencode.ai/zen/go/v1` | `OPENCODE_API_KEY` | Model adına göre Chat/Responses |

Model adı açıkça gerekir. Zen'de `zen/`, Zen Go'da `go/` öneki wire'a
gönderilmeden çıkarılır. Auto seçiminde `gpt-*`, `o` + rakam ve `muse-spark`
Responses kullanır; diğer adlar Chat kullanır. `claude-*` ve `gemini-*`
auto seçiminde 6.3 ile native buffered Messages/Google sürücüsünü seçer;
bu modellerde `--api-stream true` reddedilir.
[Native sürücülerin kapsamı](NATIVE-PROVIDERS.md). OpenRouter'da model adı sağlayıcının tam slug'ıdır.
Dinamik model keşfi/bağlam kataloğu 6.4 kapsamındadır.

## CLI ve kalıcı ayar

Anahtarı komut satırına yazma; seçilen ortam değişkenini kendi ortamında ayarla.
Bu komut gerçek bir servis isteği yapar ve hesap/model koşullarına göre
ücret doğurabilir:

```sh
namzu-rs exec 'Projeyi incele' --provider openrouter --model SAGLAYICI/MODEL \
  --api-stream true --effort low --max-output-tokens 2048 --cwd ./proje --json
```

`--api-format auto|chat|responses` (native biçimler 6.3 belgesinde), `--api-stream true|false`, `--effort` ve
`--max-output-tokens` exec/TUI/Pal ortak seçenekleridir. Stream varsayılanı
false'tur. Effort: `none|minimal|low|medium|high|xhigh`; seçilen modelin bunları
desteklemesi gerekir. Bilinmeyen/uygulanamayan seçenekler sessizce yutulmaz.
Codex abonelik sürücüsü API wire/stream/token seçeneklerini kabul etmez;
native login ve SSE yolu ayrı kalır.

[`services.config.example.json`](../services.config.example.json) bir örnektir.
Model yer tutucusunu değiştirip `--config /açık/yol/config.json` ile seç.
Projeden otomatik config yüklenmez. `api_options` alanları strict'tir:
`format`, `stream`, `reasoning_effort`, `max_output_tokens`, `site_url`,
`site_name`; 6.3 ayrıca `thinking`, `aws_region`, `bedrock_auth` alanlarını ilgili
servislere ekler. `site_url` ve `site_name` yalnızca OpenRouter'a
HTTP-Referer/X-Title olarak gönderilir. `base_url` ve `api_key_env` config'de verilmezse seçilen servisin
varsayılanları kullanılır; açık CLI seçicileri bunları değiştirebilir.

## Servis davranışları

- OpenRouter istekleri tek model içerir; `provider.allow_fallbacks=false` ve
  `require_parameters=true` gönderilir. SDK/CLI otomatik retry veya model/
  sağlayıcı fallback yapmaz. Desteklenmeyen parametreye servis hata verebilir.
- Zen/Zen Go API-key ile kullanılır. Namzu'nun başka bir upstream gateway
  veya cihaz kimliği kullandığı akış taklit edilmez. API-key, abonelik ve
  ücretsiz model erişimi birbirinin garantisi değildir.
- Chat SSE parçalı text, tool ID/name/arguments, reasoning ve usage taşır.
  Text delta'ları geçicidir. Geçerli finish ve `[DONE]` gelmeden araçlar
  çalıştırılmaz. Kesik/refused/length yanıt ve bozuk metadata reddedilir.
  Bir choice, 1 MiB satır/olay, 16 MiB toplam akış, 4 MiB final text ve
  en fazla 1024 tool call sınırı vardır. HTTP beklemesi iptal/deadline ile kesilir.
- Responses hem buffered hem SSE destekler; `store:false`, native function
  call/output ve opaque reasoning replay kullanır. GPT/o-family için encrypted
  reasoning istenir; Muse Spark'a bu include gönderilmez. `response.completed`
  veya buffered `status:completed` olmadan sonuç kabul edilmez. Mevcut bounded
  Responses parser'ı Codex ile paylaşılır; auth/endpoints paylaşılmaz.
- Token tavanı Responses'ta `max_output_tokens`; Chat'te `max_tokens`, genel
  OpenAI GPT-5/GPT-6/o-family Chat'te `max_completion_tokens` olur. Model
  kapasitesi keşfedilmez ve istek başka bir modele dönüştürülmez.
- Zen Go DeepSeek Chat geçmişindeki assistant mesajlarında eksik
  `reasoning_content` boş string ile tamamlanır.

## Özel replay ve maliyet

Reasoning/opaque veriler kullanıcı mesajına veya genel metadata'ya eklenmez.
Private ProviderState; servis, endpoint, model, seçenekler ve assistant
imzasına bağlıdır. Aynı rotadaki opaque state farklı anahtarla tekrar
gönderilmez; private state anahtarın SHA-256 etiketini içerir, anahtarı içermez.
Yabancı route/kind state'i wire'a taşınmaz. Normal history/TUI ve CLI sonucu
bu state'i göstermez; private session dosyaları reasoning içerebilir.

Yeni servisler ve seçenekli generic API oturumları servis/base URL/model,
anahtar değişkeninin adı ve bütün API seçeneklerine pin edilir. Resume veya
Pal çağrısında aynı ayarları kullan; seçeneklerin eksilmesi/değişmesi istekten
önce reddedilir. Kalıcı config kullanmak bu tekrarları kolaylaştırır. Eski
varsayılan generic Chat oturum kimliği geriye uyumluluk için korunur.

OpenRouter `usage.cost` USD alanı string veya JSON number olarak okunur.
Orijinal ondalık sayı float'a çevrilmeden integer nano-USD'ye dönüştürülür;
pozitif nano altı kalıntı yukarı yuvarlanır. Örneğin `0.00000000100000000001`
2 nano-USD olur. Negatif/bozuk/taşan değer reddedilir; eksik/null maliyet
bilinmiyor kalır, açık sıfır 0'dır. Global JSON sayı davranışı değiştirilmez.

Raporlanan ücret `stats.usage.reported_cost_nano_usd` ve samples ile saklanır.
SDK'ye açık fiyat verilmişse yalnızca raporlanan ücret yokken tahmin kullanılır;
`estimated_cost_nano_usd` ayrı tutulur. Eksik token/ücret ücretsiz demek değildir.
Başarısız/yarım isteklerin faturası bu sayaçlarla ölçülmez. TUI token sayısını
gösterir; bu teslim yeni bir ödeme/bütçe rezervasyon ekranı eklemez.

## Tekrar üretilebilir kabul

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --workspace --locked
python3 scripts/services_smoke.py
python3 scripts/provider_contract_smoke.py
python3 scripts/tui_smoke.py --binary target/release/namzu-rs
```

Testler yalnızca loopback fixture ve sahte anahtar kullanır. SDK testleri
profil/header/format seçimini, parçalı stream'i, exact cost'u, reasoning replay
ve anahtar değişimini, kesik akışı ve metadata'nın hata/iptalde silinmesini
sınar. Release kabulü gerçek CLI, Pal ve normal/Pal PTY'de stream, araç etkisi,
onay ret/kabul, durable usage ve oturum/profile drift reddini doğrular.
6.2 doğrulamasında 17 yeni Rust testiyle toplam 549 test; fmt, Clippy ve
release derlemesi; 11 servis kabul grubu, 6 provider-contract ve 25 TUI
regresyon kontrolü kullanılır. Resize regresyonu, asenkron SIGWINCH sonrasında
normal frame görünmeden input göndermemek için görünür ekranı bekler.
Canlı ücretli/ücretsiz modelin erişilebilirliği ayrıca denenmiş sayılmaz.
