# 4.3 — sayfa eylemleri ve dosya aktarımı

4.2'nin owned/ağsız Chromium altyapısına hedefe bağlı eylemler ve sınırlı dosya
aktarımları eklendi. Çekirdek/SDK ve worker native Rust'tır; model/TUI araç
entegrasyonu 4.4'te yapılacaktır. Tek ana belge/sekme, yeni profille restart,
Docker izolasyonu ve Chromium `--no-sandbox` sınırları [4.2 belgesindeki](BROWSER.md)
gibidir. İnternet erişimi açılmaz; kabul guest içindeki yerel HTTP sunucusuyla
çalışır. Canlı model veya kullanıcı hesabı kullanılmaz.

## Belge ve hedef kimliği

Önce `snapshot` isteği ver. `page` içindeki `frame_id`, `loader_id`, `url`
alanlarını `document` olarak; seçilen DOM düğümünün `backend_node_id` değerini
hedef olarak kullan:

```json
{
  "action": "fill",
  "target": {
    "document": {
      "frame_id": "SNAPSHOT_FRAME",
      "loader_id": "SNAPSHOT_LOADER",
      "url": "http://127.0.0.1:8765/actions"
    },
    "backend_node_id": 42
  },
  "text": "Namzu Türkçe ✓"
}
```

SDK: `Target { document: Document::from(&page), backend_node_id }` ve
`computer.browser(Request::Fill { target, text }, &context)`.
Worker mevcut URL/frame/loader ile eşleşmeyi ve düğümün normal ana DOM içinde
bulunduğunu denetler. Shadow/iframe hedefleri, eski belge/yeniden başlatma ve
kopmuş düğümler reddedilir. Başlık değişmesi hedefi bozmaz; fragment değişmesi
bozar. DOM/AX düğüm kimlikleri kalıcı locator değildir; güncel snapshot gerekir.
Sınırları aşan/büyük DOM incelemesi hata verebilir; sınırsız target araması yoktur.

## İşlemler ve izinler

CLI isteği mevcut JSON dosyası yoluyla verilir:

```sh
namzu-rs pal --allow-browser-actions computer PAL_ID browser request.json
```

| İstek | Alanlar / davranış | CLI izni |
| --- | --- | --- |
| `info`, `snapshot` | 4.2'nin readonly inceleme sözleşmesi | Ek flag yok |
| `navigate` | `url`; load/redirect/fragment sözleşmesi korunur | `--allow-navigate` |
| `click` | `target`; scroll, görünür box/hit kontrolü ve gerçek CDP mouse press/release | `--allow-browser-actions` |
| `fill` | `target`, `text`; text input/textarea değeri, input/change event | `--allow-browser-actions` |
| `select` | `target`, `values`; select option değerleri, input/change event | `--allow-browser-actions` |
| `press` | `target`, `key`; focus, gerçek CDP key down/up | `--allow-browser-actions` |
| `scroll_into_view` | `target`; DOM scrollIntoViewIfNeeded | `--allow-browser-actions` |
| `upload` | `target`, guest workspace `path`; tek file input dosyası | `--allow-upload` |
| `download` | `document`, `url`, yeni guest workspace `path` | `--allow-download` |

İzinler ayrı ve invocation'a özeldir. Navigate izni input, input izni upload,
upload izni download yetkisi vermez. Model araç listesi veya Pal profiline yeni
izin yazılmaz. CLI JSON/URL/hedef yapısı/izin kontrolünü lease/start/stop'tan önce
yapar; bu reddin çalışan tarayıcıya etkisi yoktur. SDK doğrudan trusted caller
API'sidir; uygulama kendi yetkilendirmesini yapmalıdır.

Fill boşaltmayı ve UTF-8 Unicode'u destekler; en fazla 16 KiB, NUL yoktur.
Desteklenen input türleri text/search/email/tel/url/password ve textarea'dır;
contenteditable, date/number gibi farklı editörler yoktur. Bu işlem native value
setter ve sabit input/change event üretir; her karakter için klavye olayı veya
clipboard kullanmaz. Select en fazla 16 farklı, her biri 1024 byte değer alır;
single select tam bir değer ister, disabled seçenekler reddedilir. Değerlerin
hepsi değişiklikten önce kontrol edilir. Press: ENTER/TAB/ESC/BACKSPACE/DELETE,
LEFT/UP/RIGHT/DOWN, HOME/END, PAGEUP/PAGEDOWN. Modifier/tuşu basılı tutma yoktur.
File input'a click/press yerine açık upload gerekir.

İzinli click/press veya sayfa event handler'ları gezinme/form gönderme gibi
sonuçlar doğurabilir. `Applied` tarayıcı komutunun kabulünü bildirir; SPA, paint,
HTTP veya event handler görevlerinin tamamlandığını garanti etmez. Sonuç için
readonly snapshot/ekran incelemesi yapılır; aynı eylem otomatik tekrarlanmaz.
Sayfa eylemlerinin DOM/layout değişikliklerine karşı atomikliği garanti edilmez.
Click hit kontrolü olaydan hemen öncedir; sayfa sonrasında değişebilir.

Caller JS, CSS selector veya CDP method gönderemez. Fill/select/hit kontrolü
sabit işlevlerle izole JS world içinde yapılır; metin ve değerler ayrı CDP
argümanlarıdır. Worker stdout/stderr hata ayrıntısına girilen metni eklemez;
Request Debug biçimi bütün içeriği redakte eder. Snapshot/form değerleri ise
kullanıcının açık inceleme isteğinin verisidir, model talimatı sayılmaz.

## Dosyalar

Bütün kabul edilen dosyalar **64 KiB** ile sınırlıdır. Upload yolu yalnızca owned
guest workspace içindedir; host yolu veya browser profilini okuyamaz. Symlink
ile root dışına çıkış ve regular olmayan dosyalar reddedilir. Worker dosyayı
0700 özel /tmp dizinine, 0600 olarak kopyalar ve Chromium file input'a bu kopyayı
verir. Chrome daha sonraki submit'te dosyayı okuyabildiği için başarılı staging
restart/cleanup'a kadar tutulur. Oturumda en fazla 16 staging dizini (en fazla
1 MiB dosya verisi) vardır. Temizlenmesi restart/cleanup ile olur; otomatik upload
tekrarı veya eski dosya içeriğine sessiz fallback yoktur.

Host → guest aktarımı önce mevcut açık komutla yapılır:

```sh
namzu-rs pal --allow-write computer PAL_ID upload input.bin LOCAL_FILE
# Sonra {"action":"upload","target":{...},"path":"input.bin"}
namzu-rs pal --allow-upload computer PAL_ID browser upload.json
```

Browser spec'li bilgisayarlarda başarılı `computer upload` ve `computer download`
çalışan oturumu durdurmaz; alınmış hedefler sırf aktarım yapıldı diye bozulmaz.
Diğer spec'lerin önceki davranışı korunur. Host IO hatasında persistent guard
bilgisayarı durdurabilir.

Download, URL'ye bir kez `Page.navigate` göndererek indirmeyi başlatır; ayrı bir
HTTP fetch veya JS download değildir. URL normal sayfa döndürürse navigation
etkisi oluşabilir ve indirme başarısız sayılır. Her worker başında download
politikası deny'dır; sıradan click/navigate indirmeye kalıcı izin vermez.
Açık download sırasında browser-level CDP policy geçici allowAndName olur:

1. Yeni özel /tmp dizini kullanılır; tek frame/GUID ve completion izlenir.
2. Sunucunun `Content-Disposition` dosya adı hiçbir zaman yol yapılmaz.
3. Progress byte sınırı aşarsa cancel gönderilir; completed dosya tekrar regular,
   symlink/hardlink, boyut ve okunan byte sayısı açısından doğrulanır.
4. Deny politikası geri kurulduktan sonra veri seçilen yeni workspace dosyasına
   0600, sync ve no-overwrite hard-link publication ile aktarılır.

64 KiB kabul edilen son dosya sınırıdır; progress bildiriminden önce ağ/tmp'ye
daha fazla veri gelebilir. Container RAM/tmp/PID sınırları ve deadline devam eder;
streaming çok büyük indirmeler, resume ve uzun ömürlü download manager yoktur.
Mevcut hedef dosyası/symlink üzerine yazılmaz. Başarı `Downloaded {path, bytes}`
verir; host bu yanıtın istenen yol ile eşleşmesini pending temizliğinden önce
kontrol eder. Guest → host export mevcut komuttur:

```sh
namzu-rs pal computer PAL_ID download saved.bin NEW_HOST_FILE
```

Host dosyası create-new/0600 yazılır. Başarısız export mevcut dosyayı değiştirmez.

## İptal ve kabul

Geçersiz yapı/izin host preflight'ta; stale/disabled/unsupported DOM hedefi ve
dosya IO durumu guest'te denetlenir. Guest bilinen eylem hatası bile belirsiz
etki olarak ele alınır: owned container durur ve `browser_action`,
`browser_upload` veya `browser_download` pending kaydı korunur. Süre aşımı,
iptal, protocol/transport veya yanlış reply yolu da aynı korumayı kullanır.
İncelemeden sonra mevcut açık acknowledge akışı gerekir. Info/snapshot/read
eski kaydı temizlemez; hiçbir click, upload veya download otomatik replay edilmez.
Worker toplamı en fazla 30 saniye ve parent kalan süresi; host varsayılanı
300 saniyedir. İptalde container durduğu için tarayıcı ve indirme süreçleri de biter.

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 scripts/build_browser_image.py
python3 scripts/browser_actions_smoke.py
```

Normal Rust testleri model/Docker gerektirmez. Ayrı kabul gerçek Chromium ve
native Rust HTTP fixture'ı kullanır: fill/select/click/scroll/Enter sonuçları,
Unicode, file input'tan daha sonraki fetch'e birebir binary upload, exact download
bytes, önerilen filename izolasyonu, oversize reddi, stale hedef ve sunucunun
isteği gördüğü anda in-flight iptal. CLI ayrı izinler, host upload/export,
0600/no-overwrite ve hedeflerin aktarımda korunmasını doğrular. 4.2'nin gezinme,
DOM/AX/screenshot ve toparlanma kabulü de aynı imajda tekrar çalışır.
Kanıtlar `artifacts/browser43-docker-acceptance.json` ve
`browser43-downloaded.bin` içindedir; Git'e eklenmez. Fixture production imajına
konmaz; canlı model çağrısı 0.

2026-10-19: 426 Rust testi (7 yeni), format, Clippy `-D warnings`, release workspace
bins/examples ve gerçek Chromium SDK/CLI kabulü geçti. Yeni normal testler eylem
ve yol preflight'ı, ayrı izin/türler, cancellation/fence/no-replay, yanlış reply
dosya yolu, private bounded read ve atomik no-overwrite/root confinement kapsar.
Gerçek kabul ayrıca disabled fieldset, server-side upload byte oraklı ve başladı
marker'ı görülen indirmeyi iptal ederek partial dosyanın yayımlanmamasını doğrular.
