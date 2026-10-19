# Yetenek kaydı ve ajan entegrasyonu — 5.5

`namzu-capabilities` bağımsız SDK, `Tools` adaptörü ve typed config sağlar.
Çekirdek optional paketlere bağımlı değildir. CLI exec, normal TUI, Pal ask/team,
mailbox ve Pal TUI aynı adaptörü kullanır. Ses/LSP varsayılan olarak açılmaz;
config tek başına bir program veya servis başlatmaz.

## Seçim ve yetkiler

```sh
namzu-rs capabilities --config capabilities.config.example.json list
namzu-rs capabilities --config capabilities.config.example.json \
  --capability audio,lsp --allow-audio --allow-lsp --allow-write list
namzu-rs exec 'WAV dosyasını incele' --model MODEL --config CONFIG.json \
  --capability audio --cwd ./project --json
namzu-rs tui --model MODEL --config CONFIG.json --capability audio,lsp \
  --allow-audio --allow-lsp --allow-write
namzu-rs pal --model MODEL --config CONFIG.json --capability audio,lsp \
  --allow-audio --allow-lsp --allow-write create Assistant
# Aynı seçim/config ve yalnızca o çağrıda istediğin izinlerle:
namzu-rs pal --config CONFIG.json --capability audio,lsp \
  --allow-audio --allow-lsp ask PAL_ID 'Dosyaları incele'
```

Örnek config'teki program/model/env adları yer tutucudur; gerçek seçimleri
kullanıcı yapar. Key dosyaya veya komut satırına yazılmaz; yalnızca env adı
saklanır. Varsayılan CLI/config örneği önceki dosya araçlarını korur.
`--capability` tekrar veya virgülle `audio,lsp` alır; unknown/duplicate seçim ve
seçilmemiş pakete grant reddedilir. `capabilities list` gerçek model, ses
servisi veya LSP sunucusu başlatmadan araç isimlerini ve etkin izinleri verir.
`allow` capability listesi varsa daraltır; `deny` önceliklidir. Normal
`tool_allow` / `tool_deny` de bu araçlara uygulanır. `files` paketini config'te
reddetmek built-in dosya/shell araçlarını kapatır; MCP ayrı açık seçimdir.

| Paket/araç | Gereken seçim | Ek çağrı yetkisi / davranış |
|---|---|---|
| Mevcut gelişmiş dosya, belge, görsel araçları | varsayılan `files` | Yazma ayrıca `--allow-write`; mevcut 5.1 sınırları |
| `audio_inspect` | `--capability audio` | Yerel bounded PCM16 WAV metadata; servis yok |
| `audio_transcribe` | audio + config `audio` | `--allow-audio`, WAV seçilen speech endpoint'ine gönderilir |
| `audio_synthesize` | audio + config `audio` | `--allow-audio` + `--allow-write`; text gönderir, NEW private WAV üretir |
| `lsp_diagnostics`, `lsp_definition`, `lsp_references` | lsp + config `lsp` | `--allow-lsp`; explicit HOST executable, UTF-16 konum |
| `lsp_rename_preview` | lsp + config `lsp` | `--allow-lsp`; plan + preview döner, kaynak yazılmaz |
| `lsp_apply` | lsp + config `lsp` | `--allow-lsp` + `--allow-write`; tam `plan` nesnesiyle tek dosya apply |
| Docker sandbox | `--computer-image` + `--sandbox-policy` | Policy shell/yazmayı açmaz; ayrıca mevcut shell/yazma grant'ları gerekir |

Ses sürücüsü ancak onaylanmış tool call içinde oluşturulur. `audio_inspect`
key veya service ayarı gerektirmez. LSP her sorguda taze client/document açar ve
aynı çağrının sonunda shutdown/exit yapar; arka planda devamlı server yoktur.
LSP süreç env'i temizdir; config'teki `env` yalnızca açık aktarılacak isimleri
alır. `args` içinde credential verilmemelidir. Host LSP, cwd dışındaki dosyalara
ağa erişebilen güvenilen executable'dır; bu bir sandbox değildir.

`lsp_rename_preview` sonucu `plan` ve `preview` içerir. `lsp_apply` argümanı
`{"plan":...}` tam typed plandır; kaynak hash'i değişmişse veya plan workspace
dışına çıkıyorsa yazma olmaz. Tool batch'inin bütün argüman/path/WAV/konum/plan
kontrolleri ilk etkiden önce yapılır. Servis veya süreç hatası otomatik tekrar
edilmez; başarısız session/pending journal mevcut recovery kuralını korur.
SDK ses/LSP dosya, süre ve byte sınırları 5.2/5.3'ten devralınır.

## TUI ve Pal

Yeni ses ve LSP araçları normal ve Pal TUI'de **her çağrıda** onay ekranından
geçer. Dialog, speech servisine veri gönderileceğini veya host LSP executable
çalıştırılacağını açıkça gösterir; ret halinde servis/process başlatılmaz.
`audio_inspect` de bu ilk sürümde onay ister. Uzun tool argümanlarında mevcut
64 KiB TUI approval sınırı geçerlidir. Başlık paket seçimini ve invocation
izinlerini gösterir; F6 model/bütçe düzenlemesi yeni izin üretmez.

Session route'u yalnızca seçilen paket ayarlarının canonical SHA-256 kimliğini
saklar. Endpoint/model/env seçicisi veya LSP program/argv/env isimleri değişirse
resume reddedilir. Ayar dosyası path'i değişse de içerik aynıysa seçim aynıdır.
İzin bayrakları route'tan geri verilmez: devam çağrısında yeniden seçilir,
daha dar izinle devam edilebilir. Executable veya argv ile seçilen script
baytlarını hash'leyen bir binary attestation sistemi değildir; kullanıcı
explicit seçtiği server kurulumuna güvenir.

Pal profilinde paket/config kimliği ve oluşturulurken açık olan tool isimleri
üst sınırdır. `ask`, `team`, mailbox ve TUI güncel seçim/izinleri tekrar kesiştirir.
Inspect-only oluşturulmuş Pal sonradan yalnızca `--allow-audio` ile speech aracı
kazanamaz; yeni profil/revizyon bilinçli oluşturulmalıdır. Geçmişte capability
alanı olmayan Pal/session kayıtları eski varsayılan seçimle okunur.

## Sandbox ile ajan ve devam

```sh
namzu-rs exec 'Çalışma alanına bir dosya yaz' --model MODEL \
  --computer-image sha256:LOCAL_IMAGE_ID --sandbox-policy policy.json \
  --allow-write --allow-shell --json
namzu-rs exec 'Aynı dosyayı oku' --model MODEL --resume SESSION_ID \
  --computer-image sha256:LOCAL_IMAGE_ID --sandbox-policy policy.json --json
namzu-rs capabilities --state-dir PRIVATE_STATE show SESSION_ID
namzu-rs capabilities --state-dir PRIVATE_STATE cleanup SESSION_ID
```

Normal sandbox session'ında kaynaklar başarılı işten sonra durdurulur ve
volume tutulur; tamamlanan session resume aynı owned bilgisayarı kullanır.
Policy/imaj değişikliği veya belirsiz pending resume'u engeller. Failed session
otomatik devam etmez. Cleanup açıkça owned container ve volume'u siler.
Hata/Ctrl-C halinde de ayrı cleanup context'iyle durdurma denenir.

Pal için aynı `--computer-image` ve `--sandbox-policy` create/ask/team/work/tui
çağrılarında yeniden seçilir. Full ComputerSpec profilde pin edilir; başarılı
işler arasında guest volume korunur. Pal `computer ... cleanup` mevcut açık
operator temizliğidir. Direkt `pal computer ... create` de seçilen policy'yi
uygular; bir Pal'ın pinned sandbox spec'inden farklı computer ile görev
çalıştırması reddedilir.

Guest üzerinde host ses/LSP paket seçimi **reddedilir**. Model veya config
bir host workspace'i sessizce yedek backend olarak kullanamaz. Guest dosya
aktarımı mevcut 64 KiB protokolüyle sınırlıdır; 5.1'in büyük dosya/PNG araçları
henüz guest köprüsüne taşınmadı. Masaüstü/tarayıcı policy kombinasyonları ve
native sandbox 5.4'teki gibi desteklenmez. API sağlayıcısı host kontrol
katmanındadır; guest network none modeli host API bağlantısını kapatmaz.

## Deterministik kabul

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo build --release --workspace --bins --examples --locked
python3 scripts/capabilities_smoke.py
# Yerelde 5.4 headless guest image mevcutsa gerçek Docker kabulü de:
python3 scripts/capabilities_smoke.py --image sha256:LOCAL_IMAGE_ID
```

Fixture speech servisi, toy LSP, bağımsız dosya oraklları, gerçek release
exec/Pal çağrıları ve normal/Pal Linux PTY onay-ret testleri kullanılır.
Bunlar canlı model muhakeme benchmark'ı değildir. Özel session, ses çıktısı,
credential ve `artifacts` dosyaları GitHub yedeğine girmez.

2026-10-19 bu workspace kabulü: 13 yeni Rust testiyle toplam **516 test**,
workspace fmt/Clippy `-D warnings` ve release derlemesi geçti. Bağımsız fixture
ile **15 CLI/PTY/Docker kabul grubu**, ayrıca **25 mevcut TUI regresyon kontrolü**
geçti. Bu testlerde canlı model veya gerçek speech servisi çağrısı **0**.
