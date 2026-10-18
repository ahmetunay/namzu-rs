# MCP 3.4 — Profiller, yönetim, proje güveni ve izinler

`namzu-mcp::config` ve `namzu-mcp::manager` native Rust katmanları, 3.2/3.3
stdio/HTTP taşımasını yönetir. CLI `mcp` grubu bu katmanı kullanır. Normal
`exec/tui/pal` model araçlarına MCP eklenmesi 3.6'dadır; keşif 3.5'tedir.

## CLI

```sh
namzu-rs mcp init
namzu-rs mcp import mcp.config.example.json
namzu-rs mcp list
namzu-rs mcp show local
namzu-rs mcp profile create work
namzu-rs mcp profile list
namzu-rs mcp profile default work
namzu-rs mcp --profile default list
namzu-rs mcp --profile default enable local
namzu-rs mcp --profile default check local --approve --timeout 300
namzu-rs mcp --profile default disable local
namzu-rs mcp --profile default remove local
```

Örnek config'in adres/yolları yer tutucudur; iki sunucu da disabled gelir.
Gerçek executable/cwd/endpoint seçilmeden bağlantı kurulamaz. Import yalnızca
validate + atomik kayıt yapar; sunucu başlatmaz, hesap/model çağırmaz.

Tek sunucu eklemek/güncellemek için `Server` JSON nesnesi seçilir:

```json
{
  "transport": {
    "type": "stdio",
    "program": "/absolute/path/to/mcp-server",
    "args": [],
    "cwd": "/absolute/path/to/workspace",
    "env": {}
  },
  "policy": {
    "allow": ["connect", "ping", "tools/list"],
    "deny": [],
    "require_approval": true
  },
  "timeout_secs": 300
}
```

```sh
namzu-rs mcp add reader --spec server.json
namzu-rs mcp update reader --spec server.json
namzu-rs mcp --deny connect check reader --approve
namzu-rs mcp --allow connect,ping --require-approval check reader --approve
```

Add mevcut ismi, update eksik ismi reddeder. Import tüm kullanıcı belge/profil
kümesini değiştirir. `--profile` server yönetimi/listesi/check için seçicidir;
verilmezse kullanıcı belgesindeki `default_profile` seçilir. Default profil
silinemez; önce başka default seçilmelidir. Tüm komutlarda `--state-dir` ortak
özel kayıt dizinini seçer. Sonuçlar `format_version/status/output|error` JSON'dur.

List/show yalnızca isim, enabled, transport tipi, kaynak ve connect kararını
çıkarır. Argv, URL, env selector/değeri, bearer, server instructions/stderr veya
uzak hata gövdesi yazdırılmaz. Check, açıkça initialize + initialized + ping
çalıştırır ve kapatır; araç/model çağırmaz. `--approve` yalnızca o check
çağrısının connect/ping işlemlerini onaylar. Deny kuralını aşamaz. İnteraktif
TUI onay diyaloğu bu yönetim tesliminin kapsamında değildir.

## Kayıt ve şema

Özel `STATE/mcp/profiles.json` ve `trust.json` kullanılır. Mevcut PrivateStore
0700/0600, ownership, symlink/hardlink reddi, atomik rename/fsync ve process lock
kurallarını uygular. Config ve güven mutasyonları aynı `registry.lock` üzerindedir.

Document version 1, default_profile ve profiles içerir; her profil servers
haritasıdır. En fazla 32 profil, profil başına 64 sunucu ve 256 KiB JSON vardır.
Profil/server isimleri 1–64 ASCII alfanümerik, `_` veya `-` olabilir. Bilinmeyen
alanlar, import/proje JSON'unda yinelenen anahtarlar ve aşırı derin/düğümlü JSON
reddedilir. FIFO/socket/aygıt seçilmez; dosya okumaları bounded ve nonblocking'dir.

Stdio program/cwd mutlak yoldur; argv literal gider, shell/package runner veya
indirme eklenmez. `env` hedef ortam adı → kaynak ortam adı eşlemesidir; config
gerçek env değerini saklamaz. HTTP bearer yalnızca `bearer_env` selector'ıdır;
`bearer` gibi literal alan reddedilir. Seçilmiş değişkenler ancak connect kabul
edildikten sonra okunur. Argv/URL'ye kullanıcı tarafından konan özel metinler
özel config'te saklanır; CLI bunları otomatik görüntülemez. TLS/proxy/CA ve
stdio kontrollü env sözleşmesi korunur. Gizli state dosyaları Git'e yüklenmez.

## Proje güveni ve öncelik

```sh
namzu-rs mcp --project /absolute/project --project-config .namzu/mcp.json list
namzu-rs mcp --project /absolute/project --project-config .namzu/mcp.json trust
namzu-rs mcp --project /absolute/project --project-config .namzu/mcp.json check reader --approve
namzu-rs mcp --project /absolute/project --project-config .namzu/mcp.json untrust
```

İlk list/check, henüz güvenilmediyse hata verir. Proje dosyası kendiliğinden cwd
veya üst dizinlerden aranmaz. İki seçici birlikte gerekir; config proje içinde
relative olmalıdır. Capability-relative open, `..`/mutlak yol ve dışarı kaçan
symlink/FIFO'yu reddeder. Trust komutu tam seçilmiş dosyanın geçerli şemasını
önce doğrular, canonical proje kökü + relative config yolu + ham byte'ların
SHA-256 kimliğini özel kullanıcı kaydına koyar. İçerik/yol/kök değişince mevcut
trust eşleşmez. Aynı güvenilmiş byte'lar geri getirilirse içerik kimliği yine
eşleşebilir; bu monoton revision/OS attestation sistemi değildir.

Seçilen kullanıcı profili temel katmandır. Güvenilen proje belgesinin aynı
profilindeki sunucu tanımı isimle bunu ezer veya yeni isim ekler. Yerel default
profil kullanıcı seçimini değiştirmez. Aynı adlı global sunucunun disabled,
allow/deny ve approval sınırları korunur; proje bunları genişletemez. Çalıştıran
host'un Policy ceiling'i ek bir daraltmadır. Kullanıcı/project/host deny birleşir,
allow listeleri kesişir, katmanlardan herhangi birinin approval şartı korunur.
Yeni proje isimlerinde proje ve host kuralları geçerlidir; global kurallar kendi
server isimlerine bağlıdır. Project seçiliyken kullanıcı config mutasyonları
CLI'den reddedilir; proje JSON'u dışarıda değiştirilip yeniden trust edilmelidir.

## İzin ve SDK

Policy varsayılanı `allow: null, deny: [], require_approval: true` şeklindedir.
Allow null, deny dışında her hedefin onaya sunulmasına izin verir; boş allow
listesi her şeyi kapatır. `*`, tam method, tam hedef veya `method:*` desteklenir.
Örnek hedefler `connect`, `ping`, `tools/list`, `tools/call:read_file`,
`prompts/get:review`, `resources/read:file:///project/a` şeklindedir. Connect ayrı
bir etkidir ve allow listesinde ayrıca bulunmalıdır. Bilinmeyen metodlar, task
istekleri, eksik/bozuk name/uri ve aşırı JSON admission öncesi reddedilir. Server
capability beyanı bu izinleri açmaz; taşıma ayrıca capability kontrolünü sürdürür.

SDK akışı:

1. `Registry::open`, `Manager::new(registry, profile, project, ceiling, parent)`.
2. `prepare_connect` veya `prepare_call` immutable `Plan` döndürür.
3. Host Allow için `plan.permit()`, Ask için açık kullanıcı onayından sonra
   `plan.approve()` çağırır. Deny her iki yolda da hatadır.
4. `execute(permit, &context)` tam bu işlemi bir defa gönderir. Permit taşınır;
   Clone/Serialize değildir. Başka manager'ın veya eski config'in izni reddedilir.
5. `statuses`, `abandoned` ve açık `close(&cleanup_context)` yaşam döngüsünü sürer.

Parametreler/isim/sunucu/konfigürasyon onaydan sonra değiştirilemez. Yeni her
prepare/execute özel kaydı ve proje trust'ını yeniden doğrular. Config değişimi
veya trust kaybı sahip olunan eski bağlantıları emekliye ayırır; stdio cleanup yapar
ve HTTP session DELETE denemeleri için ortak 2 saniyelik cleanup context'i kullanır.
Hata nedeniyle kapalı HTTP nesli açık close için korunur. Cleanup hatası uzak
session'ın sonlandığını garanti etmez. Drop stdio süreçlerini temizler; HTTP
DELETE için açık close gerekir. Reconnect/replay otomatik yapılmaz.

Registry lock admitted network/process etkisi boyunca tutulur. O sırada ikinci
süreçte ayar mutasyonu fail-fast busy döner; mutasyonun başarılı olması sonraki
admission'ı engeller. Sürmekte olan etkiyi durdurmak için parent/caller context
iptali gerekir; bitmiş uzak etki geri alınmaz. Caller context parent'ın clone'u
veya daha dar çocuğu olmalıdır: deadline ve cancellation ancestry birlikte
kontrol edilir. Başka root ile parent iptali devre dışı bırakılamaz. Stdio'nun
kalıcı lifetime context'i ile kısa initialize context'i ayrıdır.

Manager en fazla 16 sahip olunan bağlantı tutar ve stdio nedeniyle thread-affine
çalışır. Her işlem `timeout_secs` (1–300, varsayılan 300) ve parent/caller kalan
süresinin küçüğüyle sınırlıdır. Tek mutable owner, tek aktif operasyon vardır.
Manager normal model ajanına otomatik eklenmez; discovery/notification pump'ı
3.5–3.6'da bağlanacaktır. HTTP async host için MCP-HTTP.md'deki runtime kuralı
geçerlidir.

Belirsiz çağrıların ID/metodu `abandoned()` içinde korunur; o sunucu aynı manager
neslinde tekrar bağlanamaz. Kanıt bellektedir; süreç çökmesinde kalıcı recovery
journal değildir. Kalıcı recovery/subscription/reconnect 3.8, OAuth 3.7 kapsamıdır.
Güvenilmiş bir stdio programı host yetkileriyle çalışır; bu katman OS sandbox'ı
ve başka OS kullanıcılarına karşı güvenlik sınırı değildir.

## Kabul — 2026-10-18

19 yönetim/güven/gerçek taşıma testi, 6 CLI subprocess testi, 1 core context
ancestry testi ve 1 tek kullanımlık permit compile-fail testi: 27 yeni test,
workspace toplamı 295 Rust testi. Profil önceliği, özel/atomik kayıt ve lock,
bozuk import sonrası eski kaydı koruma, redaksiyon, duplicate/depth/size sınırı,
traversal/symlink/FIFO, trust değişimi/revoke, deny/allow/approval birleşimi,
çapraz manager/eski grant, ayrı root reddi, gerçek stdio çağrı/iptal/cleanup,
init lifetime ayrımı, belirsiz etkiyi tekrar etmeme ve gerçek HTTP initialize/
call/DELETE/permission-lock/revoke/failure cleanup doğrulandı.

Fmt, locked workspace test, warnings-as-errors Clippy, workspace release ve
release CLI yönetim/check kabulü çalıştırılır. Testler canlı model/MCP hesabı
kullanmaz; model çağrısı 0. Önceki HTTP TLS/proxy ve stdio testleri regresyona
dahildir. Bu teslimde Docker/PTY veya üçüncü taraf canlı MCP kabulü çalıştırılmaz.
