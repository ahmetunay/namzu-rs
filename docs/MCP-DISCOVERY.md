# MCP 3.5 — Keşif, kataloglar ve bildirimler

`namzu_mcp::discovery` bounded/typed keşif sonuçlarını, `schema` şema metadata
kontrollerini sağlar. Manager dört bağımsız katalog yönetir: tools, resources,
prompts ve resource templates. Normal model `Tools`/exec/TUI/Pal adaptörü 3.6'dadır.

```sh
namzu-rs mcp discover SERVER tools --approve
namzu-rs mcp discover SERVER resources --approve
namzu-rs mcp discover SERVER prompts --approve
namzu-rs mcp discover SERVER templates --approve
namzu-rs mcp discover SERVER tools --approve --details
```

Mevcut kullanıcı/proje/profil seçicileri ve allow/deny/approval tavanları aynıdır.
Connect ayrı izin ister. Her liste sayfası ayrı immutable Plan ve tek kullanımlık
Permit'ten geçer. `--approve` bu invocation'ın bağlantı ve liste sayfalarını
onaylar; deny veya eksik server capability'yi geçemez. Varsayılan timeout 300
saniyedir ve tüm sayfaların ortak bütçesidir. Komut araç çalıştırmaz, resource
okumaz, prompt getirmez, endpoint/şema URI'si açmaz ve model kullanmaz.

Varsayılan JSON çıktı server/kind/count ve öğe kimlikleridir. `--details`, açıkça
istenmiş untrusted tanım/açıklama/URI/şema/extension verilerini de döndürür. Bunlar
sistem talimatı veya güven/onay kanıtı değildir. Hata gövdeleri/RPC error message
ve private stderr loglanmaz; partial sonuç başarı çıktısına konmaz.

## SDK akışı

- Önce 3.4 `prepare_connect → permit/approve → execute` ile açık bağlantı kurulur.
- `manager.discover(server, Kind, &context, |plan| host_admission(plan))`
  sunucu capability'sini, her sayfada policy/trust/snapshot'ı ve context'i denetler.
  Callback'te Plan server/method/params okunabilir. Dönen grant'ın istenen sayfaya
  ait olması ayrıca kontrol edilir; başka bir RPC grant'ı kullanılamaz.
- `catalog(server, kind)` yalnızca tamamlanmış ve geçerli mevcut katalog verir.
  Kimlik `(server, kind, identity)` ile sınırlıdır: iki server'ın aynı tool adı
  birbirini ezmez. Cache'in dışarı verilen kopyası içerideki tanımı değiştiremez.
- `prepare_tool_call`, `prepare_resource_read`, `prepare_prompt_get` mevcut
  katalogdaki kimliği seçer ve katalog revision'ına bağlı normal bir Plan üretir.
  Policy yine karar verir. Bildirim veya refresh'ten sonra eski grant reddedilir.
  Prompt argümanları string, ilan edilmiş ve required alanları tamam olmalıdır.
- `execute` ham RPC sonucunu döndürür. `tool_result`, `read_result`, `prompt_result`
  bunu typed MCP içeriklerine doğrular. RPC error ile `isError` araç sonucu
  ayrıdır. Bu katman bu verileri core/model mesajına kendiliğinden dönüştürmez.

Düşük seviyeli `prepare_call` açık/manual RPC API'sidir ve katalog gerektirmez.
Model adaptörü 3.6'da katalog bağlı girişleri kullanacaktır. 3.5'in şema kontrolü
argümanların veya structuredContent'ın bütün JSON Schema kurallarını sağladığını
iddia etmez; tam instance doğrulaması 3.6 entegrasyonunun işidir.

## Sayfalama ve atomic yayın

Pager başlangıçta cursor göndermez. Sonraki `nextCursor` aynen geri gönderilen
opaque bir string'dir; URI veya sayı olarak yorumlanmaz. Missing nextCursor
bitiştir; boş/null/nonstring/1024 byte'tan uzun/kontrol karakterli cursor hatadır.
Tekrarlanan cursor, A→B→A döngüsü ve sayfalar arasında aynı item kimliği reddedilir.

Bir liste en fazla 32 RPC sayfası, 1024 öğe ve 4 MiB encoded page verisidir.
33. sayfa isteği gönderilmeden bütçe reddedilir. Her sayfa ayrıca 3.1 wire
message/depth/node/string sınırından geçer. Manager'ın tüm kataloglarının encoded
boyutu ortak 8 MiB ile sınırlıdır; dolması diğer geçerli server kataloglarını
sessizce silmez. Bu sınırlar process için OS RAM kotası değildir; Rust veri
structures/serialize ve geri verilen snapshot kopyalarının ek maliyeti vardır.

Refresh hedef kataloğu başta geçersiz kılar. Tüm sayfalar doğrulanmadan yayın yoktur.
Bozuk son sayfa, RPC hatası, izin reddi, iptal, deadline, limit veya ilgili
list_changed bildiriminde prefix cache'e girmez. Başarısız Pager zehirlenir;
finish çağrısıyla kısmi sonuç alınamaz. Kendiliğinden refresh/retry yapılmaz.
Konfigürasyon/trust değişimi veya bağlantı hatası eski katalogları kaldırır.

## İsim, metadata ve şema sınırları

Tool/prompt/argument adı 1–128 ASCII alfanümerik, `_`, `-`, `.` karakterleridir.
Resource/template display name 1–256 kontrol karaktersiz Unicode olabilir.
URI/template 4096 byte, geçerli scheme başlangıcı ve whitespace/control reddiyle
opaque tutulur. Template brace yapısı kontrol edilir; RFC 6570 genişletmesi bu
katmanda yapılmaz. Resource kimliği URI, template kimliği uriTemplate, diğerleri
name'dir. Prompt başına en fazla 64 benzersiz argument, description en fazla
16 KiB'dir. MIME string'i bounded/kontrol karaktersiz olmalıdır.

Tool inputSchema ve varsa outputSchema JSON nesnesi olmalı; kök `type: object`
gerekir. Her şema 64 KiB, 4096 düğüm, derinlik 24 ve string başına 16 KiB ile
sınırlıdır. Bilinen type/required/properties/defs/items/combinator/numeric/size ve
annotation alanlarının metadata şekilleri kontrol edilir. Duplicate required
ve type girdileri, bozuk subschema ve geçersiz yerel referans reddedilir.
`$ref`/`$dynamicRef`/`$recursiveRef` yalnızca aynı dokümandaki mevcut object/bool
JSON Pointer hedefini gösterebilir. External HTTP/file referansı ve anchor-only
referans reddedilir. Recursive yerel metadata genişletilmeden saklanabilir.
Şema veya instance evaluation motoru/network resolver çalıştırılmaz. Pattern,
format, bilinmeyen keyword/extension ve hint'ler veri olarak korunur; izin açmaz.
Bu kontrol bütün JSON Schema draft'larının metaschema doğrulaması değildir.

Typed sonuçlar en fazla 128 content/message öğesidir ve wire limitleri geçerlidir.
Resource contents tam bir text veya blob alanına sahip olmalıdır. Prompt rolü
yalnızca user/assistant olabilir; system role reddedilir. Text/image/audio/link/
embedded resource biçimleri doğrulanır. Blob/base64/media opaque'tur; decode,
dosya yazma, link açma veya URI fetch yapılmaz. Rich model içerik dönüşümü 3.6'dadır.

## Bildirim yaşam döngüsü

Stdio `call_observed` sequential çağrı sırasında gelen bildirimleri kaybetmeden
host'a aktarır. Eski `call` API'sinin bildirimleri yok sayma sözleşmesi korunur.
Manager stdio callback'ini ve HTTP/SSE pop_event kuyruğunu aynı sınırlı notification
kuyruğuna bağlar: toplam 64 mesaj/2 MiB encoded içerik; stdio capture için de
aynı bound vardır. Taşma generation'ı kapatır, katalogları geçersiz kılar.

Server listChanged capability'si true ise tools/prompts bildirimi o kataloğu,
resources bildirimi resources ve templates kataloglarını geçersiz kılar.
Diğer kataloglara veya server'lara yayılan otomatik invalidation yoktur. İlan
edilmemiş listChanged, progress, resource updated ve bilinmeyen bildirimler
bounded inert veri olarak `pop_notification()` ile alınır. Bunlar otomatik RPC,
URI açma, araç/Pal işi veya yerel request cancellation başlatmaz.

İsteğe bağlı açık bekleme:

```text
plan = manager.prepare_notifications(server)  // policy target events/listen
permit = host admission(plan)
manager.execute(permit, context)              // Outcome::Events { supported }
manager.pop_notification()
```

Stdio bir bildirim bekler; HTTP retained GET SSE akışını sürer. GET 405,
supported=false'dur. Her bekleme açık context/permission taşır; deadline/iptalde
transport generation'ı kapanır. Gizli reader thread/timer/reconnect yoktur.
Host idle akışı sürmediğinde henüz okunmamış uzaktaki değişiklik bilinemez;
cache coherence yalnızca işlenmiş bildirim/snapshot'lar içindir. Sürekli pump,
normal ajan akışları ve kalıcı recovery/subscription 3.6/3.8 kapsamındadır.

## Kabul — 2026-10-18

40 yeni Rust testi: 13 payload/schema/budget testi, 22 gerçek stdio keşif testi,
3 gerçek loopback HTTP/SSE/GET testi, 2 CLI subprocess testi. Toplam 335 workspace
Rust testi. Dört katalog türü, typed tool/read/prompt sonuçları, opaque cursor,
cycle/duplicate/poison, 33. RPC öncesi limit, 8 MiB ortak cache bütçesi, bozuk
şema/isim/veri, eksik capability, per-page grant/deny, bildirim invalidation ve
stale grant, farklı server isim çakışması, notification backpressure ve sayfa
beklemesinde cancellation sınandı. File/HTTP resource URI otomatik açılmadı;
remote RPC hata metni CLI çıktısına eklenmedi. Eski stdio/HTTP TLS/proxy ve
3.4 güven/izin testleri workspace regresyonuna dahildir.

Format, locked workspace test, warnings-as-errors Clippy, workspace release ve
release CLI dört katalog kabulü çalıştırılır. Fixture server bağımsız Python
stdlib test kodudur; üretim istemci/manager/discovery Rust'tır. Canlı model
çağrısı 0. Üçüncü taraf canlı MCP/OAuth, Docker/PTY veya tam JSON Schema instance
uyumluluğu kabulü bu teslimde çalıştırılmaz.
