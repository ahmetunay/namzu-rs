# MCP 3.6 — SDK, CLI, TUI ve Pals

Bu yeni Rust ağacında uygulanmıştır. Canlı model/hesap gerektirmeyen kabul
sunucuları ve HTTP model fixture'ları kullanılır.

## Kullanım

Önce 3.4'teki `mcp add` / proje trust akışıyla sunucuyu açıkça kaydet.
Hiçbir sunucu otomatik seçilmez; çalışma klasöründeki config otomatik açılmaz.

```sh
namzu-rs exec 'Görevi yap' --cwd ./proje --model MODEL \
  --api-key-env OPENAI_API_KEY --mcp-server myserver --mcp-approve --json
namzu-rs tui --cwd ./proje --model MODEL --api-key-env OPENAI_API_KEY \
  --mcp-server myserver
namzu-rs mcp discover myserver tools --approve --agent-names
```

`--mcp-server` tekrarlanabilir (en fazla 8). `--mcp-profile`,
`--mcp-project` ve `--mcp-project-config` normal exec/TUI ve Pal seçenekleridir.
Açık proje seçimi, mevcut trust hash'i ve user/project/host politika kesişimi
korunur. Deny önceliklidir. Noninteractive `--mcp-approve`, bu koşumda izin
isteyen connect, keşif sayfaları ve ajan çağrılarını açıkça onaylar; kalıcı
policy değiştirmez. Bu bayrak olmadan Ask durumundaki bootstrap model
çağrısından önce durur. TUI gerçek onay diyaloglarını kullanır; bayrak TUI
onayını atlatmaz. Her araç onayı çağrı ID'si, adı ve argümanlarına bağlı tek
kullanımlık karardır. Varsayılan SDK NoopObserver onay yetkisi vermez.

`mcp discover ... --agent-names` her remote kimlik için modelin kullanacağı
kararlı ASCII alias'ı verir. Aynı remote isim farklı sunucu veya katalog türünde
çakışmaz; digest ve alias çakışması ayrıca reddedilir. Normal `--tool-allow` /
`--tool-deny` alias'lara da uygulanır. Tanımlar untrusted açıklamaları içerir;
izin ve sistem talimatı sayılmaz.

Pal oluştururken MCP seçimi profilin backend route'u içinde içerik hash'iyle
sabitlenir. MCP alias'ları **açık `--tool-allow` ile** çocuk profilinin tavanına
alınır. Mevcut CLI izinleri bunu yalnızca daraltabilir. Kaydedilmiş seçim yeni
koşumda geri yüklenir; onaylar geri yüklenmez. Config/trust değişirse profil
sessizce farklı sunucuya bağlanmaz. MCP aracı verilmeyen çocuk sunucu başlatmaz.
Normal oturum resume/TUI history de MCP seçimi ve config hash'ini kontrol eder.
Belirsiz/pending araç çağrıları otomatik tekrar edilmez.

## SDK ve veri sözleşmesi

`namzu_mcp::agent::AgentTools` mevcut bir `core::Tools` kümesini ve açıkça
oluşturulmuş `manager::Manager`'ı sarar. `load` connect ve keşif için host permit
callback'i ister. `Approvals` / `ApprovedObserver` hostun gerçek onay kararını
tek kullanımlık çağrı iznine taşır. Host `explicit` değerini yalnızca yetkili
karar için açmalıdır. SDK kendi başına kullanıcı onayı varsaymaz.

Araçlar `tools/call`, somut kaynaklar `resources/read`, prompt'lar `prompts/get`
üzerinden çağrılır. Prompt argümanları string'dir. Sonuç typed MCP yapısının JSON
metni olarak **tool sonucu** olur: text, media/base64, resource/link, blob,
structuredContent ve prompt rolleri veri olarak korunur. Uzak prompt rolleri
sistem/kullanıcı mesajına yükseltilmez. URI'ler açılmaz; görsel/ses render veya
ayrı model multimodal parçalarına dönüştürme bu teslimde yoktur. API sağlayıcısı
zaten tool çıktısını `{content,is_error}` zarfına alır; içerideki MCP JSON ayrıca
parse edilir. RPC error ayrıntıları model çıktısına taşınmaz. MCP `isError` normal
araç hatasıdır; bozuk transport/typed içerik/başarılı outputSchema ihlali turu
durdurur ve pending journal bırakır. Otomatik replay/reconnect yapılmaz.

## JSON Schema ve sınırlar

Input ve mevcut outputSchema, offline Draft 2020-12 validator ile compile
edilir. Argümanlar **tüm model araç grubu için ilk yan etkiden önce** doğrulanır;
çağrı anında da kontrol edilir. Başarılı tool sonucunda outputSchema varsa
structuredContent zorunludur ve modeli bilgilendirmeden önce doğrulanır.
Eski `Tools` implementasyonları default `validate` sayesinde uyumludur.

Şema 64 KiB/depth 24/4096 node; instance varsayılan wire sınırları
(1 MiB/depth 48/32768 node), tanımlar toplam 256 KiB ve local+MCP en fazla
128 function. Regex backtrack limiti 10000. Dış şema indirme kapalıdır; yerel
JSON pointer referansları ve instance altına inen recursive object şemaları
desteklenir. Instance tüketmeden dönen ref döngüleri reddedilir. Başka `$schema`
draft'ları ve anchor/dynamic ref senaryoları bu sınırlandırılmış arayüzün
uyumluluk vaadi değildir. `format`, 2020-12'nin varsayılan annotation davranışıdır.
Validation senkrondur: context önce/sonra kontrol edilir, CPU içinde zorla
preemption yoktur. Arbitrary düşmanca şemalar için ayrı OS process sandbox'ı
sağlanmaz; bu sınırlara rağmen pahalı şemalar dikkat gerektirir.

List-changed/config/trust değişikliği binding'i geçersiz kılar. Çalışan agent
sessizce yeni katalogla devam etmez. Resource template kataloğu 3.5 yönetim
API'sinde bulunur; ajan için otomatik URI template genişletmesi yoktur.
OAuth 3.7, reconnect/subscription toparlanması 3.8 kapsamındadır.

## Pal bilgisayarında stdio

`SharedGuestTools` sahipliği doğrulanan ComputerManager bilgisayarını yerel
araçlarla paylaşır. Manager `with_guest` ile açıkça bağlanır. MCP program/cwd
absolute guest yollarıdır; argv shell olmadan `docker exec --interactive`'e
gider. Seçilen env değerleri argv'ye yazılmaz, guest'e açık selector ile geçer.
Guest env host Docker ortamına eklenmez: private 0600 geçici `--env-file`
kullanılır, scope Drop'unda silinir; newline/CR/NUL değerleri reddedilir.
Host SIGKILL sırasında kalan dosya için OS temp temizliği gerekebilir.
Docker host sabit local socket, container ID'si tam ve ownership/confinement
her RPC öncesinde kontrol edilir. Sonradan guest bağlanan manager mevcut host
bağlantılarını/kataloglarını kapatır ve eski permitleri geçersiz kılar. Guest HTTP bu aşamada reddedilir; hosta
sessiz fallback yapılmaz. Runtime otomatik image çekmez/oluşturmaz.

İlk launch öncesi durable `mcp-active` pending kaydı tutulur. Bunun aynı canlı
Rust scope'a ait olduğunu gösteren izin bellektedir, diskten geri yüklenmez.
Normal kapamada stdio kapatılır ve container durdurularak uzak exec alt
süreçleri de sonlandırılır. Sağlıklı scope pending'i temizler; iptal/hata scope'u
inceleme için pending bırakır. Host SIGKILL gibi kesintiler otomatik replay
hakkı vermez; mevcut bilgisayar inspect/ack/recovery akışı gerekir. Linux
stdio process group temizliği tek başına Docker daemon'daki uzak süreçleri
öldürdüğü iddiası taşımaz; owned bilgisayar lifecycle'ı bu işi yapar.

## Tekrar üretme

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo build --release --workspace --bins --examples --locked
python3 scripts/mcp_smoke.py
python3 scripts/mcp_guest_smoke.py
```

İlk kabul gerçek Linux PTY'de bootstrap/tool allow/deny/cancel kontrol eder.
İkinci kabul mevcut pinned Debian base üzerinden `--network=none --pull=false`
ile native Rust MCP fixture image'ını açıkça oluşturur ve gerçek Docker owned
bilgisayarında dosya sonucu, temizlik ve belirsiz kaydın resume korumasını
doğrular. Ayrıca gerçek CLI Pal guest akışını loopback modelle çalıştırır. Hostta bu guest programının
kurulu olması gerekmez. Docker normal Rust testlerinin bağımlılığı değildir.

Doğrulanan sonuç: 353 Rust testi (18 yeni), format/Clippy/release; normal ve
Pal TUI için 6 MCP PTY vakası, mevcut 25 TUI + 4 Pal PTY regresyonu. Gerçek
Docker owned guest stdio/dosya/cleanup, uncertain resume, CLI Pal guest
ve guest env izolasyonu kontrolleri geçti.
Canlı model çağrısı 0.
