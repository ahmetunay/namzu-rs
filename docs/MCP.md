# MCP 3.1 — protokol ve veri tipleri

2026-10-18: `namzu-mcp` bağımsız Rust paketi eklendi. Core bu pakete bağımlı
değildir. Paket JSON-RPC codec'i ve bir bağlantı nesline ait client durum
makinesidir; süreç/ağ/dosya açmaz, araç yürütmez ve kimlik bilgisi okumaz.

## Kapsam ve resmî referans

Uygulama MCP **2025-11-25** sürümünü önerir; initialize yanıtında
**2025-06-18** sürümüne anlaşmayı da kabul eder. Başka sürüme sessiz fallback veya
yeni initialize isteği yoktur. Bu, bu sürümlerin her isteğe bağlı özelliğinin
uygulandığı anlamına gelmez: roots/sampling/elicitation/tasks client capability'leri
ilan edilmez; server task augmentation istekleri de açılmaz.

Resmî kaynaklar 2026-10-18 tarihinde karşılaştırıldı:

- [2025-11-25 TypeScript wire şeması](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2025-11-25/schema.ts)
- [Lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)
- [Cancellation](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation)

Okunan şema SHA-256:
`e74b56e73b2e37bdb595f74ba22e428ad7f07aa3519355ba661d681298ed38ac`.
Lifecycle kaynak SHA-256:
`45a6e8b7fb8c96e7b9ba1b0a3c727e8451c1e55bf56bb62f3ab63fddc365b919`.
Cancellation kaynak SHA-256:
`9bd2a4422cf22b003621b0da0b812cb7b85c00e2feee1e6847a9d2f4837343d4`.
Bu kaynaklar wire sözleşmesi için referanstır; implementation yerel Rust kodudur.

## Wire sözleşmesi ve yerel sınırlar

`wire::decode/encode` tek bir JSON-RPC 2.0 nesnesini doğrular. Request,
notification, success ve error ayrıdır. Request ID string veya sayıdır;
`1` ile `"1"` eşleşmez. Null request/success ID, boş veya 256 byte'tan uzun
string ID, kesirli sayı ve JS safe-integer aralığı dışındaki ID reddedilir.
Error yanıtında eksik/null ID yalnızca **ilişkisiz** hata olarak çözümlenir;
hiçbir pending request'i tamamlayamaz.

MCP params ve success result nesne olmalıdır. Aynı anda method/result/error,
aynı yanıtta result+error, hatalı jsonrpc, batch array, fazladan JSON verisi,
UTF-8 hatası ve **her seviyede tekrarlı JSON anahtarı** reddedilir. Envelope
alanları sıkıdır; genişletmeler payload içindeki `_meta` veya ilgili veri
nesnesinde tutulur. Şüpheli mesajın ham içeriği hata Display'ine eklenmez.

| Kaynak | Varsayılan | Yapılandırılabilir üst sınır |
| --- | --- | --- |
| JSON mesajı | 1 MiB | 4 MiB |
| Tek string veya object key | 512 KiB | Mesaj byte sınırı |
| İç içe derinlik | 48 | 64 |
| JSON value düğümü | 32768 | 65536 |
| Eşzamanlı pending request | 64 | 256 |
| Bir bağlantı neslinde toplam request | 4096 | 65536 |

Bu sınırlar yerel kaynak/uyumluluk politikasıdır; daha geniş bütün geçerli
JSON-RPC değerlerini kabul etme iddiası yoktur. Pending sayısı toplam request
sayısını aşamaz. Method 1–128 byte, request ID 1–256 byte ve kontrol karakteri
içermeyen string olabilir. Sayısal ID aralığı ±9007199254740991'dir; ondalık JSON
sayı gösterimi ID olarak kabul edilmez. Bunlar peer izniyle yükseltilemez.

Outbound SDK nesneleri de aynı codec'ten geçer. Host tarafından kurulmuş JSON
ağacı serileştirmeden önce derinlik/düğüm/string açısından kontrol edilir;
byte limiti olan writer çıktı allocation'ını sınırlar. Tek başına codec, gelecek
transport'un **okuma sırasında** buffering sınırı uygulamasının yerini tutmaz.
DTO'ları doğrudan `serde_json::from_slice` ile çözmek bu wire doğrulamasını
sağlamaz; transport giriş noktası `ClientSession::receive` olmalıdır.

## Yaşam döngüsü

```text
New -> initialize -> Initializing
    -> geçerli initialize yanıtı -> AwaitingInitialized
    -> notifications/initialized başarıyla yazıldı -> Ready
    -> close / initialize hatası veya deadline -> Closed
```

1. `ClientSession::new` isim/sürüm ve client metadata sınırlarını kontrol eder.
2. `initialize(context)` empty client capabilities ve tercih edilen sürümle
   ilk request'i hazırlar. Tekrar initialize ve hazır olmadan normal request
   reddedilir. Client outgoing ping'i de Ready'ye kadar bekletir.
3. `receive(bytes)` ID, sürüm, serverInfo ve capabilities tiplerini kontrol eder.
   Bilinen capability alanında yanlış tip/null sessizce anlaşılmış sayılmaz.
   Başarısız initialize bağlantı neslini kapatır; otomatik tekrar yoktur.
4. `initialized_notification()` veriyi hazırlar; state henüz Ready olmaz.
   Transport tam mesajı yazdıktan sonra `mark_initialized_sent()` çağırmalıdır.
5. `request(method, params, context)` yalnızca Ready ve ilan edilen server
   capability'leriyle çalışır. Byte/slot/ömrü boyunca request sınırı **ID tahsisi
   ve transport etkisinden önce** kontrol edilir. Reddedilen request slot/ID tüketmez.

Ready'de ping; tools list/call; prompts list/get; resources list/template/read;
ilan edilmişse subscribe/unsubscribe; logging/setLevel ve completion/complete
adları kullanılabilir. Bu yalnızca **protokol kullanılabilirliği** kontrolüdür;
araç izni veya parametre JSON Schema doğrulaması değildir. Güven/izin 3.4,
keşif/doğrulama 3.5 ve ajan araç kabulü 3.6'da bağlanacaktır.

Server capability/implementation genişletmeleri ve instructions veri olarak
saklanır; bilinmeyen capability izin vermez. Instructions hiçbir system mesaja
çevrilmez. Server-initiated ping boş başarı yanıtı, diğer client method istekleri
MethodNotFound yanıtı üretir. Host filesystem, sampling veya elicitation çalışmaz.
Server notification'ları sınırlı veri olarak döner; otomatik eylem oluşturmaz.

## Yanıt, süre, iptal ve kopma

- Her ID yalnızca bir pending request'e karşılık gelir. Sırasız cevaplar doğru
  request/method'a döner. Yanlış tipte/ilişkisiz/duplicate response diğer işi
  tamamlamaz. Hatalı mesaj, mevcut pending binding'i değiştirmez.
- RPC error ile başarılı RPC içindeki `CallToolResult.isError` ayrıdır. Peer
  message/data alanları untrusted veridir; typed sonucu açıkça incelemek mümkündür,
  otomatik hata Display'i bu gövdeleri loglamaz.
- Request context'i core `TurnContext` parent deadline'ını ve aşağı doğru iptal
  ağacını devralır. Progress mesajı deadline'ı uzatmaz. Süre/iptal dolmuşsa gelen
  başarı bile kabul edilmez. Örnekte timeout 300 saniyedir; transport hostu kendi
  turn/request bütçesinden context vermelidir.
- Transport beklerken `expire()` çağırır; sonuçlar best-effort gönderilecek
  cancellation notification'ını içerir. `cancel(id)` yalnızca mevcut pending işi
  yerelde kapatır; **remote stop/rollback kanıtı değildir**. Sonradan gelen yanıt
  yok sayılır; aynı ID tekrar kullanılmaz.
- Initialize iptal notification'ıyla iptal edilemez. Initialize context'i
  biterse bağlantı kapatılır ve cancellation notification üretilmez.
- Cancellation yönlüdür: server'ın kendi request ID'siyle gönderdiği bildirim,
  aynı değerdeki client outbound request'i iptal edemez. Client server-request
  yürütmediği için bu bildirimler 3.1'de yalnızca veri olarak kalır.
- `close()` pending ID/method listesini döndürür ve oturumu kalıcı kapatır.
  İstekler peer'e ulaşmış olabilir; otomatik replay yoktur. Transport hatasında
  adapter bu kapatmayı ve remote cleanup'ı yapmalıdır. Bu aşamada pending kayıtları
  yalnızca bellektedir; süreç çökmesine dayanıklı MCP journal iddiası yoktur.

`receive` protocol hatası döndürdüğünde transport bunu görmezden gelip çalışma
başarısı üretmemelidir. Gelecek adapter bağlantı kapatma politikasını uygulamalı
ve belirsiz pending etkileri korumalıdır. Transport write başarısı, notification
flush, framing, process shutdown ve HTTP reconnect bu paketin yaptığı işler değildir.

## Veri tipleri ve örnek

`types` modülünde initialize, implementation, server capabilities, sayfa params,
tool/şemalar, tool-call params/result ve text/image/audio/resource-link/embedded
resource tipleri vardır. `_meta`, annotations ve vendor uzantıları korunur;
readOnlyHint gibi annotations izin sayılmaz. Base64/URI alanları burada opaque
veridir: dosya veya ağdan kaynak açılmaz. Bilinmeyen content tipi açıkça parse
hatasıdır; içerik sessizce atılmaz. Tam discovery, schema değerlendirme ve
content aktarımı 3.5–3.6 kapsamındadır.

```sh
cargo run --release -p namzu-mcp --example protocol --locked
cargo test -p namzu-mcp --locked
```

Örnek yerel fixture yanıtını JSON byte'larıyla protokole verir; çıktı 12'yi ve
iptal sonrası geç yanıtın yok sayıldığını kontrol eder. Model muhakemesi veya
gerçek MCP server testi değildir.

## 3.1 kabul — 2026-10-18

- **26 yeni protokol testi**, toplam **213 Rust testi** geçti; 0 başarısız/atlandı.
- Workspace format, all-targets Clippy (`-D warnings`) ve release build geçti.
- Release offline protokol örneği geçti; canlı model/kota çağrısı **0**.
- Bozuk/ambiguous envelope, duplicate key/ID, sıra dışı yanıt, boyut/derinlik/
  düğüm/string sınırı; typed metadata/content ve capability doğrulaması sınandı.
- Initialize sırası, write ack öncesi admission reddi, fallback sürüm/yanlış
  sürüm, RPC vs tool hatası, context iptali, gerçek parent deadline sonrası
  expiry, progress'in deadline'ı uzatmaması, geç yanıt ve yönlü cancellation sınandı.
- Pending/lifetime rezervasyonu, ID tekrar kullanmama ve close sonrası replay
  reddi sınandı. Client kimliğini metadata ile ezme ve büyük client metadata reddedilir.
- Önceki 187 test de geçti. Bu bağımsız protokol değişikliğinde gerçek Docker,
  PTY veya canlı MCP sunucusu çalıştırılmadı; 2.6'nın Docker/PTY sayıları bu teslimin
  yeni kabulü olarak sayılmaz.

Yerel makine kanıtları ignored `artifacts/mcp31-validation.json`,
`mcp31-example.json` ve `mcp31-spec-sources.json` dosyalarındadır. Kaynak/belgeler
ayrı commit ile GitHub'a yüklenir. **3.2–3.8 henüz tamamlanmadı.** Sıradaki iş
stdio framing, kontrollü env/cwd ve child process yaşam döngüsüdür.
