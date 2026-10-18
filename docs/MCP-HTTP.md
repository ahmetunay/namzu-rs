# MCP 3.3 — Streamable HTTP/SSE

Native Rust `namzu_mcp::http::{Client, Spec, Error}` kullanılır. SDK'nin tek
bağlantı nesli ve tek aktif çağrı sözleşmesidir; model çağırmaz. MCP'nin
2025-11-25 ve 2025-06-18 sürümleriyle çalışır. HTTP adaptörü core'a veya model
sağlayıcısına bağımlılık eklemez; aynı `ClientSession` protokolünü kullanır.

```sh
cargo run --release -p namzu-mcp --example http -- https://server.example/mcp
# İsteğe bağlı açık bearer: değer değil, ortam değişkeninin adı verilir.
cargo run --release -p namzu-mcp --example http -- https://server.example/mcp MCP_TOKEN
```

Örnek gerçek initialize → initialized → ping → DELETE çalıştırır; varsayılan
bütçe 300 saniye, ayrı kapanış bütçesi 5 saniyedir. Sunucuya ulaşma/hesap izni
sağlamaz. Örnek endpoint bir yer tutucudur.

## Yaşam döngüsü

- `Client::connect(spec, info, &context)` initialize sonucunu doğrular, session
  ID'yi alır ve initialized bildirimi 202/boş gövdeyle kabul edilince Ready olur.
- Her JSON-RPC mesajı ayrı POST'tur. Accept JSON ve SSE içerir; gövde JSON'dur.
  Initialize isteğinden sonra anlaşılmış `MCP-Protocol-Version` eklenir.
  `MCP-Session-Id` verilmişse sonraki POST/GET/DELETE isteklerinde taşınır.
  Yinelenen, boş, kontrol karakterli, 1024 byte'tan uzun veya değiştirilen
  session ID reddedilir. ID ve bearer header'ları sensitive işaretlenir.
- `call(method, params, &context)` JSON yanıtı veya SSE üzerinden eşleşen
  protokol sonucunu döndürür. Server ping/unsupported request çekirdekçe
  yanıtlanır; cevap ayrı POST ile gönderilir. Host aracı çalıştırılmaz.
- SSE bildirimleri `pop_event()` ile alınan inert verilerdir. Görev başlatmaz,
  izin açmaz veya yerel isteği iptal etmez. RPC error ve `isError` araç sonucu
  taşıma hatası sayılmaz; bağlantı kullanılabilir kalır.
- `listen(&context)` isteğe bağlı GET akışından bir bildirim döndürür. Akış ve
  okunmamış byte'lar sonraki listen çağrısı için korunur. GET Accept yalnızca
  SSE'dir; 405 `None` döndürür. Normal GET'te RPC response kabul edilmez.
  Host listen/pop_event çağrılarını sürer; bağımsız bir mesaj worker'ı yoktur.
- `close(&cleanup_context)` yerel nesli kapatır, tutulmuş GET'i bırakır ve
  session varsa açık DELETE dener. 405 desteklenmeyen remote sonlandırmadır;
  404 zaten kayıp session'dır. Diğer hatalar raporlanır. Drop ağ I/O'su yapmaz;
  DELETE veya uzak işlerin sonlanması Drop ile garanti edilmez.

## Kesilme ve belirsiz etki

HTTP başlık, JSON gövde, SSE ve yanıt POST beklemeleri aynı `TurnContext`
iptal/deadline'ıyla kesilir. Beklenen async future bırakılır; detached istek
thread'i oluşturulmaz. İptal en fazla yaklaşık 10 ms aralıkla denetlenir;
SSE yorumları/ilerleme olayları bütçeyi uzatmaz. Her listen çağrısının açık
context'i vardır; stream için ayrıca sabit toplam byte/olay sınırı korunur.

Taşıma/protokol/akış/limit/context hatası o nesli kapatır. `abandoned()` bekleyen
isteğin ID ve metodunu saklar; timeout sırasında çekirdekte emekliye ayrılmış
bir ID de belirsiz etkilerden düşmez. Bu liste işlemin gerçekten yürütüldüğünü
veya geri alındığını kanıtlamaz, kalıcı recovery journal değildir. Admission
öncesi iptal ise hiçbir istek göndermez ve sağlıklı session'ı kapatmaz.

404 session kaybında yeniden initialize veya araç replay yoktur. Redirect ve
Reqwest otomatik retry politikası kapalıdır. SSE `id`/`retry` bilgileri bu
sürümde yeniden bağlanma başlatmaz; Last-Event-ID gönderilmez ve özgün POST
tekrarlanmaz. Kopma açık hatadır. Resume/reconnect, subscription ve recovery
3.8'in ayrı kapsamıdır.

## Sınırlar ve güven

JSON mesajı ve SSE satırı/olayı `Limits.message_bytes` ile sınırlıdır
(varsayılan 1 MiB, en fazla 4 MiB). JSON ayrıca ortak derinlik/düğüm/string
kontrollerinden geçer. UTF-8, BOM, LF/CRLF/CR, parçalı çok byte'lı karakterler,
çok satırlı data, yorumlar ve boş priming olayları desteklenir. Bilinmeyen SSE
olay tipleri RPC olarak çalıştırılmaz. Sonuç için tam boş satırla bitmiş SSE
olayı gerekir; kesik olay başarı sayılmaz.

Her SSE akışı en fazla 16 MiB ve 4096 veri olayıdır. Bildirim kuyruğu en fazla
64 mesaj ve 2 MiB ham mesaj içeriğidir; dolması nesli kapatır. Bir HTTP chunk'ı
kopyalanmadan önce 16 MiB üst sınır kontrol edilir. JSON byte'ları mesaj sınırı
kontrolünden sonra biriktirilir. Sınırlar sonsuz/çok büyük akışları reddeder;
process için toplam OS RAM kotası değildir.

HTTPS gerekir; açık literal loopback IP'lerine HTTP yerel sunucular için
izinlidir. URL user/password ve fragment reddedilir. TLS doğrulaması açıktır;
Reqwest/native TLS ortam proxy ve CA ayarlarını kullanır. İstemci bunları
silmez veya sessiz direct fallback eklemez. Bearer yalnızca açık Spec ile
verilir; otomatik login, cookie/token keşfi veya yönlendirmeyle başka endpoint'e
gönderme yoktur. Hata gövdesi, endpoint, token ve session ID hataya/loga eklenmez.
Senkron facade kendi Tokio runtime'ını sürer; async host bunu runtime dışındaki
senkron thread'den veya `spawn_blocking` içinde çağırmalıdır.

Uyumluluk burada iki desteklenen Streamable HTTP sürümü ve JSON/SSE yanıtları
anlamındadır. Eski 2024-11-05 HTTP+SSE `endpoint` yönlendirme protokolü yoktur;
400/404/405'te eski endpoint keşfi denenmez. OAuth 3.7'dedir. CLI/config/güven
ve model aracı entegrasyonu 3.4–3.6'dadır; bu teslim bunları açmaz.

## Kabul — 2026-10-18

3 SSE framing birim testi ve 27 HTTP entegrasyon testi (bir çocuk süreç helper
entrypoint'i dahil), toplam 30 yeni Rust testi. Gerçek loopback TCP fixture'ları
initialize/session/sürüm/auth başlıkları, JSON/SSE, parçalı Unicode/priming,
server ping, GET akışını koruma/405, DELETE/405, RPC/araç hata ayrımı, yanlış ID,
404, redirect, header değişimi/tekrarı, eksik içerik, akış/mesaj/kuyruk sınırları,
iptal ve deadline'ı sınar. Belirsiz araç isteğinde fixture tam bir POST görür;
otomatik tekrar veya yeniden initialize yoktur.

İki test ayrı native Rust çocuk süreçte gerçek TLS ve HTTP CONNECT proxy kurar:
geçici üretilmiş self-signed sertifika child-local SSL_CERT_FILE ile güvenildiğinde
gerçek handshake/ping/DELETE geçer; güvenilmeyen sertifika Network hatasıyla
reddedilir. DNS/harici ağ/model hesabı kullanılmaz. Test için Python gerekmez;
TLS fixture sertifikası üretiminde sistem `openssl` executable'ı gerekir.
Geçici private key test dizinindedir ve kaynak depoya kaydedilmez.

Workspace toplamı 268 Rust testi; fmt, locked test, warnings-as-errors Clippy,
workspace release ve yerel sunucuyla release HTTP örneği kabul kontrolleridir.
Canlı model çağrısı 0. Üçüncü taraf MCP sunucusu/OAuth hesabı veya sistemler
arası canlı uyumluluk doğrulaması iddiası yoktur.
