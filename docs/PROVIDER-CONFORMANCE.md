# Ortak sağlayıcı kabulü — 6.5

6.1–6.4'te uygulanan sürücülere aynı davranış sözleşmesi uygulanır. Bu teslim,
13 ortak Rust senaryo grubunu 28 servis/format/auth kombinasyonunda çalıştırır.
Bunlar 13 Rust testidir; 364 ayrı Rust testi veya 28 farklı sağlayıcı değildir.
Her kombinasyonda aynı assertion'lar çalışır; desteklenmeyen transport biçimi
başarılı/atlanmış hücre diye sayılmaz.

Fixture'lar yalnızca loopback, sahte credential ve geçici özel state kullanır.
Normal kabul testleri gerçek hesap/model kotasını tüketmez. CLI ve SDK aynı
sağlayıcı sürücülerini kullanır; çekirdeğe CLI bağımlılığı eklenmedi. Codex için
endpoint/credential enjeksiyonu yalnızca `cfg(test)` altında vardır. Üretim
abonelik tokenı başka base URL'ye yönlendirilemez.

## Ortak matris

| Profil | Wire | Mod | Kombinasyon |
|---|---|---|---:|
| openai-compatible, openrouter, zen, zen-go, deepseek, lmstudio | Chat Completions | Buffered + SSE | 12 |
| openai-compatible, zen, zen-go | Responses | Buffered + SSE | 6 |
| anthropic | Messages | Buffered | 1 |
| google | GenerateContent | Buffered | 1 |
| bedrock | Converse | Buffered, bearer + explicit session SigV4 | 2 |
| ollama | native Chat | Buffered, anahtarsız | 1 |
| zen, zen-go | Messages + GenerateContent | Buffered | 4 |
| codex | Native subscription Responses | SSE | 1 |
| **Toplam** | | | **28** |

Yerel LM Studio anahtarsız test edilir; diğer API profilleri sahte anahtar,
Codex ise süresi dolmamış sahte account tokenı kullanır. SigV4 test credential'ı
kod içinde açıkça kurulur; AWS ortam değişkeni/credential chain okunmaz.
Zen native hücreler, servisin böyle bir modeli gerçekten sunduğuna dair iddia
değildir. Matris model adları da gerçek model erişimi kanıtı değildir.

13 senaryo grubunun her biri bütün 28 kombinasyona uygulanır:

1. Descriptor, doğru endpoint/auth/model, tamamlanmış metin, presentation delta,
   3 giriş/2 çıkış/5 toplam kullanım; tek kullanımlık metadata ve yeni iptal
   girişiminde eski metadata/replay'in temizlenmesi. Private reasoning event'e çıkmaz.
2. Eksik kullanım ve bilinmeyen ücretin `None` kalması; sıfır/ücretsiz sayılmaması.
3. Ajan/araç döngüsü, gerçek `42\n` dosya oraklı, tek araç etkisi, iki model çağrısı
   ve toplam kullanım. Aynı döngü tool-error sonucuyla da çalışır; hata sonucu
   modelin sonraki isteğinde veri olarak bulunur ve dosya yazılmaz.
4. Negatif kullanım sayacının bütün araç etkilerini engellemesi.
5. Incomplete finish/status/done bilgisinin araç etkilerini engellemesi.
6. Object olmayan araç argümanının araç etkilerini engellemesi.
7. Bozuk araç adı, önceden iptal ve dolmuş deadline durumunda sıfır HTTP isteği.
8. 400/401/403/404/429/503 için ortak hata sınıfları, Retry-After, sunucu hata
   gövdesi/anahtar redaksiyonu ve tam bir istek: retry/fallback yok.
9. Başka loopback host/porta 307 yönlendirmeyi izlememe; credential iletimi yok.
10. Kesilmiş HTTP gövdesinde hiçbir araç, başarılı metadata veya replay yayını yok.
11. Başlık beklerken ve yarım gövde geldiğinde iptal: araç etkisi yok.
12. Geçerli PNG'nin native/Chat/Responses isteğine taşınması; seçilen modelin
    `images:false` veya `tools:false` bildirmesi halinde HTTP öncesi ret.
13. Sunucunun yarım gövdeyi açık tutması halinde typed deadline hatası; araç
    etkisi ve metadata yok. Dar bir elapsed-time penceresiyle yarış testi yapılmaz.

Bu kapsam [6.1 sözleşmesinin](PROVIDER-CONTRACT.md),
[6.2 servis yollarının](COMPATIBLE-SERVICES.md),
[6.3 native sınırlarının](NATIVE-PROVIDERS.md) ve
[6.4 katalog seçiminin](MODEL-CATALOG.md) yerine geçmez. Örneğin private replay,
SSE parçalama, AWS imzasının bağımsız HMAC kontrolü ve sayfalama sınırları için
önceki ayrıntılı testler korunur.

## Tek komutla deterministik kabul

Önkoşullar: Rust stable, Python 3, sistem derleyicisi/OpenSSL ve Linux PTY.
`target/release/namzu-rs` önce derlenmelidir; model sunucusu/Docker gerekmez.

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --workspace --locked
python3 scripts/provider_conformance.py
```

Koşucu sırasıyla ortak Rust matrisini, canlı koşucunun 9 offline birim testini,
10 gerçek CLI loopback okuma oraklını, katalog (18), native (15), servis (11),
provider-contract (6) ve TUI (25) kabul gruplarını çalıştırır. Son beş suite'te
75 mevcut release kabul kontrolü vardır. Canlı koşucu için eklenen 10 grup bu
75'e dahil değildir. Parametresiz gerçek `models`/auth/login çağırmaz.

Çıktı `artifacts/provider-conformance/report.json`; her suite'in logu yanındadır.
Rapor başlangıçta `running`, başlamamış suite'ler `pending` olur; kesilen koşum
önceki yeşil raporu bırakmaz. Eksik binary/toolchain, timeout veya başarısız suite
nonzero exit verir. Sonraki suite'ler hata halinde de çalıştırılır. Varsayılan
PATH'te cargo yoksa `--cargo /tam/yol/cargo` verilebilir. Her suite'in süreç
limiti 300 saniyedir; bu bir model turn'ünün 300 saniye sınırıyla aynı ölçüm değildir.

Bu teslimin Rust toplamı 596'dır: önceki 583 + 13 ortak matris testi.

## Açık canlı smoke

Bu komut **gerçek servis/model isteği yapar ve kota/ücret tüketebilir**. Normal
matrise dahil edilmez. Tam servis/model seçimi ve `--live` zorunludur. Ortam
anahtarlarını kullanıcı kendi ortamında ayarlar; komut anahtar değeri kabul etmez.

```sh
python3 scripts/provider_live_smoke.py --live --provider openrouter \
  --model GERCEK_MODEL_ID --api-key-env OPENROUTER_API_KEY \
  --out artifacts/openrouter-live.json

python3 scripts/provider_live_smoke.py --live --provider codex \
  --model GERCEK_KATALOG_ID --state-dir /ozel/kimlikli/state \
  --out artifacts/codex-live.json
```

API profilleri kendi endpoint/key-env varsayılanlarını kullanır; özel hedef
`--base-url`, Responses seçimi `--format responses`, desteklenen SSE `--stream`
ile açıkça seçilebilir. Bedrock SigV4 için `--bedrock-auth sigv4 --aws-region REGION`
ve gerektiğinde o region'ın `--base-url` seçimi gerekir. Secret/session-env okuması
6.3 CLI sözleşmesindeki gibidir. Native buffered profilde SSE istemek reddedilir;
koşucu desteklenmeyen isteği başka wire'a çeviremez.

Tek görev, geçici klasörde rastgele bir satırı `read_file` ile okuyup aynen
döndürmektir. Beklenen satır prompt'a konmaz; dosya içeriği ve model yanıtı bağımsız
karşılaştırılır. CLI yalnızca `read_file` sunar; shell/yazma, MCP, sandbox, ses,
LSP veya bilgisayar yetkisi verilmez. Açık geçici config, mevcut varsayılan
capability/model/izin ayarlarını devralmaz. 3 model adımı, 2 araç çağrısı, 64 KiB
konuşma ve varsayılan/en fazla 300 saniye turn sınırı vardır; API'de 128 çıktı
token istenir. Codex native wire'a API output parametresi eklenmez.

Başarı için 1–2 araç, 2–3 model çağrısı, doğru satır ve değişmemiş kaynak gerekir.
Yanlış çıktı, modelin araç kullanmaması, hata veya süre aşımı başarısızdır.
Model/cwd/state seçimi oturum ortasında değiştirilmez; keşif, retry, login,
refresh sonrası account değiştirme veya ücretli modele fallback eklenmez.

Rapor `passed/failed`, explicit provider/model, sayısal çağrı adetleri, oracle
booleans ve süre içerir. Model metni, ham CLI stdout/stderr, sunucu hata gövdesi,
anahtar veya endpoint URL rapora kopyalanmaz. `--out` private 0600 **yeni** dosya
oluşturur; var olan dosya/symlink için model çağrısından önce ret verir. Üst
dizini önce oluştur. CLI'ın özel session kayıtları kendi state politikasını
korur; Codex'te verilen auth state'i de bu kayıtları tutar.

Bu teslimde gerçek hesap credential'ı kullanılmadı; **canlı model çağrısı 0**.
Canlı erişim/kota/fiyat, upstream ile tam özellik eşitliği, bütün gerçek modeller,
model kalitesi veya Jcode'a karşı performans sonucu iddia edilmez. Native SSE,
Vertex/OAuth/AWS credential chain, LM Studio WebSocket ve otomatik giriş tokenizer'ı
önceki açık farklar olarak durur. Sonraki ana aşama **7.1 telemetri/olay şeması**;
orijinalle ayrıntılı eşitlik karşılaştırması ayrıca yapılacaktır.
