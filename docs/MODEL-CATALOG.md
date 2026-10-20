# Model keşfi ve limit seçimi — 6.4

Katalog yalnızca açık `models` komutuyla ağdan alınır. Normal exec/TUI/Pal
isteği model listesi aramaz, yeni katalog indirmez ve başka modele geçmez.
Listeleme model üretimi çağrısı değildir; yine de listeye girmek hesap erişimi,
kota, ücretsiz kullanım veya seçilen modelin her özelliği için garanti değildir.

Upstream incelemesi önceki snapshot `a8a751ab157ba5392db107ee6c589575336b1711`
üzerinden yapıldı. Oradaki bazı statik katalog/fallback tahminleri burada
kullanılmaz: sunucunun bildirmediği limit/yetenek `null` olarak kalır.

## CLI kullanımı

```sh
# Mevcut davranış korunur: parametresiz models, native Codex hesabını listeler.
namzu-rs models

# Açık servis, kendi varsayılan endpoint/key-env seçicisini kullanır.
namzu-rs models --provider openrouter --save
namzu-rs models --provider google --model MODEL_ID
namzu-rs models --provider ollama --save

# Çıktıdaki c- ile başlayan 64 hex haneli kimliği açıkça seç.
namzu-rs models --cached KATALOG_ID
namzu-rs exec 'Projeyi incele' --provider openrouter --model MODEL_ID \
  --model-catalog KATALOG_ID --cwd ./proje --json
namzu-rs tui --provider openrouter --model MODEL_ID --model-catalog KATALOG_ID
namzu-rs pal --provider openrouter --model MODEL_ID --model-catalog KATALOG_ID create Reader
```

Yeni listeleme çıktısı `catalog`, isteğe bağlı `catalog_id`, `offline` ve
`status` içerir. Eski parametresiz Codex çıktısı korunur. `--model` listelemeyi
tam ID/alias ile süzer; ilk sıradaki model seçilmez. `--save` bütün doğrulanmış
kataloğu kaydeder; filtrede model bulunamazsa kayıt yapılmaz. `--cached` credential
okumadan/ağa çıkmadan özel kaydı inceler; endpoint/provider/key gibi ağ
seçicileriyle birlikte kullanılamaz.

Katalog kullanmak isteğe bağlıdır. Önceki `--model` çağrıları yeni keşif isteği
veya kayıt gerektirmeden çalışır. Config'e `model_catalog` eklenebilir;
[`catalog.config.example.json`](../catalog.config.example.json) bir örnektir.
Kimliği `models --save` çıktısından, modeli gerçek listedeki ID'den seç.
Anahtarlar komut satırına veya örnek config'e yazılmaz.

## Sağlayıcı uçları ve sayfalama

| Provider | Açık keşif isteği | Limit bilgisi |
|---|---|---|
| OpenAI-compatible / OpenRouter / DeepSeek / Zen / Zen Go / LM Studio | seçilen base altında `GET /models` | Bildirilen context/output alanları; eksikse bilinmiyor |
| Anthropic | `GET /models?limit=1000`, `after_id` | Bildirilen max input/max output; statik 200k tahmini yok |
| Google | `GET /models?pageSize=1000`, `pageToken` | inputTokenLimit ve outputTokenLimit ayrı tutulur |
| Ollama | `GET /api/tags` | Bu uç çoğunlukla limit bildirmez; ayrıca show/metadata isteği yapılmaz |
| Bedrock | `GET /foundation-models` | Model ID/ARN alias ve bildirilen modaliteler; statik context tahmini yok |
| Native Codex | mevcut sabit authenticated `/models?client_version=1.0.0` | Bildirilen context/output/modalite alanları |

Anthropic ve Google sayfaları tek atomik katalog oluşturur. Cursor, aynı host ve
sabit path'e query parametresi olarak encode edilir; sunucudan gelen başka bir
URL izlenmez. Google açıkça `generateContent` desteklemediğini bildiren modelleri
(ör. yalnızca embedding) dışarıda bırakır. Native Codex hidden modelleri dışlar.
Bedrock foundation listesi, inference profile/account erişim keşfi değildir;
ARN yalnızca model kaydında bildirilmişse alias olarak seçilebilir.

AWS'nin bilinen `bedrock-runtime.REGION.amazonaws.com` host'unda keşif için
aynı region'ın `bedrock.REGION.amazonaws.com` control-plane host'u kullanılır.
Custom/loopback base host değiştirilmez. Bearer veya açık SigV4 seçimi 6.3'teki
gibidir; GET boş payload hash'iyle imzalanır. ListFoundationModels yetkisi ayrıca
gerekebilir. Credential chain/SSO/IMDS eklenmedi.

Listeleme kendi auth başlığını kullanır: Anthropic x-api-key ve version,
Google x-goog-api-key, Zen/Zen Go bearer; yerel anahtarsız servislerde sahte
Authorization yoktur. Generation'ın native/SSE ayarları keşif isteğine
aktarılmaz; auth/region seçimi korunur. Redirect/retry, başarısız listelemede
eski veya statik bir listeye otomatik dönüş yoktur.

Sınırlar: 4096 model/katalog, sayfada en fazla 4096 satır, 32 sayfa, sayfada
4 MiB ve toplam 16 MiB HTTP gövdesi; ID/alias 256 byte, isim 512 byte, cursor
2048 byte. Tekrarlanan ID/alias, cursor döngüsü, bozuk/negatif/sıfır/aşırı limit
ve eksik sayfa reddedilir. Sonraki sayfa başarısızsa önceki sayfalar yayımlanmaz.
Desteklenmeyen bir katalog pagination biçimi sessizce kısmi listeye çevrilmez.

## Limitlerin anlamı ve uygulama sınırı

Her modelde aşağıdaki alanlar ayrı ayrı `Option` değerleridir:

- `context_window`: bildirilen toplam bağlam kapasitesi.
- `input_token_limit`: bildirilen giriş sınırı. Google inputTokenLimit toplam
  context olarak yeniden adlandırılmaz.
- `output_token_limit`: bildirilen üretim sınırı.
- `tools` / `images`: bildirilen model yeteneği; eksik veri bilinmiyordur.

Birden fazla context/output sınırı raporlanıyorsa daha dar değer kullanılır.
Metadata bir token sayacı değildir. Modelin bilinmeyen tavanı 0 veya sınırsız
olarak ilan edilmez. **Bu teslim otomatik giriş token sayımı eklemez.**
Mevcut 4 MiB konuşma/opaque/görsel bütçesi byte sınırıdır; token context'e
sığma garantisi değildir. Modele özgü tokenizer, görüntü token maliyeti ve
otomatik sıkıştırma uygulanmadı.

Katalog seçildiğinde API profilleri şu korumaları uygular:

- Modelin tam ID/normalize ID/alias ile bulunması gerekir. Seçili model yoksa
  inference yapılmaz; erişilebilir veya ücretli başka model seçilmez.
- Açık çıktı bütçesi bilinen output/context tavanını aşamaz. Açık bütçe yoksa
  4096 ile bilinen output/context tavanlarının en küçüğü gönderilir. Bu değer
  toplam giriş+çıkışın bağlama sığacağını tek başına kanıtlamaz.
- Bilinen `tools:false` / `images:false`, sağlayıcı descriptor'ını daraltır;
  ortak request validator desteklenmeyen mesaj/araç isteğini HTTP'den önce reddeder.
  Bilinmeyen model yetenekleri transport'un var olan desteğini değiştirmez.

Codex abonelik transport'u orijinal parametre sözleşmesini korur; bu teslim
ona `max_output_tokens` göndermeye başlamaz. Codex'te katalog seçimi model,
hesap, snapshot ve bilinen araç/görsel yeteneklerini doğrular; limitler metadata
olarak sunulur. API'ye özgü çıktı bayrağı Codex'te hâlâ reddedilir.

SDK'de `catalog::Model::validate_tokens(Some(measured_input), requested_output)`
ölçülmüş giriş sayısı verildiğinde input/output/combined context ve taşmayı
kontrol eder. Giriş sayacı host'un sorumluluğudur. `with_model(Model)` API
profiline çıktı/yetenek politikasını bağlar; `model_metadata()` seçilen bilgiyi
verir. `discover_models`, `save_catalog`, `require_catalog` API/Codex sürücülerinde
kullanılabilir. Codex için `with_cached_catalog` hesap bağını route ve completion
sınırında doğrular; interaktif login/fallback başlatmaz.

## Özel snapshot ve oturum bağı

`$STATE/catalogs/c-<digest>.json`, PrivateStore'un owned/0700 dizin, 0600 dosya,
symlink/hardlink reddi ve atomik yayın kurallarını kullanır. Kimlik; model
verisi, sağlayıcı/base URL, key selector ve credential/account/region bağına
dayanır. Snapshot hash'i bozuksa kullanılmaz. Aynı içerikteki yenilemenin gözlem
zamanı değişebilir; timestamp seçim hash'inin parçası değildir. Süre dolduğunda
otomatik yenileme veya TTL/freshness garantisi yoktur; zaman bilgi amaçlıdır.

API scope özel credential hash'ine, SigV4 scope anahtar/session/region'a,
Codex scope account ID'ye bağlıdır. Token refresh aynı Codex hesabında seçimi
bozmaz. Kaydedilen dosya gerçek API/token/secret değerini içermez; private
bağ bilgisi normal `models`/`--cached` çıktısına veya Debug'a eklenmez.

Oturum route'u katalog ID'sini ve API'de çözülmüş çıktı politikasını pin eder.
Yeni snapshot, eksilen/değişen seçim veya anahtar/account farkı açıkça yeni
seçim gerektirir. TUI F6 yalnızca seçili katalogdaki modele geçebilir; başka
kataloğa geçmek yeni CLI çağrısı ister. Pal'ın sonraki ask/tui çağrılarında aynı
`--model-catalog` seçeneğini veya aynı kalıcı config'i kullan. Normal resume/
Pal akışı katalog yenilemez ve discovery için gizli ağ çağrısı eklemez.

## Doğrulama

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --workspace --locked
python3 scripts/catalog_smoke.py
python3 scripts/native_providers_smoke.py
python3 scripts/services_smoke.py
python3 scripts/provider_contract_smoke.py
python3 scripts/tui_smoke.py --binary target/release/namzu-rs
```

15 yeni Rust testi; toplam 583. Katalog normalizasyonu/sayfalama, exact model,
context/output ayrımı, metadata bütünlüğü, aynı içerikte ID kararlılığı,
anahtar/account/cursor değişimi ve ilk HTTP'den önce iptal/yetenek reddi sınanır.
18 release kabul grubu gerçek CLI/normal-Pal PTY'de keşif → özel kayıt → model → dosya
oraklı zincirini, resume, çıktı tavanı, offline inceleme, snapshot bozulması ve
sayfa hatasında no-publish davranışını doğrular. GET SigV4 Python ile bağımsız
kontrol edilir. Ayrıca 57 mevcut sağlayıcı/servis/TUI regresyon kontrolü geçti.
Normal testler yalnızca loopback/sahte anahtar kullanır;
canlı model ve gerçek hesap keşfi yapılmış sayılmaz.
