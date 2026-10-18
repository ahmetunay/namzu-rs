# MCP 3.8 — toparlanma, abonelikler ve birlikte çalışabilirlik

Kalıcı tool-effect kaydı, açık reconnect, bağlantı nesline bağlı izin/katalog/
abonelikler ve bağımsız stdio/HTTP fixture'ları eklendi. Bu teslim otomatik araç
tekrarı, subscription restore veya interaktif login başlatmaz.

## Kesilen araç çağrısı

Normal ajan, CLI ve SDK manager `tools/call` göndermeden önce özel diske bir
pending kaydı yazar. Yanıt alınırsa (correlated RPC error dahil) kayıt kaldırılır.
Transport/observation hatası, iptal veya bilinmeyen sonuçta kayıt uncertain olur.
Host SIGKILL ile kapanırsa işletim sisteminin bıraktığı owner lock üzerinden
pending kaydın artık aktif olmadığı anlaşılır. Process PID'sinin tekrar
kullanılması izin vermez; owner kimliği kriptografik rastgele 128 bit'tir.

```sh
namzu-rs mcp recovery list
# Önce ilgili oturumu ve sunucudaki gerçek etkileri incele.
namzu-rs mcp recovery acknowledge RECORD_ID --inspected
```

Acknowledge bir yerel fence kaldırır; çağrıyı tekrarlamaz, etkileri geri almaz ve
uzakta devam eden işi öldürdüğünü iddia etmez. Aktif pending owner varken
acknowledge reddedilir. Yanıtı belirsiz bir kaydı config/manager/process yenileyerek
atlamak mümkün değildir: aynı target'a yeni connect ve yeni tool çağrısı kayıt
incelenene kadar reddedilir. Farklı canlı owner'ların devam eden pending
çağrıları birbirinin crash kaydı sayılmaz. Tek owner kendi pending çağrısının
üzerine ikinci çağrı ekleyemez.

Durable kayıtlar `STATE/mcp/recovery/records/` altında 0600, üst dizinler 0700;
atomik/sync yazma ve symlink/FIFO/owner/mode kontrolleri PrivateStore'dandır.
Kayıt sadece id, owner id, target hash, server adı, `tools/call`, argümanların
SHA-256 hash'i, zaman ve uncertain flag içerir. Tool argümanı, URI/endpoint, argv,
kimlik bilgisi veya replay payload saklanmaz. Hash düşük entropili girdilerin
bilinmesini engelleyen bir şifreleme sistemi değildir. Durum raporu owner id'yi
de yayımlamaz.

Target; server adı, açık transport/OAuth ayarları ve host/guest konumundan
türetilir. Policy değişimi fence'i değiştirmez. Owned Pal guest için container
kimliği ayrı scope'tur; başka Pal bilgisayarını yanlışlıkla bloke etmez. Özel
SDK GuestStdio implementasyonları `recovery_scope` ile kararlı, benzersiz guest
kimliği sağlamalıdır (default `custom-guest`). Farklı transport/client ayarı
farklı target sayılır; eski kayıt kendiliğinden silinmez. Böyle bir değişiklik
eski çağrının sonucunu öğrenmek veya onu otomatik tekrar etmek değildir.

Durable kayıt sadece `tools/call` içindir; read/discovery, ping, subscriptions
ve logging gibi session işlemleri replay log'u değildir. Taşıma öncesindeki
bazı alt seviye doğrulama/bütçe hataları da konservatif uncertain bırakabilir;
fence uzakta kesin etki oluştuğu anlamına gelmez. Her kayıt external etkiler
incelenerek değerlendirilir. Read-only bağlantı hatası yeni transport kurmaya
izin verebilir, ancak failed RPC yine otomatik gönderilmez.

Mevcut agent/Pal journal katmanı ayrıca pending tool call/history replay'ini
engeller. MCP yanıtı alınmış ancak SDK output schema/uygulama kontrolü başarısız
olmuşsa MCP durable kaydı tamamlanabilir; üst oturumun pending kaydı kendi
kurallarınca korunur. MCP acknowledge oturum journal'ını temizlemez veya eski
turu resume edilebilir yapmaz.

## Açık reconnect ve yeniden bağlama

SDK `prepare_reconnect(server)` → host permit/approval → `execute` yeni bir
transport ve initialize/initialized handshake kurar. Hem `connect` hem
`connection/reconnect` policy tavanları uygulanır. Güncel trust/config,
TurnContext ve guest ownership yeniden denetlenir. Belirsiz tool kaydı varsa
önce inceleme gerekir. Eski RPC, model mesajı, bearer/login veya subscription
isteği yeniden gönderilmez. OAuth mevcut 3.7 credential/expiry kurallarını
kullanır; token yoksa giriş istenir, tarayıcı açılmaz.

```sh
namzu-rs mcp reconnect SERVER --approve --timeout 300
```

Bu stateless CLI komutu bir connect → açık reconnect → yeni generation'da ping
kabul kontrolüdür; önceki bir CLI sürecinin canlı bağlantısını sahiplenmez.
Kalıcı çağrı fence'i için yukarıdaki recovery komutları kullanılır. Başarılı
reconnect çıktısı generation'ları ve `replayed:false` değerini verir.

Disconnect/reconnect/close/config invalidation; eski permit, katalog ve
abonelikleri geçersiz kılar. Daha önce gönderilmemiş connect permit'i bile close
sonrasında kullanılamaz. Notification'lar generation alanı taşır; eski
sunucunun sıradaki bildirimleri yeni bağlantıya aktarılmaz.

SDK AgentTools hostu reconnect sonrası **açık `rebind`** ile yeni katalogları
izin callback'i üzerinden keşfeder. Bu işlem transport başlatmaz, eski per-call
onaylarını temizler; başarısızlıkta eski MCP tanımları kalmaz. Yeni model turu
yeni tanımları kullanmalıdır. `rebind` belirsiz çağrıyı/history'yi resume etmez.
Normal exec/TUI/Pal turu transport hatasında durur; sessizce tekrar model/tool
çağrısına devam etmez. Yeni görev için bağlantı/izin adımları yeniden çalışır.

## Resource subscription yaşam döngüsü

`resources.subscribe:true` ilan eden sunucuda ayrı izinli
`resources/subscribe {uri}` ve `resources/unsubscribe {uri}` çağrıları vardır.
Standart `_meta` nesnesi korunur. Başarılı correlated cevap üyeliği değiştirir;
RPC error değiştirmez. Duplicate subscribe, olmayan unsubscribe, kötü parametre
ve capability eksikliği uzak RPC'den önce reddedilir. Sunucu başına en fazla
128 aktif URI kabul edilir.

`subscriptions(server)` yerel aktif üyelikleri, `take_resource_updates(server)`
sadece bu generation'da abone olunan URI'lerin değişim kümesini verir. Aynı URI
coalesce edilir, tüketildikten sonra kümeden çıkar. Başka URI bildirimi bounded
inert data olarak kalır. Bilinen update bildiriminde bozuk URI parametresi
transport neslini fence eder. Katalog list_changed kendi 3.5 invalidation
kurallarını korur; update hiçbir URI'yi açmaz veya otomatik read çalıştırmaz.

```sh
namzu-rs mcp watch SERVER 'mem://resource' --approve --count 1 --timeout 300
```

Watch açık subscribe, sınırlı notification pump, explicit unsubscribe ve close
akışıdır. Varsayılan tek update, count en fazla 64, timeout en fazla 300 saniye.
Hata/iptalde bağımsız cleanup context'i ile close yapılır. Stdio EOF/grup
kapatması peer'i sonlandırır; HTTP'de varsa session DELETE best-effort'tür.
GET 405 veya desteklenmeyen notification akışı hata olarak raporlanır.

Kopma/SSE EOF/close/reconnect üyelikleri ve dirty set'i temizler. Yeni generation
eski abonelikleri otomatik restore etmez. SSE Last-Event-ID/resumption ve
arka plan reconnect döngüsü yoktur; yeni akış için açık reconnect/subscribe
çağrıları gerekir. Host crash durumunda uzaktaki HTTP session/aboneliklerin
anında silinmesi garanti değildir; sunucunun TTL/GC veya operatör cleanup'ı
geçerlidir. Guest stdio ayrıca owned Computer lifecycle'ının durdurma/pending
kurallarını kullanır.

## Sınırlar ve kabul

Toparlanma store'u en fazla 256 çözülmemiş kayıt kabul eder; kapasite aşımı ve
bozuk dosya fail-closed'dur. Store/registry lock'ları meşgulse beklemek yerine
hata verir. Owner lock dosyaları sır içermez; owner kimlikleri tekrar
kullanılmaz. Tüm eski lock dosyalarını prune eden bir komut bu teslimde yoktur.
Acknowledge yetkisi aynı OS kullanıcı hesabının host yönetimidir; model aracı
olarak sunulmaz. Store üzerinde yapılan yönetim işlemleri remote ağ çağrısı
başlatmaz.

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --workspace --locked
python3 scripts/mcp_recovery_smoke.py
python3 scripts/mcp_oauth_smoke.py
python3 scripts/mcp_smoke.py
```

Bağımsız Python stdio peer, Rust HTTP/SSE peer ve fake model fixture'ları:
SIGKILL/dead owner, aktif ack reddi, kalıcı fence, inspect/ack sonrası sıfır
replay, izin/generation/config sınırları, fresh session, HTTP 404/401, SSE EOF,
coalescing/subscribe/unsubscribe/128 sınırı, iki MCP sürümü ve AgentTools rebind
kontrollerini içerir. Gerçek CLI processen arası fence ve watch deadline cleanup
ayrıca sınanır. Üçüncü taraf canlı sunucu uyumluluğu veya bütün opsiyonel MCP
uzantıları için sertifikasyon iddiası yoktur. OAuth'ın 3.7'de belgelenmiş statik
client/origin sınırları devam eder. Canlı model ve gerçek OAuth kotası kullanılmaz.

Doğrulanan sonuç: 25 yeni Rust testi, toplam 396; format/Clippy/release. Release
CLI'de beş recovery kontrolü, 11 OAuth regresyon kontrolü ve normal/Pal TUI'de
altı MCP PTY kontrolü geçti. Toplam üç model fixture isteği, canlı model ve
gerçek OAuth hesabı çağrısı 0.
