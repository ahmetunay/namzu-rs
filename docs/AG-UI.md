# AG-UI yerel POST/SSE ve istemci — 7.2

`namzu-ag-ui`, Core'un opsiyonel host adaptörüdür: typed AG-UI olayları,
sınırlandırılmış SSE encoder/decoder ve istemci lifecycle modeli sunar. CLI
aynı exec/session/izin/telemetry zincirini kimliği doğrulanan loopback servisine
bağlar. Core bu pakete bağımlı değildir; ham provider replay export edilmez.

Upstream `a8a751ab157ba5392db107ee6c589575336b1711` snapshot'ındaki AG-UI
README, adapter, event mapper ve resmî client testleri referans alındı.
Uyumluluk hedefi `@ag-ui/core` / `@ag-ui/client` **0.0.59** olay alt kümesidir;
release servisi gerçek `HttpAgent` ile metin ve backend araç/onay/ret üzerinden
sınandı. Upstream'in frontend araçları, kalıcı interrupts/resume, state patch,
history snapshot, session index rebuild ve tüm opsiyonları bu teslimde yoktur.

## Sunucuyu başlatma

Sunucu için ayrı, rastgele bir capability oluştur. Bu model API anahtarı değildir.
Anahtarı komut satırına/JSON'a koyma; istemci aynı ortam değişkeninden okur:

```sh
export NAMZU_AGUI_TOKEN="$(python3 -c 'import secrets; print(secrets.token_urlsafe(32))')"
namzu-rs ag-ui --listen 127.0.0.1:0 --provider openrouter --model MODEL_ID \
  --cwd ./proje --timeout 300 --telemetry
```

Başlangıç stdout'u `url` alanıyla seçilen `/agent` adresini bildirir; token
bildirilmez. Varsayılan port `0` serbest yerel port seçer. `--auth-token-env NAME`
farklı değişken seçebilir; 32–512 görünür ASCII byte gerekir. Provider anahtarı,
base URL, model, workspace, MCP/capability seçimi ve araç izinleri host'un CLI/
config seçicileridir. Ağ/proxy/CA davranışı mevcut provider transport'undadır.

Yazma/shell gibi yetkiler ancak sunucunun `--allow-write` / `--allow-shell`
seçimiyle açılabilir; bunlar daha sonra istemcinin tek kullanımlık onayını da
ister. `--mcp-approve` ve `--resume` sunucu başlangıcında reddedilir. MCP de
canlı onay yolunu kullanır; istemci yeni MCP sunucusu seçemez veya policy'yi aşamaz.

Servis yalnızca literal loopback IP'ye bind eder. Bearer ve tam `Host` kontrolü
vardır; `Origin` içeren istekler reddedilir. CORS, TLS, public host/çoklu tenant
ve web uygulaması bu sürümde sağlanmaz. Node/Python istemcileri hedeflenir.
Capability sahibi, bu host'un seçtiği tek yetki alanına erişir; bir kimlik
sağlayıcı/rol sistemi değildir. Sunucu başlangıcı otomatik model çağrısı/login
başlatmaz; görev isteği seçilen servisi kullanır ve kota tüketebilir.

## İstemci ve istek

Birlikte gelen Python istemcisi final sonucu JSON döndürür. Varsayılanı etkili
araç taleplerini reddetmektir; `ask` gerçek terminalde tek tek sorar:

```sh
python3 scripts/agui_client.py --url http://127.0.0.1:PORT/agent \
  --thread-id proje-inceleme --approval ask 'Projeyi incele'
```

Ctrl-C `/cancel` gönderir ve bağlantıyı kapatır. Provisional metin final çıktı
diye basılmaz; eksik stream veya `RUN_ERROR` başarı sayılmaz. İstemci model
credential'ı okumaz. Capability/token değeri stdout/stderr'e eklenmez; görev
çıktısı özel kullanıcı verisidir.

`POST /agent`, `Content-Type: application/json` ve `Authorization: Bearer ...`
ile standart bir `RunAgentInput` alır:

```json
{
  "threadId": "opaque-thread",
  "runId": "fresh-run-id",
  "messages": [{"id": "fresh-user-id", "role": "user", "content": "Görev"}],
  "tools": [],
  "context": [],
  "state": {},
  "forwardedProps": {}
}
```

Son mesaj yeni, kimlikli bir user metni olmalıdır; önceki user/assistant/tool
mesajları yalnızca display history'dir ve modele yeniden aktarılmaz. Yetkili
model geçmişini sunucu kendi session'ından alır. System/developer/provider-state
rolleri, frontend tools, context, boş olmayan state/forwardedProps ve bilinmeyen
üst seviye alanlar reddedilir. İstemci provider/model/cwd/credential/izin/session
seçicisi gönderemez. Multimodal user input bu subset'te yoktur.

Aynı thread'in yeni turn'ü için yeni `runId` ve yeni son user message ID ver.
Resmî istemcinin biriktirdiği display history native session'ın yerine geçmez.
İstemciden gönderilen tool sonucu, onay veya yeni araç tanımı olarak yorumlanmaz.

Örnek resmî Node istemcisi (`@ag-ui/client@0.0.59`):

```js
import { HttpAgent } from '@ag-ui/client'
const agent = new HttpAgent({
  url: process.env.AGENT_URL,
  threadId: 'project-thread',
  headers: { Authorization: `Bearer ${process.env.NAMZU_AGUI_TOKEN}` },
  initialMessages: [{ id: 'user-1', role: 'user', content: 'Projeyi incele' }]
})
await agent.runAgent({ runId: 'run-1' }, {
  onRunFinishedEvent: ({ event }) => console.log(event.result.output)
})
```

Etkili araç kullanan resmî client, aşağıdaki custom onay olayını ele almalıdır;
yoksa çalışma onay, deadline veya disconnect bekler. Resmî istemcinin kullanılmış
`runId`'yi yeniden göndermesi otomatik retry/replay hakkı sağlamaz.

## Olay eşlemesi ve authoritative sonuç

| Kaynak | AG-UI olayı |
|---|---|
| Host run başlangıcı | `RUN_STARTED` |
| Provider adımı | `STEP_STARTED` / `STEP_FINISHED` |
| Assistant delta/aggregate | `TEXT_MESSAGE_START/CONTENT/END` |
| Doğrulanmış pending araç çağrısı | `TOOL_CALL_START/ARGS/END` |
| Gerçek araç sonucu veya ret | `TOOL_CALL_RESULT`, metadata.namzu.isError |
| Host/session/cleanup tamamlandı | `RUN_FINISHED` veya redakte `RUN_ERROR` |

`TOOL_CALL_END` argümanların tamamlandığını bildirir; etkinin tamamlandığı anlamına
gelmez. Çağrı, checkpoint ve izin kontrollerinden sonra gerçek araç sınırına
ulaşır. Args olayı aynı ID için tekrarlanmaz; değişen argümanla ID tekrar kullanımı
reddedilir. Denied/unavailable sonuçları hata olarak araca bağlı döner.

Native `TurnCompleted` tek başına `RUN_FINISHED` üretmez. Ajan döngüsü,
private session, telemetry ve cleanup sonucu beklenir. Kesik stream, timeout,
iptal, journal/publish hatası success'e çevrilmez. Son mesaj akıştan farklıysa
eski provisional mesaj kapatılıp `namzu.provisional_text_discarded` mesaj ID'sini
bildirir ve yeni mesaj açılır. **Final çıktı `RUN_FINISHED.result.output`tur**;
delta'ları birleştirmek bu sonucun garantisi değildir.

Olaylar task content/araç argümanı/sonucu içerir; telemetry değildir. ProviderState,
private reasoning, host credential/Authorization başlıkları ve ham sunucu hata gövdeleri olaylara
kopyalanmaz. `RUN_ERROR` sabit mesaj ve sınıf kodu taşır. Tool image için yalnızca
`namzu.image_available` boyut metadata'sı vardır; PNG/base64 transferi veya browser
rendering bu teslimde yoktur.

## Onay ve iptal

Read-only araçlar mevcut host politikasına göre çalışır. Diğer talepler için
`CUSTOM`, `name: "namzu.approval_required"` gelir. Value içinde `threadId`, `runId`,
`approvalId`, `toolCallId`, `name`, `arguments` bulunur. `/approve` ayrı bağlantıda
aynı bearer ile şu dar JSON'u kabul eder:

```json
{"threadId":"opaque-thread","runId":"fresh-run-id","approvalId":"a-...","decision":"allow"}
```

`deny` de geçerlidir. Approval nonce runtime tarafından üretilir, yalnızca o
aktif run'daki pending kararı tüketir; eski/yanlış/tekrar karar 400/404/409 döner,
etki vermez. İzin onayı stream'e argüman yayınının kendisi değildir. MCP connect
onayı gibi host gate'leri yalnızca custom payload olabilir; fake tool result üretilmez.
Onayların süresi aynı turn deadline'ıdır; yetki tavanını genişletmez.

`POST /cancel` body `{"threadId":"...","runId":"..."}` şeklindedir. Başka run/
thread'i iptal edemez; kabul 202'dir. SSE bağlantısının kapanması da provider HTTP
veya onay bekleyişindeki aynı cancellation ancestry'yi iptal eder. Efektler geri
alınmaz; gerçekleştirilmiş yazılar varsa session'dan incelenmelidir.

Bu yaşayan run içindeki onaydır. Upstream `RUN_FINISHED.outcome.interrupt` ile
kalıcı pause/resume, user-question/handoff ve frontend tool-result continuation
uygulanmadı; kesilen onay yeni bağlantıyla otomatik sürdürülmez.

## Kalıcılık, sınırlar ve SDK

Dış ID'ler opaktır; dosya yolu olarak kullanılmaz. Token namespace + thread hash'i,
private 0700 `$STATE/agui` altında 0600 binding dosyasını seçer. Binding model task
metni içermez; native session ID, hash'lenmiş kullanılmış run/message ID'leri ve
ready/running/blocked durumu tutar. Token değiştirmek yeni namespace seçer.
Completed thread normal restart sonrası aynı session'a devam eder. Failed,
cancelled veya çökmede running kalan thread bloklanır; otomatik replay/ack yoktur.
Yeni thread açmak eski etkileri geri almaz. Global retention/GC yoktur.

- Tek aktif worker/run; çakışan görev 409. Her thread en fazla 1024 run/message
  kimliği tutar. Private dizinde en fazla 4096 binding vardır; tek sabit admission
  lock kapasiteyi ve thread rezervasyonunu sunucu süreçleri arasında da korur.
- En fazla 16 geçici HTTP bağlantısı; başlık 16 KiB, JSON body 256 KiB,
  upload deadline 5 saniye. Duplicate header, chunked request, Expect, pipelining
  ve absolute request target reddedilir. Yavaş/kopan tüketicide bounded yazı iptali vardır.
- Event 1 MiB, stream 16 MiB; args 64 KiB, tool-result metni 128 KiB,
  en fazla 512 tool lifecycle. Konuşma bütçesi en fazla 128 KiB, turn en fazla
  300 saniye. Host'un config/CLI ile belirlediği küçük bütçe korunur; varsayılan
  config bütçesi servis tavanına daraltılır, CLI'de daha büyük istek reddedilir.
- Bounded final-write/cleanup süreleri ve dosya sistemi kernel çağrıları turn
  süresinin dışına taşabilir; hard real-time veya process sandbox iddiası yoktur.

SDK `Mapper`, `sse`, `client::Decoder` ve `client::ClientState` sunar.
`Decoder.eof()` ile frame tamamlanması, `ClientState.eof()` ile gerçek terminal
sonucu ayrı doğrulanır. Lifecycle hatasından sonra client state yeniden kullanılmaz.
`Mapper.propose` doğrulanmış pending çağrı sınırında bağlanmalıdır; CLI'ın
`EventSink::tool_proposed` hook'u journal'dan sonra, etkiden önce bunu sağlar.
Hook hata verirse araç çalışmaz. Core, TUI veya telemetry'ye AG-UI bağımlılığı eklenmez.

```sh
cargo run -p namzu-ag-ui --example offline --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo build --release --workspace --locked
python3 scripts/agui_smoke.py
# İsteğe bağlı resmî client uyumluluk kabulü:
npm install --prefix artifacts/agui-official --ignore-scripts --no-audit --no-fund @ag-ui/client@0.0.59
python3 scripts/agui_smoke.py --official-client
```

23 yeni Rust testi, toplam 638: 19 AG-UI codec/mapper/client, 3 HTTP/capability ve
1 telemetry proposal-forwarding testi. Resmî client seçeneğiyle 20 release kabul
grubu geçti: metin ve araç/onay/ret için gerçek HttpAgent, Python istemci,
private thread/session/telemetry, restart, replay, scope/auth reddi, bağımsız dosya
oraklı, provider hatası/kesik finish, iptal/disconnect, deadline ve süreç ölümü.
17 yerel grup Node/npm olmadan çalışır. Canlı model çağrısı 0.

Önceki 75 provider/servis/katalog/TUI, 13 telemetry ve 4 Pal PTY regresyon
kontrolü de geçti. Format/Clippy/release temizdir.

7.3 deterministik eval, 7.4 ajan/performance karşılaştırması ayrı aşamalardır.
