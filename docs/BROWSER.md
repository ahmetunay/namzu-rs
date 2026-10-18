# 4.2 — owned Chromium gezinme ve sayfa inceleme

Tarayıcı, Pal'ın sahipli Linux container'ında, 4.1 Xvfb ekranında görünür Chromium
olarak çalışır. Native Rust `namzu-browser` worker yalnızca sabit üç işlemi
kabul eder: `info`, `navigate`, `snapshot`. Rastgele JavaScript/CDP komutu,
host tarayıcısına bağlanma veya kullanıcı profilini içeri alma yoktur.
Sayfa eylemleri/dosya indirme-yükleme 4.3; model araçları/TUI görsel entegrasyonu
4.4 kapsamındadır.

## İmaj ve yaşam döngüsü

```sh
python3 scripts/build_browser_image.py
# Çıktıdaki sha256 image ID'yi açıkça seç:
namzu-rs pal --image sha256:LOCAL_IMAGE computer PAL_ID create --desktop --browser
```

Aktif guest Pal ve normal CLI state/config seçenekleri gerekir. `--browser`,
`--desktop` gerektirir. CLI browser spec için 768 MiB RAM, 128 PID ve 256 MiB
/tmp seçer; varsayılan ekran 1024×768, `--width`/`--height` ile değiştirilebilir.
SDK'de `ComputerSpec.browser = true`, `desktop = Some(ScreenConfig)` ve en az
512 MiB RAM / 128 PID gerekir. Eski spec'lerde browser yoksa false olur;
headless ve yalnızca desktop davranışı korunur.

Container PID1 Xvfb ardından Chromium'u başlatır; ikisinden biri biterse
container sona erer. Start/restart yeni geçici Chromium profili ve about:blank
sayfasıyla başlar. Cookie/login/history profilini resume etme yoktur; profil
/tmp'dedir, workspace volume'üne yazılmaz. `computer stop` tarayıcıyı da durdurur;
`cleanup` yalnızca eşleşen owned container ve volume'ü kaldırır.

Tarayıcı **ağsızdır**: mevcut `--network=none` korunur. Aynı guest içindeki
127.0.0.1 HTTP sunucularına erişir; internet/host özel ağına erişmez. API'nin
http(s) URL kabul etmesi egress yetkisi değildir. Bu aşama internet siteleri,
kullanıcı hesabı login'i veya model kotasıyla doğrulanmamıştır. Yerel HTML'yi
sunmak için guest içinde ayrıca HTTP sunucusu başlatılabilir; worker kendi
başına sunucu veya shell başlatmaz. Gelişmiş ağ/sandbox politikaları 5.4'tedir.

Debian tabanı digest ile sabittir; Chromium/Xvfb/font paketleri build anındaki
Debian deposundan gelir, sürümleri snapshot deposuna sabitlenmez. Build proxy ve
CA ayarlarını korur; CA yalnızca secret mount'udur. Runtime otomatik image
build/pull yapmaz. Dockerfile/script Git yedeğindedir, image/layer/volume değildir.
Yeni oturumda yeniden build ve yeni sha256 ID seçimi gerekir.

Chromium non-root, read-only rootfs, cap-drop/no-new-privileges, kaynak limitleri
ve ağsız container içinde `--no-sandbox` ile çalışır. Chromium'un kendi user
namespace sandbox'ı bu backend'de kullanılmaz; OS izolasyon sınırı owned Docker
container'ıdır. CDP guest loopback 127.0.0.1:9222'de kalır; host portu yayımlanmaz.
Aynı guest'teki yetkili shell/programlar aynı tarayıcıyı kontrol edebilir;
uygulamalar arasında ayrı bir güvenlik sınırı iddiası yoktur.

## CLI ve SDK

İstek dosyası:

```json
{"action":"navigate","url":"http://127.0.0.1:8765/"}
```

```sh
namzu-rs pal --allow-navigate computer PAL_ID browser request.json
```

Girdi etkisi olan gezinme her komutta açık `--allow-navigate` ister. Flag profil
veya model tool listesine yazılmaz; `--allow-input` ayrı masaüstü iznidir.
JSON/URL/izin/opt-in kontrolü lease/start/stop öncesinde yapılır; reddedilen
istek çalışan tarayıcıyı durdurmaz. Başarılı browser komutu container'ı çalışır
bırakır. Info ve snapshot gezinme izni istemez:

```json
{"action":"info"}
```

```json
{"action":"snapshot"}
```

```sh
namzu-rs pal computer PAL_ID browser inspect.json
# Ekran görüntüsü, 4.1'in aynı ekran/PNG sözleşmesiyle:
namzu-rs pal computer PAL_ID desktop screenshot.json --output NEW_FILE.png
```

SDK: `computer.browser(browser::Request::Info, &context)`,
`Request::Navigate { url }`, `Request::Snapshot`. Typed `Reply::Info`,
`Reply::Navigated` veya `Reply::Snapshot` döner. Host beklenen yanıt türünü ve
bütün veri sınırlarını pending kaydını temizlemeden doğrular. CLI açıkça istenen
sayfa verisini JSON olarak verir; içerik güvenilmez sayfa verisidir, talimat veya
çalıştırılacak kod sayılmaz. `Request` Debug biçimi URL'yi redakte eder;
worker hata gövdeleri sayfa/CDP/URL ayrıntısını loglamaz.

## Gezinmenin tamamlanması

Yalnızca credentials içermeyen http/https URL'leri (en fazla 2048 byte) veya
`about:blank` kabul edilir. `file:`, `data:`, `javascript:`, `chrome:` ve kontrol
karakterleri preflight'ta reddedilir. Gezinme bir kez gönderilir. Yeni belge
navigasyonunda CDP loader ID'sine bağlı `load` olayı beklenir; response'dan önce
gelen olaylar da korunur. Aynı belge fragment gezintisi loader üretmez ve
komut yanıtıyla tamamlanır. HTTP redirect sonrası gerçek URL/title döner.
HTTP 404/500 tarayıcının yüklediği bir sayfadır; HTTP başarı kodu garantisi yoktur.
`load`, SPA/network-idle veya animasyon/boya tamamlanması garantisi değildir;
sayfa içeriği ve screenshot sonucunu beklemek caller'a aittir.

Download'a dönüşen, CDP hata dönen veya tamamlanması belirsiz gezinme başarısız
sayılır. Container durdurulur, durable `browser_navigate` kaydı korunur;
inspect/ack olmadan yeni navigate/write/shell yapılamaz. Otomatik retry/replay
yoktur. Info/snapshot readonly inceleme olarak pending kaydını temizlemeden
kullanılabilir; durmuş bilgisayarı açıkça yeniden başlatmak gerekebilir.
Bilinen readonly inceleme hatası yeni etki olarak tutulmaz; bozuk guest protokolü,
engine transport hatası, iptal veya host deadline container stop/pending korumasını kullanır.

## Snapshot kapsamı ve sınırları

- `page`: URL, title, frame ID, loader ID. Başlık/URL en fazla 2048 byte,
  kimlikler 128 byte. Kimlikler navigasyon/yeniden başlatmadan sonra geçersizdir.
- `dom`: ana belge normal DOM ağacının preorder sırasıyla en fazla 256 düğümü;
  node/backend ID, parent ID, node type/name/text ve en fazla 16 attribute çifti.
  Text 1024, attribute adları 256/değerleri 512 byte'a sınırlıdır.
- `accessibility`: Chromium'un ana frame AX ağacından en fazla 256 düğüm;
  role/name/value, ignored, backend DOM ID ve child ID'ler. Name/value 1024 byte.
  Bazı AX düğümlerinin DOM karşılığı olmayabilir; kırpma sonrası dışarıda kalan
  child ID'ler bulunabilir. Görünürlük/clickability garantisi vermez.
- İki ağaç ve metadata ortak 64 KiB UTF-8 metin bütçesi kullanır. Alan/düğüm/metin
  kırpıldığında `truncated: true`; UTF-8 karakteri ortasından bölünmez.
- Raw CDP frame/message 4 MiB, discovery body 64 KiB, normalize guest wire 512 KiB ile
  sınırlıdır. Raw/wire aşımı güvenli hatadır; sınırsız DOM stream edilmez.
- Shadow root, child/cross-origin iframe içeriği, canvas/OCR/PDF ve tam rendered
  metin çıkarımı bu sürümün kapsamı değildir. DOM markup/script text içerebilir;
  görünür sayfa metni için AX ve gerçek screenshot ayrıca değerlendirilmelidir.

DOM ve AX alımından önce/sonra root URL/loader kontrol edilir; belge değişirse
karma snapshot reddedilir. Aynı belge içindeki JS mutasyonları için atomik
snapshot garantisi yoktur. JavaScript çalışan sayfalar kendi değişikliklerini
yapabilir; readonly burada worker'ın navigate/input/evaluate göndermemesidir.
İnceleme kullanıcı/sayfa içeriği ve form değerlerini içerebilir; gizli değerleri
otomatik keşfetme veya dışarı gönderme işlevi değildir.

Worker guest loopback'ten tek page target keşfeder; ikinci sekme/popup varsa
rastgele hedef seçmez, hata verir. Discovery'nin döndürdüğü remote host/port,
query/credentials/yanlış endpoint reddedilir. Tek uçuşlu ID eşlemesi, 256 olay / toplam 1 MiB bildirim
kuyruğu ve sınırlı WebSocket çerçeveleri vardır. Socket IO en fazla beş saniye,
worker toplamı host context'in kalan süresi ve 30 saniyeyle sınırlıdır.
Varsayılan host turn bütçesi 300 saniyedir. Hazır olma sırasında discovery tekrar
edilebilir; başlatılmış gezinme tekrar edilmez.

## Tekrar üretme ve kabul

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 scripts/build_browser_image.py
python3 scripts/browser_smoke.py
```

Normal Rust testleri Docker/Chromium/model gerektirmez. Gerçek Docker kabulü
ayrı native Rust HTTP fixture'ıyla görünür Chromium'da gezinme, redirect,
fragment, DOM başlık/metin/attribute, AX buton/label/value ve X11 screenshot
piksel rengini doğrular. Büyük DOM'un kırpılması, hatalı gezintinin stop/pending
kaydı, inspect/ack ve temizlik de kontrol edilir. CLI açık gezinme izni ve
izinsiz/hatalı URL'nin çalışan tarayıcıyı bozmaması ayrıca sınanır.
Üretilen kanıtlar `artifacts/browser42-docker-acceptance.json`,
`browser-snapshot.json`, `browser-screen.png` içinde kalır; Git'e eklenmez.
Fixture HTTP server production image'da bulunmaz. Canlı model çağrısı 0.

2026-10-18 kabulü: 419 Rust testi (13 yeni), format, Clippy `-D warnings` ve
release workspace bins/examples derlemesi geçti. Gerçek görünür Chromium'da
SDK ve CLI kabul script'i yukarıdaki bütün kontrolleri geçti. HTTP discovery
keep-alive, erken load olayı, yanlış ID/remote endpoint, notification bütçeleri,
fragment metadata, UTF-8 kırpma, opt-in/URL sınırları ve cancel/pending davranışı
normal testlerle ayrıca doğrulandı.
Önceki Pals kabulünün 4 PTY ve 9 gerçek Docker kontrolü de browser image'ın
headless modunda geçti. PTY test harness'i process exit'ten sonra kuyrukta kalan
son terminal çıktısını okumadan restoration assert'i yapmayacak şekilde
düzeltildi. Üretim guest/desktop/browser binary'lerinin SHA-256 değerleri build
manifestine yazılır; kabul script'i güncel release ile eşleşmeyi de kontrol eder.
