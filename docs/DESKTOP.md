# 4.1 — owned guest ekran ve girdi kontrolü

Linux Pal bilgisayarına açıkça etkinleştirilen X11/Xvfb masaüstü ve native Rust
X11/XTEST worker eklendi. Host ekranına bağlanma veya host input fallback yoktur.
Tarayıcı/DOM 4.2–4.3, model araçları ve TUI görsel entegrasyonu 4.4 kapsamındadır.

## İmaj ve SDK

```sh
python3 scripts/build_desktop_image.py
```

Script mevcut pinned Debian tabanından `namzu-desktop:local` imajını açıkça
oluşturur; Xvfb/XKB paketleri build sırasında kurulur. Proxy ayarları korunur,
HTTPS CA bundle sadece build secret mount'udur. Runtime otomatik image build/
pull yapmaz. Çalıştırmak için script'in verdiği **sha256 image ID** kullanılır.
Dockerfile kaynak Git yedeğine dahildir; yerel image/layer/volume değildir.
Başka cloud oturumunda image yeniden build edilip yeni ID açıkça seçilmelidir.
Apt paketleri build anındaki Debian deposundan gelir; base image digest'i
sabitken paket sürümleri ayrıca bir snapshot deposuna sabitlenmiş değildir.

```rust,no_run
use namzu_computer::{ComputerSpec, desktop::{ScreenConfig, Request}};
let mut spec = ComputerSpec::new("sha256:...explicit local image...");
spec.desktop = Some(ScreenConfig { width: 1024, height: 768 });
// manager.acquire(spec, &context)? -> owned Computer
// computer.desktop(Request::Screenshot, &context)?
// computer.desktop(Request::Key { keys: vec!["CTRL".into(), "a".into()] }, &context)?
```

Eski ComputerSpec JSON'larında `desktop` yoksa headless davranış korunur.
Varsayılan ScreenConfig 1024×768; genişlik 320–1920, yükseklik 240–1080 piksel.
Ekran tek X11 root'tur, sol üst (0,0); koordinatlar screenshot ile aynı ölçekte
fiziksel piksel kullanır. Resize, multi-monitor, DPI dönüşümü ve Wayland yoktur.
X server yalnızca guest Unix socket'inde :99 olarak dinler; TCP kapalıdır.
Xvfb container PID1 tarafından başlatılır ve server ölürse guest sona erer.
Aynı owned guest içindeki uygulamalar aynı X oturumunu paylaşır; bu bir
uygulamalar arası X11 güvenlik sınırı değildir.

## İşlemler ve sınırlar

`Computer::desktop(Request, TurnContext)` şu typed sonuçları verir:

| İstek | Sonuç / davranış |
| --- | --- |
| `info` | width, height ve root pointer_x/pointer_y |
| `screenshot` | PNG base64 + width/height; RGB8, tek frame |
| `move` | absolute x/y |
| `click` | absolute x/y, left/middle/right; press + release |
| `drag` | from_x/from_y → to_x/to_y; sol tuşla 16 adım |
| `scroll` | horizontal/vertical; eksen başına −20…20 tick, ikisi birden 0 olamaz |
| `key` | en fazla dört modifier + tek tuş; press/release |
| `type` | 1–1024 byte printable ASCII, newline ve tab |

Scroll negative: yukarı/sol; positive: aşağı/sağ. Uygulamanın bir tick'i kaç
satır/piksel olarak yorumlayacağı uygulamaya bağlıdır. `key` modifier isimleri
`CTRL`, `SHIFT`, `ALT`, `SUPER`; modifier'lar önce gelir ve tekrar edemez.
Son tuş printable ASCII karakteri veya `ENTER`, `TAB`, `ESC`, `BACKSPACE`,
`DELETE`, `LEFT/UP/RIGHT/DOWN`, `HOME/END`, `PAGEUP/PAGEDOWN`, `F1`…`F12` olur.
Örneğin `["CTRL","a"]`. Büyük harf ve US-layout noktalama karakterleri için
Shift gerektiğinde eklenip bırakılır. Bütün metin/tuş planı ilk XTEST input
öncesi guest keymap'inde çözümlenir; desteklenmeyen giriş kısmen yazılmaz.

Bu ilk backend US XKB ASCII metin girişidir: Türkçe/diğer Unicode karakterleri,
IME, clipboard paste ve tuşu çağrılar arasında basılı tutma desteklenmez.
Unicode input sessizce ASCII'ye dönüştürülmez; host preflight'ta reddedilir.
Shell, dosya veya browser DOM metin API'si gibi başka izinler için fallback
uygulanmaz. Type içeriği worker'a stdin JSON ile gider; shell komutu veya worker
argv'si değildir. Debug request biçimi içeriği redakte eder.

Screenshot cursor sprite'ını içermez; pointer konumu info'dadır. PNG en fazla
8 MiB, wire response en fazla 12 MiB; yalnızca screenshot bu geniş çıktı
bütçesini kullanır. Dosya/shell wire sınırı 512 KiB ve dosya 64 KiB olarak kalır.
Genel süreç çalıştırıcısının seçilebilir üst sınırı 16 MiB'dir; desktop worker
12 MiB ister, diğer çağrıların kendi küçük bütçeleri değişmez.
Host PNG header, ölçü, renk/bit-depth, tek frame ve decode/CRC/trailer doğrulaması
yapar. Yalnızca 24-bit truecolor root ve 24/32-bit RGB pixel formatları kabul
edilir. Uygulama semantiğinin tamamlandığı değil, X server'ın olayları kabul
ettiği bildirilir; UI sonucunu beklemek caller'ın kontrolüdür.

## CLI host kontrolü

Önceden oluşturulmuş aktif guest Pal için:

```sh
namzu-rs pal --image sha256:LOCAL_IMAGE computer PAL_ID create --desktop
# isteğe bağlı: --width 800 --height 600
```

Mevcut normal Pal CLI model/config/state seçenekleri kullanılır. Masaüstü
isteğini JSON dosyasına koy:

```json
{"action":"click","x":100,"y":120,"button":"left"}
```

```sh
namzu-rs pal --allow-input computer PAL_ID desktop request.json
```

Info: `{"action":"info"}`. Screenshot:

```json
{"action":"screenshot"}
```

```sh
namzu-rs pal computer PAL_ID desktop screenshot.json --output NEW_FILE.png
```

Screenshot dosyası 0600 olarak create-new yazılır; mevcut dosya üzerine yazılmaz.
Stdout PNG/base64 veya typed text içeriği basmaz; sadece metadata verir.
Input için her koşumda **`--allow-input`** gerekir. Bu flag Pal profiline veya
model tool listesine kalıcı girdi yetkisi eklemez. Input izni, JSON, geometri ve
screenshot/output eşleşmesi lease/start/stop etkisinden önce denetlenir.
Reddedilen input mevcut çalışan masaüstünü durdurmaz. Başarılı desktop komutu
container'ı çalışır bırakır; `computer stop` / `cleanup` açık yaşam döngüsüdür.
Hatalı command/IO sonrasında mevcut persistent guard container'ı durdurabilir.

## İptal, belirsiz etki ve izolasyon

Her işlem mevcut owner/label/image/confinement doğrulamasını kullanır. Worker
ayrı süreçtir; en fazla 15 saniye ve host TurnContext'in kalan süresiyle sınırlı.
X bağlantısı ilk açılışta en fazla iki saniye hazır olmayı bekler. Default host
turn sınırı 300 saniyedir. İptal veya belirsiz transport/worker hatasında owned
container durdurulur; uzak Xvfb/exec süreçleri host Docker CLI öldürülmesine
bırakılmaz. Input hata/iptalinde durable `desktop_input` pending kaydı korunur;
yeni input/write/shell için mevcut inspect/ack akışı gerekir. Otomatik tıklama
veya yazma tekrarı yoktur.

Info/screenshot read-only sınıftır; pending input varken inceleme için
kullanılabilir (gerekirse bilgisayar açıkça yeniden başlatılır). Bunların başarısı
eski pending kaydını temizlemez. Normal stdio MCP'nin canlı scope/pending ve
Computer cleanup kuralları korunur. Raw X socket hosta mount edilmez, ağ none,
read-only rootfs, non-root user, cap-drop ve CPU/RAM/PID limitleri devam eder.
Host OS input kontrolü ve canlı üçüncü taraf uygulama uyumluluğu iddiası yoktur.

## Tekrar üretme

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 scripts/build_desktop_image.py
python3 scripts/desktop_smoke.py
```

Normal Rust testleri Docker/model gerektirmez. Ayrı gerçek Docker kabulü
native Rust GUI penceresinin bilinen pikselini, click sonrası renk değişimini,
karakter/CTRL olaylarını, scroll/drag/pointer konumunu ve stop/restart/cleanup'ı
bağımsız orakllarla kontrol eder. CLI izinsiz input'un etkisizliği ve screenshot
create-new/0600 yazımı ayrıca sınanır. Test fixture imajı üretim desktop imajını
native GUI örneğiyle genişletir; fixture üretim imajına eklenmez. Model çağrısı 0.

## Bu aşamanın kabulü

2026-10-18: workspace'te 406 Rust testi (10 yeni), format, Clippy `-D warnings`
ve release workspace bins/examples derlemesi geçti. Gerçek Docker SDK testinde
bilinen RGB pikseli, tıklamayla renk değişimi, ASCII/Shift/Ctrl+A olayları,
scroll ve drag/pointer sonucu, stop/restart ve owned cleanup doğrulandı.
Release CLI'de izinsiz input çalışan masaüstünü değiştirmedi; açık izinli move,
screenshot PNG/0600 ve mevcut dosyaya yazmama kontrolleri geçti.
Önceki Pals kabulünün 4 PTY ve 9 gerçek Docker kontrolü de desktop image ile
headless modda geçti; canlı model çağrısı 0.

Yerel kanıtlar `artifacts/screen-before.png`, `screen-after.png` ve
`desktop41-pals-regression.json` içindedir; üretilen bu dosyalar Git'e eklenmez.
Yukarıdaki script'ler bunları kaynak koddan tekrar üretir.
