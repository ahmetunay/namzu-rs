# LSP SDK ve açık kod zekâsı komutları — 5.3

`namzu-lsp`, Unix üzerinde explicit stdio Language Server Protocol istemcisidir.
Core, CLI veya model sağlayıcısına bağımlı değildir; yalnız core context/hata
sözleşmesini kullanır. Sunucu executable'ı ve argümanları açıkça seçilir.
Otomatik sunucu keşfi/kurulumu veya kopan isteği yeniden oynatma yoktur.
Bu teslim yaşam döngüsü, diagnostics, definition/references ve tek dosyada
hash/sürüm bağlı düzenleme içindir; bütün LSP standardı iddiası değildir.

## Açık CLI

LSP sunucusunu önceden kendi ortamında kur ve mutlak executable yolunu seç.
Aşağıdaki `LSP_PROGRAM` gerçek kurulu sunucu yolu yerine kullanılan örnektir:

```sh
namzu-rs lsp --root ./project --program /absolute/path/LSP_PROGRAM \
  diagnostics src/main.rs --language rust
namzu-rs lsp --root ./project --program /absolute/path/LSP_PROGRAM \
  definition src/main.rs --line 12 --character 4
namzu-rs lsp --root ./project --program /absolute/path/LSP_PROGRAM \
  references src/main.rs --line 12 --character 4
```

Konumlar **sıfır tabanlı UTF-16** line/character birimleridir. Emoji surrogate
çiftinin ortası ve satır sınırı dışı konum reddedilir; LF, CRLF ve CR desteklenir.
`--language` varsayılanı `rust`; sunucuya gerçek language ID verilebilir.
`--server-arg=--stdio` gibi tekrarlanan seçenekler argv'ye aynen aktarılır;
istemci shell veya paket yöneticisi eklemez. Varsayılan süre 300 saniye,
`--timeout` açık override'dır. Model çağrısı ve abonelik anahtarı gerekmez.

Sunucu **host üzerinde kullanıcı yetkileriyle program çalıştırır**; OS sandbox
veya ağ izolasyonu değildir. İstemcinin URI/dosya kontrolleri sunucunun kendi
host erişimini sınırlandırmaz. Sunucu proje yapılandırmasını veya build
araçlarını kendi davranışına göre çalıştırabilir; explicit executable seçimi
bu güven sınırıdır. Sandbox genişletmesi 5.4'tür.

Env temizlenir; sabit PATH, locale/timezone ve mevcut proxy/CA değişkenleri
korunur. Diğer değişkenler otomatik aktarılmaz. Gerekli ortam değişkenlerinin
**adlarını** açıkça seçebilirsin:

```sh
namzu-rs lsp --root ./project --program /absolute/path/rust-analyzer \
  --server-env PATH --server-env HOME diagnostics src/main.rs
```

Argümanlar/env değerleri loglanmaz; stderr payload'ı hata çıktısına eklenmez.
Bu çağrılar executable'a host yetkisi verdiği için modele otomatik tool olarak
sunulmaz; 5.5 ile açık `--capability lsp --allow-lsp` seçimi kullanılabilir.
[Ajan/TUI/Pal izinleri ve plan apply](CAPABILITIES.md).

## Önizleme ve düzenleme

Rename varsayılan olarak yalnız plan ve tam yeni dosya önizlemesi döndürür:

```sh
namzu-rs lsp --root ./project --program /absolute/path/LSP_PROGRAM \
  rename src/main.rs --line 12 --character 4 --new-name next_name
```

Yanıttaki `output.result.plan`, URI, belge sürümü, kaynak SHA-256 ve edit'leri
barındırır. Yalnız bu nesneyi çalışma klasöründe `plan.json` olarak kaydet;
tüm CLI çıktı zarfı plan değildir. Daha sonra açık yazma izniyle uygula:

```sh
namzu-rs lsp --root ./project apply plan.json --allow-write
```

Bu ikinci çağrı sunucu başlatmaz. Aynı turda doğrudan uygulamak için rename
komutuna `--apply` verilir; bu açık yazma seçimidir. SDK, canlı belge sürümünü
ve hash'ini planla karşılaştırır; offline apply'da kalıcı LSP sürüm sayacı
olmadığından kaynak kimliğinin otoritesi SHA-256'dır.

`changes` veya tek `TextDocumentEdit` biçimindeki `documentChanges` desteklenir.
İkisi birlikte, başka URI, çok dosyalı değişiklik, create/rename/delete resource
operation, annotation ve stale version reddedilir. Bütün aralıklar orijinal
kaynak üzerinde doğrulanır; edit'ler sondan başa uygulanır. Çakışan aralıklar,
aynı başlangıçtaki belirsiz insertion'lar, ters aralık ve surrogate bölme
**herhangi bir yazmadan önce** reddedilir. Aynı plan değişmiş dosyada tekrar
uygulanmaz. İçeriği değiştirmeyen edit'in yeniden kabul edilmesi mümkün olduğu
için bu, bütün no-op planlar için kalıcı request-ID journal'ı değildir.

Kaynak ve hedef yalnız `--root` içindeki regular UTF-8 dosyalardır. Apply
symlink yolu ve hardlink hedefini reddeder; mutlak/parent/NUL yollar, remote
file URI, query/fragment ve kök dışı URI kabul edilmez. Geçici dosya aynı
parent'ta oluşturulur, önceki izinler korunur, içerik yazılıp sync edilir;
son anda hash tekrar kontrol edilir ve rename ile tek dosya atomik değiştirilir.
Hata/iptalde henüz yayımlanmamış geçici dosya temizlenir.

Harici writer'larla atomik compare-and-swap veya proje çapında transaction yoktur;
son kontrol ile rename arasındaki OS yarış aralığı tamamen kapatılamaz. Tamamlanan
dosya etkisi sonraki iptal veya bildirim hatasında geri alınmaz. Live apply sonrası
`didChange` gönderimi başarısız olursa hata açıkça dosyanın yazıldığını bildirir;
dosyayı incele, işlemi otomatik tekrarlama. Başarılı `didChange` bir notification
write'ıdır, sunucu tarafı işleme onayı değildir. Dizin entry crash kalıcılığı ve
kesilemeyen OS dosya sistemi çağrıları için ek garanti verilmez.

## Yaşam döngüsü ve veri doğrulama

- `initialize → initialized → didOpen`; belge version 1 ve okunan metnin hash'iyle açılır.
- Yalnız UTF-16 müzakere edilir; farklı `positionEncoding` reddedilir. Sunucunun
  sync ve query/rename capability'leri kontrol edilir; full-text `didChange` kullanılır.
- Push `publishDiagnostics` veya ilan edilmiş pull `textDocument/diagnostic`
  desteklenir. Pull'da full report istenir; unchanged/result-ID cache yoktur.
- Diagnostic mesaj/severity/code/range alanları typed/bounded doğrulanır. Açık
  belgeye ait olmayan veriler tutulmaz. Bildirilmiş eski sürümler atılır;
  version >1'de sürümsüz push da güncelliği kanıtlanamadığı için atılır.
- Definition/references `Location`, `Location[]`, null ve `LocationLink` sonuçları
  desteklenir; dönen dosya URI'leri ve seçim aralıkları workspace içinde doğrulanır.
  Sonuçlar açık belge snapshot'ına aittir; disk watcher veya otomatik refresh yoktur.
- `didClose`, `shutdown`/null ack ve `exit` uygulanır. CLI shutdown ile oturumu
  kapatır; SDK `close_document` ayrıca kullanılabilir.
- Server-initiated `workspace/applyEdit` her zaman `applied:false` olur;
  workspace configuration isteklerine bounded null sonuçlar verilir, host config
  keşfedilmez. Work-progress create null ile yanıtlanır; diğer client request'ler
  method-not-found alır. Dynamic registration, executeCommand ve server edits yoktur.

Tek mutable/thread-affine sahip nonblocking stdin/stdout/stderr'ı sürer; kopuk
reader thread'i veya gizli restart yoktur. Oturum ana context'in deadline/iptal
soyunu korur; yeni context ile ömrü uzatılamaz. Cleanup yalnız owned shutdown
ve kill/reap içindir, yeni belge etkisi vermez. İptal/protokol/I/O hatasında
aktif istek kapatılır. EOF/truncated frame, yanlış yanıt ID'si, bozuk JSON,
ambiguous framing ve bütçe aşımı kapanmaya yol açar. Server hata body'si
loglanmaz. Linux'ta direct-child parent-death koruması ve thread-affinity vardır.
Drop/failure yolunda process group sonlandırılır; leader reaped edilmeden PGID
kimliği korunur. Pipe miras alan normal alt süreçler de kapatılır; bu host
execution olduğundan kötü niyetle process group'tan kaçan programlara sandbox
garantisi verilmez.

## Sınırlar ve SDK

| Kaynak | Limit |
| --- | --- |
| UTF-8 dosya/sonuç dosyası | 1 MiB |
| Content-Length JSON frame | 1 MiB; kaçışlar ve JSON metadata dahil |
| Header | 8 KiB |
| Receive buffer | 1 MiB + 16 KiB |
| Oturum stdout / stderr | 16 MiB / 1 MiB |
| Oturum mesajı / client request | 4096 / 4096 |
| Açık belge | 32 |
| Diagnostic / location / edit | 512 / 256 / 256 |
| Diagnostic message / source-code metadata | 4096 / 256 byte |
| Edit toplam replacement metni | 1 MiB; sonuç dosyası da en çok 1 MiB |

Tek bir sınırın altında olmak diğer sınırları kaldırmaz; örneğin JSON kaçışları
1 MiB dosyayı frame bütçesinin üstüne çıkarabilir. SDK ayrı pakettedir:

```rust,ignore
use namzu_lsp::{Client, Spec, Position};
let mut spec = Spec::new("/absolute/path/server", canonical_root);
spec.args = vec!["--stdio".into()];
let mut client = Client::start(spec, &context)?;
let doc = client.open("src/main.rs", "rust", &context)?;
let diagnostics = client.diagnostics(&doc.uri, &context)?;
let definition = client.definition(&doc.uri, Position {line: 12, character: 4}, &context)?;
let plan = client.rename(&doc.uri, Position {line: 12, character: 4}, "next_name", &context)?;
let preview = client.workspace().preview(&plan, &context)?;
// Host policy decides whether to apply the reviewed plan.
let applied = client.apply(&plan, &context)?;
client.close_document(&doc.uri, &context)?;
let report = client.close(&cleanup_context);
```

## Kabul

- 22 yeni Rust testi: 4 framing, 7 gerçek subprocess SDK, 8 workspace/edit,
  3 CLI süreç testi; toplam **495**. Format, workspace test, Clippy `-D warnings`
  ve release workspace derlemesi geçti.
- `python3 scripts/lsp_smoke.py`: sekiz release CLI kabul grubu; push/pull ve
  parçalı frame, definition/reference/link, server edit reddi, rename önizleme,
  ayrı write grant/hash/replay, live apply/Unicode, kök dışı/çok dosyalı/stale
  edit reddi, surrogate ön kontrolü, gerçek SIGINT/deadline ve process reap.
- Bağımsız Python fixture gerçek stdio JSON-RPC/LSP konuşur ve dosya/byte
  oracle'ları kullanır. Canlı model çağrısı 0. Bu ortamda rust-analyzer bileşeni
  kurulu değildi; gerçek rust-analyzer/clangd proje doğruluğu ölçülmedi ve
  otomatik kurulmadı.

Hover/completion, code action, formatting, multi-file/resource edits, dynamic
registration, TCP LSP ve TUI/Pal/model tools entegrasyonu bu teslimin kapsamında
değildir. Client tam bir IDE veya OS sandbox değildir.
