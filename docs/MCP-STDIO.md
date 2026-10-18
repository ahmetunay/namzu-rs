# MCP 3.2 — stdio ve süreç yaşam döngüsü

2026-10-18: `namzu_mcp::stdio::{Client, Spec}` gerçek yerel MCP sunucusunu açıkça
başlatır ve 3.1 protokol oturumuyla bağlar. Üretim istemcisi/IO/supervisor Rust'tır;
CLI yöneticisi, araç keşfi ve ajan entegrasyonu sonraki parçalardadır.

Resmî [2025-11-25 stdio sözleşmesi](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#stdio)
karşılaştırıldı: stdin/stdout üzerinde newline ile ayrılan JSON-RPC mesajları,
stdout'ta yalnızca protokol, stderr'de bağımsız tanılama. Okunan transport kaynak
SHA-256: `a247fdbb3cc25c805ef43124db18d9b60a56669b3e65bd163dffb76f4129dfc0`.
Protokol sürümü ve payload sözleşmesi [MCP.md](MCP.md) içindedir.

## Açık host seçimi

```rust,no_run
use namzu_core::{Cancellation, TurnContext};
use namzu_mcp::{Limits, stdio::{Client, Spec}, types::Implementation};
use std::time::Duration;

# fn example() -> Result<(), Box<dyn std::error::Error>> {
let context = TurnContext::new(Duration::from_secs(300), Cancellation::default())?;
let mut spec = Spec::new("/absolute/path/to/mcp-server", "/absolute/workspace");
spec.args = vec!["literal-argument".into()];
let mut client = Client::connect(
    spec, Implementation::new("namzu-rs", "0.1.0"), Limits::default(), &context,
)?;
let result = client.call("ping", None, &context)?;
let report = client.close();
assert!(report.abandoned.is_empty());
# Ok(())
# }
```

Program ve cwd mutlak, mevcut dosya/dizin olmalıdır. Shell komut satırı kurulmaz;
argümanlar ayrı literal alanlardır. Paket indirme, npx/uv başlatma veya proje
ayarlarını otomatik okuma yoktur. Seçilen server programı **host kodudur**; bu API
OS sandbox'ı sağlamaz. Programın kendisi çalışırken dosya/ağ erişimi yapabilir.
Config/güven/izin arayüzü 3.4'te bağlanacaktır; bugün çağrı trusted host SDK API'sidir.

Child ortamı `env_clear` ile kurulur. Sabit PATH, locale/timezone ve proxy/CA
seçicileri aktarılır. HOME, model anahtarları, PYTHONPATH, LD_PRELOAD ve keyfi host
env'i kendiliğinden aktarılmaz. `Spec.env` açıkça verilmiş anahtar/değerleri ekler
veya izin verilen varsayılanları ezer; gizli değerleri/argümanları loglamaz.
Sunucuya bir credential gerekiyorsa açık host seçimi gerekir; otomatik login yoktur.

## Bağlantı ve I/O

`connect` config'i ve protokol limitlerini launch öncesinde doğrular. Ayrı process
group, piped stdin/stdout/stderr ve nonblocking descriptor'larla başlar. Initialize
ve initialized notification'ı yürütür; notification tamamen yazılmadan Ready
olmaz. Startup hatası/iptalinde yeni süreç de temizlenir.

- `send` admission sonrasında request ID döndürür ve sınırlı yazma kuyruğuna ekler.
- `next` okuma/yazma/stderr'i birlikte ilerletir; yanıtları ID/method ile döndürür.
  Server ping/unsupported client request yanıtlarını kendisi yazar; host effect yoktur.
- `call` tek isteğin kolaylık API'sidir. Başka pending request varsa Busy verir;
  açık pipelining için send/next kullanılır. RPC error ve tool isError veri
  sonuçlarıdır; sağlıklı bağlantıyı sırf bu sonuçlar için kapatmaz.
- Gizli worker thread'i yoktur. Host bağlantıyı `next` ile sürmeli veya kapatmalıdır;
  idle nesne kendiliğinden zamanlayıcı/reader çalıştırmaz. İptal/deadline kontrolü
  host tekrar I/O'yu sürünce devreye girer; `close` açık kapatmadır.

Linux parent-death sinyali server'ı **oluşturan thread** ile ilişkilidir. Bu nedenle
Client thread-affine'dir (`!Send`, `!Sync`): aynı kalıcı thread'de oluşturulur,
kullanılır ve kapatılır. Ayrı bir host worker istenirse Client worker'ın içinde
oluşturulur; worker arası taşınmaz. Kütüphanenin kopuk/unutulmuş reader thread'i yoktur.

Framing parçalanmış UTF-8'i ve tek read'de birden çok mesajı destekler. LF ayraçtır;
CRLF kabul edilir. JSON içindeki newline escaped olmalıdır. Boş/bozuk satır,
stdout log'u, yanlış ID, duplicate key, sürüm hatası veya eksik son frame kapatır.
Leader son yanıtı yazıp hemen çıksa da pipe son kez drain edilir; tamamlanmış yanıt
sırf process exit yarışı nedeniyle atılmaz. Leader ölürken descendant pipe'ı açık
tutsa bile reader EOF bekleyerek takılmaz.

## Kaynak sınırları

3.1'in frame byte/string/derinlik/düğüm/pending/lifetime-request sınırları korunur.
Stdio ayrıca şu yerel limitleri uygular:

| Kaynak | Varsayılan | Üst sınır |
| --- | --- | --- |
| Bağlantı ömründe stdout | 16 MiB | 64 MiB |
| Bağlantı ömründe stderr | 1 MiB | 16 MiB |
| Saklanan stderr kuyruğu/tail | 16 KiB | 64 KiB |
| Okuma ve yazma kuyruklarının byte bütçesi | Her biri 2 MiB | Her biri 8 MiB |
| Kuyrukta frame sayısı | 64 | 64 |
| Shutdown grace, her aşama | 100 ms | 2 saniye |
| Literal argüman sayısı / toplam boyutu | En fazla 128 / 64 KiB | Sabit |
| Açık env girdisi / tek değer | En fazla 64 / 16 KiB | Sabit |

Framing buffer'ı parse öncesinde sınırlandırılır; sınırsız read_to_end yoktur.
Her turda sınırlı read/write yapılır; stdout veya stderr'i input'tan önce dolduran
server istemciyi pipe deadlock'una sokmaz. Stderr tek başına hata sayılmaz; kota
uygulanır, tutulan tail sınırlıdır. `stderr_tail()` yalnızca açık tanılama erişimidir;
kütüphane untrusted stderr'i stdout'a/terminale otomatik basmaz.

Kuyruk bütçesi mantıksal byte sayısıdır; write kuyruğunda kısmen yazılmış mevcut
frame'in tüketilen prefix'i frame tamamlanana kadar bellekte kalabilir. Her yönde
bellek yine sabit limitler ve en fazla bir frame prefix'iyle sınırlıdır; heap/RSS'nin
bütçeye birebir eşit olması iddiası yoktur.

## İptal, belirsizlik ve cleanup

Process lifetime context'i bağlantıya, her request'in context'i parent deadline'a
bağlıdır. Progress deadline'ı uzatmaz. Erken iptal/deadline, EOF, protocol/I/O veya
kota hatası bu **bağlantı neslini** kapatır; restart/reconnect/replay yoktur.

`cancel(id)` bu stdio neslini de kapatır; aynı process'teki diğer pending işler de
kesilir. Bu, tek remote tool'un işbirliğine güvenmek yerine process cleanup sağlayan
bilinçli politikadır. Çalıştırılmamış/kısmen yazılmış araç frame'i sırf cancellation
notification göndermek için tamamlanıp dispatch edilmez. Normal frame kuyruğu boşsa
cancellation best-effort gönderilir; initialize için cancel notification üretilmez.

`close` önce stdin'i kapatır, grace bekler, gerekirse process group'a SIGTERM,
sonrasında SIGKILL gönderir ve doğrudan child'ı reap eder. Leader graceful çıksa da
aynı gruptaki arka plan çocuklarına cleanup uygulanır. PGID reuse yarışını önlemek
için leader grup kapatılmadan reap edilmez (`waitid/WNOWAIT`). Drop aynı cleanup'ı
uygular. Cleanup'ın context'ten bağımsız grace süresi çağrının dönüşünü uzatabilir.

`CloseReport.abandoned` ID/method bilgisini korur. İstek yazılmamış, kısmen yazılmış
veya remote etkisi tamamlanmış olabilir; bu liste **sonucu kesinleşmemiş** işlerdir.
Args, credential ve stderr report'a eklenmez. Etkiler geri alınmaz. `forced`,
leader için TERM/KILL zorlaması gerektiğini belirtir; graceful leader sonrası grup
çocuğu temizliği buna rağmen uygulanabilir. Tekrarlı close idempotenttir.

Linux'ta child için PR_SET_PDEATHSIG eklenir: native client host SIGKILL ile
kaybolduğunda doğrudan server da ölür. Bu yalnızca **doğrudan child** korumasıdır;
istemci SIGKILL sonrası tüm torunları temizleyen bağımsız guardian değildir.
Process group'tan setsid ile kaçan kötü niyetli programlara karşı OS confinement
iddiası yoktur. Runtime Unix içindir; bu teslim Linux'ta doğrulandı. Kernel'de
kesilemeyen süreç/FS çağrılarına karşı mutlak duvar saati kapanış garantisi yoktur.
Daha güçlü sandbox/computer sınırları ayrı capability paketlerindedir.

## 3.2 kabul — 2026-10-18

**25 yeni test**, toplam **238 Rust testi** geçti:

- 2 framing testi: byte byte Unicode/CRLF/coalescing ve erken byte/frame kuyruk sınırı.
- 22 Linux süreç testi: 21 davranış senaryosu ve gerçek host SIGKILL testinin
  native subprocess fixture giriş noktası. Karşı tarafta bağımsız stdlib Python
  fixture server vardır; üretim istemcisi Python/Node wrapper kullanmaz.
- 1 compile-fail ownership testi: Client başka thread'e taşınamaz.
- Gerçek initialize/ping/tool call, graceful EOF/reap, literal argv/env/cwd,
  200 KiB stdin + 100 KiB stdout + 200 KiB stderr duplex akışı sınandı.
- Ters sırada pipelined yanıt, server-initiated ping/rejection, RPC vs tool hatası,
  son yanıt/process-exit yarışı, yanlış ID ve etkiden sonra replay reddi sınandı.
- Gönderim öncesi ve kısmi yazma iptali; gerçek etkiden sonra iptal/deadline;
  lifetime/startup iptali; EOF'siz descendant pipe; Drop; native host SIGKILL
  sonrası doğrudan server ölümü PID fd'leriyle doğrulandı.
- stdout/stderr/frame sınırları, truncated/bozuk/sürüm uyumsuz startup, config
  reddi ve hata metninin untrusted tanılama içermemesi kontrol edildi.
- Workspace fmt, all-targets Clippy (`-D warnings`) ve release build geçti.
- Release `stdio` örneği gerçek fixture subprocess ile ping ve exit 0 yaptı;
  model/hesap çağrısı **0**. Normal süreç testleri Python 3 stdlib gerektirir.

```sh
cargo test --workspace --locked
cargo run --release -p namzu-mcp --example stdio -- /absolute/server literal-args
```

Yerel kanıtlar ignored `artifacts/mcp32-validation.json` ve `mcp32-example.json`.
Bu aşamada Docker, PTY veya harici üçüncü taraf MCP servisleri yeniden sınanmadı.
HTTP 3.3, güven/config 3.4, keşif 3.5, CLI/TUI/Pals bağları 3.6 ve OAuth 3.7 henüz
yoktur. Sıradaki parça **3.3 — Streamable HTTP/SSE taşıması**.
