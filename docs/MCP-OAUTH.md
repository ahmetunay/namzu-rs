# MCP 3.7 — açık OAuth girişi ve özel kimlik bilgileri

MCP HTTP sunucuları için authorization-code + PKCE/S256, protected resource ve
issuer metadata keşfi, loopback callback, özel token deposu ve refresh eklendi.
Bu modül OpenAI/Codex hesap bağlantısından bağımsızdır. Bir MCP sunucusunun
OAuth ayarları ve o sunucu için kayıt edilmiş client ID gerekir.

## Kullanım

`mcp add` ile kaydedilecek Server JSON örneği:

```json
{
  "transport": {"type": "http", "url": "https://mcp.example.com/mcp"},
  "oauth": {
    "issuer": "https://login.example.com",
    "client_id": "SUNUCUNUN_VERDIGI_CLIENT_ID",
    "scopes": ["files:read"]
  },
  "policy": {"require_approval": true}
}
```

```sh
namzu-rs mcp add files --spec server.json
namzu-rs mcp auth login files --approve --timeout 300
namzu-rs mcp auth status files
namzu-rs mcp check files --approve
namzu-rs exec 'Dosyaları incele' --mcp-server files --mcp-approve \
  --model MODEL --api-key-env OPENAI_API_KEY
namzu-rs mcp auth logout files
```

Login komutu tarayıcıda açılacak URL'yi stderr'e yazar. Tarayıcı kendiliğinden
açılmaz. OAuth callback yalnızca komutun çalıştığı makinenin **127.0.0.1**
adresinde dinlenir; `--port` yoksa boş port seçilir. Tarayıcı başka makinedeyse
bu loopback portuna önceden güvenilir erişim/forward gerekir; bulut komutunun
127.0.0.1 adresi ev bilgisayarını göstermez. Kayıtlı client redirect URI'si
`http://127.0.0.1:PORT/oauth/callback` biçimini desteklemelidir. Sabit port isteyen
kayıtlarda `--port PORT` kullanılır. Browser/device login otomasyonu veya manuel
callback URL yapıştırma bu teslimde yoktur.

`--profile`, açık `--project` / `--project-config` ve içerik trust kuralları
normal yönetim komutlarıyla aynıdır. `oauth/login` politika hedefidir;
`--deny oauth/login` üstün gelir. `--approve` yalnızca bu açık login koşumunun
Ask kararını karşılar. Ajan, TUI, Pal veya HTTP 401 yanıtı interaktif giriş
başlatmaz. Eksik credential açık login hatasıdır. Guest HTTP hâlâ desteklenmez.

Public client için `token_endpoint_auth_methods_supported` içinde `none`
gerekir. Confidential client destekleniyorsa `oauth.client_secret_env` ile
**ortam değişkeni adı** seçilir; metadata `client_secret_post` bildirmelidir.
Secret komut argümanı/config değeri olarak alınmaz. Bu alan stdio veya
`transport.bearer_env` ile birleştirilemez. Eski env-bearer yolu korunur.

## Keşif, güven ve kapsam

- Anonymous GET ile MCP endpoint'in `WWW-Authenticate: Bearer` challenge'ı
  incelenir. Quoted realm içindeki virgül/escape'ler, resource_metadata ve scope
  ayrıştırılır. Challenge scope'u config'te yoksa sessizce yetki eklenmez.
- Challenge'ta resource_metadata varsa önceliklidir. Yoksa
  `/.well-known/oauth-protected-resource/ENDPOINT_PATH`, sonra root well-known
  denenir; yalnızca 404 sonraki adrese geçirir.
- Protected metadata `resource` değeri açık endpoint ile birebir eşleşmeli,
  `authorization_servers` listesi config'te sabitlenmiş issuer'ı içermelidir.
  Birden fazla issuer varsa istemci kendi ayarındaki issuer'ı seçer.
- Issuer path'ine göre OAuth well-known, OIDC well-known/path ve
  path/OIDC well-known sırası uygulanır. Dönen issuer birebir eşleşir.
  Authorization ve token endpoint'leri aynı issuer origin'inde olmalıdır.
  Protected metadata URL'si resource origin'inde olmalıdır. Uzak içerik yeni
  trusted origin seçemez. Bu konservatif origin kısıtları bazı sağlayıcıların
  ayrı domain metadata/endpoint dağıtımlarıyla uyumlu değildir.
- HTTPS veya açık literal loopback HTTP kabul edilir. Userinfo, fragment ve
  Config/metadata endpoint URL query'leri reddedilir. Redirect takip edilmez; otomatik retry yoktur.
  Sistem proxy/CA ve TLS doğrulaması korunur. Discovery ve token endpoint'lerine
  MCP/model bearer token'ı gönderilmez.
- Metadata S256 ve code response desteğini doğrulamadan URL üretilmez. State ve
  verifier kriptografik 32 rastgele byte'tır. Authorization ve token isteklerine
  doğru `resource` parametresi eklenir. Access token opaque kabul edilir; istemci
  sağlayıcının JWT içeriğini yeniden yorumlamaz.

Pre-registered statik client ID (ve sunucu destekliyorsa metadata-document URL'si
olarak verilmiş ID) kullanılır. Otomatik dynamic client registration, client
metadata belgesi barındırma, client_secret_basic, device code ve browser açma yoktur.
Bu teslim her OAuth sağlayıcısı için evrensel uyumluluk iddiası taşımaz.

## Callback ve sınırlar

GET, exact callback path, tek Host/port, tek parametreler ve doğru state gerekir.
Yanlış state, host ve duplicate parametreli istekler 400 ile reddedilir;
meşru callback için dinleme devam eder. Hatalı istek sayısı en fazla 32,
header/request 8 KiB, bağlantı okuma süresi en fazla iki saniyedir. Toplam login
ve network akışı TurnContext deadline/iptalini kullanır; default 300 saniyedir.
Metadata issuer-response parametresi istiyorsa `iss` zorunludur; varsa her zaman
issuer eşleşir. Kendi state'iyle gelen provider error girişi bitirir. Kod,
token, verifier ve provider error_description çıktıya/log'a eklenmez. Callback
cevapları sabit metin ve no-store/no-referrer başlıkları kullanır.

Metadata/token JSON en fazla 64 KiB; arrays ve string'ler bounded'dır. Beklenmeyen
content type, bozuk JSON, redirect, scope genişlemesi, bearer dışı token tipi,
boş/geçersiz token ve eksik/0/366 günden büyük expires_in reddedilir.
Callback portu tüm çıkışlarda socket Drop'u ile kapanır. SDK senkrondur;
aktif Tokio runtime içinden çağıran host, HTTP istemcisinde olduğu gibi ayrı
blocking thread kullanmalıdır.

## Token saklama ve yenileme

`STATE/mcp/oauth/` özel 0700 dizindir. Dosyalar ve lock'lar 0600, credential
write atomik/sync, read symlink/FIFO/owner/mode kontrolleri mevcut PrivateStore
üzerindendir. Dosya adı resource URL + issuer/client/scopes/secret-selector
ayarlarının SHA-256 hash'idir. Başka resource/client ayarı bu token'ı kullanamaz.
Aynı açık resource/client kimliği profiller arasında paylaşılabilir; işlem
izinleri her çağrıda ayrıca kontrol edilir. Config değişince eski blob otomatik
silinmez; eski konfigürasyona dönülünce yeniden kullanılabilir. Durum çıktısı
sadece logged_in/refresh_available/uncertain/expiry verir.

Depo **OS erişim izinleriyle korunan plaintext JSON**'dur; işletim sistemi
keychain'i veya disk şifreleme katmanı değildir. Kaynak Git yedeğine eklenmez.
Dosyalar token'lar ve varsa refresh token içerir; client secret saklanmaz.

Access token yeni HTTP bağlantısında 30 saniye erken expiry payıyla kontrol
edilir. Süresi dolmak üzereyse mevcut refresh token, aynı issuer token endpoint'i,
client ID ve resource ile yenilenir. Rotated refresh token atomik saklanır;
yanıt refresh token vermiyorsa önceki korunur. Scope daralabilir, genişleyemez.
Aynı credential için process lock çift refresh'i önler; meşgul store fail-fast'tir.
Aktif MCP session'ında 401 alan bir araç çağrısı yeniden gönderilmez. Refresh
bağlantı kurulmadan önce yapılır; otomatik login/reconnect değildir.

Rotating grant gönderilmeden önce durable `uncertain` kaydı yazılır. Network
kopması, iptal, invalid_grant veya başarısız response/write sonrasında aynı grant
yeniden denenmez; açık login gerekir. Code exchange başarısızsa eski identity
sessizce aktif kalmasın diye credential exchange öncesi kaldırılır. Tamamlanmamış
login callback öncesinde mevcut credential'ı değiştirmez. Login registry lock'ını
admission boyunca tutar; başka config işlemleri bu sırada meşgul hatası alabilir.

Logout yalnızca seçili binding'in yerel dosyasını siler; sağlayıcıda remote
revocation yaptığını iddia etmez. Kullanıcı provider hesabında da erişimi
kaldırabilir. Konfigürasyonları değiştirmeden önce ilgili credential'ları logout
etmek orphan private blob bırakılmasını önler.

## Kabul

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --workspace --locked
python3 scripts/mcp_oauth_smoke.py
```

Rust loopback fixture'ları keşif/path/OIDC fallback, PKCE, pinning, expiry/refresh
rotation, kayıp yanıt/invalid_grant sonrası replay reddi, callback/iptal ve private
storage sınırlarını kontrol eder. Ayrı Python fixture gerçek CLI login/status/
logout/check/exec akışını, kötü callback'leri, SIGINT iptalini ve token sızdırmayan
hataları doğrular. Normal ajan testinde model de loopback fixture'dır; canlı model,
gerçek OAuth hesabı veya provider kotası kullanılmaz. Canlı üçüncü taraf sağlayıcı
uyumluluğu bu kabulün sonucu değildir; toparlanma/interoperability 3.8'de sürer.

Standartlar: [MCP 2025-11-25 Authorization](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization),
RFC 9728, RFC 8414, OIDC Discovery, RFC 7636, RFC 8252 ve RFC 8707.

Bu aşamanın sonucu: 18 yeni Rust testi, toplam 371; format/Clippy/release ve
release CLI üzerinde 11 bağımsız uçtan uca kontrol. Model fixture'ına iki
istek gönderildi; canlı model ve gerçek OAuth hesabı çağrısı 0.
