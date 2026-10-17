# Pals 2.6 kabul — 2026-10-17

Planlanan 2.1–2.6 Pals kapsamı bu kaynak ağacında tamamlandı: SDK çocuklar ve
paralel ekipler, özel kimlik/görev/session, lease'li mesaj işleme, sahip olunan
Rust guest bilgisayarlar ve açık CLI/TUI akışları. Bu, Namzu'nun kalan MCP,
tarayıcı, yetenek paketleri veya masaüstü modüllerinin bittiği anlamına gelmez.

## Sonuçlar

| Kontrol | Sonuç |
| --- | --- |
| `cargo fmt --all -- --check` | Geçti |
| `cargo test --workspace --locked` | **187 geçti**, 0 başarısız/atlandı |
| Workspace Clippy, all-targets, warnings-as-errors | Geçti |
| Workspace release build | Geçti |
| Mevcut TUI gerçek Linux PTY regresyonu | **25/25** |
| Yeni Pal gerçek Linux PTY | **5/5**: dört host akışı, bir gerçek guest |
| SDK gerçek Docker kabulü | **15/15** |
| Yeni CLI/TUI gerçek Docker akışları | **9/9**; guest PTY bu sete de dâhil |
| Canlı model/kota çağrısı | **0** |

Önceki 174 teste 13 test eklendi: altı gerçek CLI/loopback entegrasyonu,
bir Pal TUI host testi, iki host binary aktarım sınırı testi, iki computer
lifecycle testi ve iki scoped mailbox/guard testi. Gerçek HTTP fixture'ları
sağlayıcı adaptörünü ve SDK araç döngüsünü çalıştırır; gerçek model başarısı
veya yeni performans karşılaştırması ölçmez.

## Davranış oraklları

- Altı ayrı Pal aynı CLI çağrısından çalışır. İlk iki HTTP sağlayıcısının aynı
  anda aktif olması bariyerle zorlanır; sonuçların girdi sırası ve tüm kalıcı
  task Complete kayıtları doğrulanır. Paylaştırılan toplam adım/araç bütçesi,
  duplicate Pal ve yetersiz bütçenin yayınlamadan reddi kontrol edilir.
- Profilin yazma izni ile mevcut CLI bayrakları kesişir. İki farklı yöndeki
  yetki yükseltme denemesinde dosya oluşmaz. Gerçek yazmalı çağrı 42 dosyasını
  üretir; followup yeni task/session açar, eski session birebir aynı kalır.
- Model rotası/owner uyuşmazlığı model çağrısından önce reddedilir. Mesaj ID
  dedup, farklı payload çatışması, FIFO, ack, yeniden açınca boş pump ve eski
  mailbox task'ını ham `run` ile tekrar çalıştıramama doğrulanır.
- Scoped executor host callback'i lease'i bitirince, pending checkpoint korunur
  fakat dosya etkisi oluşmaz; NeedsInspection olur. Guard'ın ret kararı host
  onayıyla aşılamaz. Önceki beş gerçek süreç çökme testi de geçmeye devam eder.
- Host binary upload 64 KiB, regular file, symlink/FIFO reddi; yeni dosyaya 0600
  binary export ve overwrite reddi test edilir. Kalıcı computer lease'i setup
  iptalinde veya host panic'inde dosyaları korur ve remote işi durdurur.
- Pal PTY: F5 geçmişten followup; F6 model değişimini reddetme; yazma onayı,
  ret ve aktif HTTP iptali. Her kapanışta termios/alternate screen geri yüklenir.
- Docker CLI: create/start/restart/stop; binary upload/download; çalışan kalıcı
  guest'e SDK görevi ve sonrasında stop; mesaj pump'ından guest araçları ve ack;
  gerçek guest TUI yazma onayı; iki geçici guest'li paralel ekip ve cleanup.
- Guest shell'in `ready` dosyasına **gerçekten** yazdığı gözlendikten sonra CLI'ye
  SIGTERM gönderilir. Task Cancelled, computer Stopped/pending kalır; yeni model
  çalışması reddedilir. Açık download ile `once` etkisi okunur, sonra host ack
  yapılır. Testin sahip olduğu tüm container/volume isimleri tek tek yoklanıp
  cleanup sonrası bulunamadıkları doğrulanır. Başka kaynaklar silinmez.
- SDK'nin 15 Docker kontrolü RAM/CPU/PID kernel sınırları, network/capability/
  host mount/credential izolasyonu, kaçan guest çocuklar, timeout/çıktı limiti,
  gerçek client SIGKILL/pending koruması ve sahiplik sınırlarını tekrar doğrular.

## Çalıştırılan image

- Guest protokolü: **1**.
- Yerel image ID:
  `sha256:42e796bab54b18db2ad68d0b6cb97ff326e9bfeb457f4f5de93db7f0a8a74b18`.
- Build script'inin image'e koyduğu guest binary SHA-256:
  `9e9d767319eea93749553dfef29c9d73a6daf1cdda4d043a2f11bb8458be8e51`.
- Docker 28.4.0, vfs; runtime network none. Runtime pull/build yapmaz.

Tekrar üretim komutları [PALS-CLI.md](PALS-CLI.md) içindedir. Yerel makine
kanıtları ignored `artifacts/pals26-acceptance.json`, `pals26-pty.json`,
`pals26-sdk-docker.json`, `pals26-validation.json` dosyalarında tutulur. Kaynak ve
bu kabul belgesi GitHub'a gider; özel state/session/kimlik bilgileri gitmez.

## Kapsam sınırı

Açık `pal` CLI grubu ve SDK Pals akışlarını başlatır; normal `exec/tui` içinde
modelin kendiliğinden Pal araçları keşfetmesi uygulanmış sayılmaz. Otomatik mesaj
işleme hostun başlattığı sonlu pump'tır; sürekli daemon değildir. Ekip yayınlama
çok-Pal transaction değildir. Hard disk kotası, genel ağ erişimi ve görsel
bilgisayar/tarayıcı bu teslimde yoktur. Container ortak kernel kullanır;
bağımsız VM veya host admin'e karşı güven sınırı iddiası yoktur. İptalden sonra
metadata/20 saniyelik cleanup dönüş süresini uzatabilir; tamamlanan etkiler geri
alınmaz. Tüm kullanım ve toparlanma sınırları ilgili CLI/computer belgelerindedir.
