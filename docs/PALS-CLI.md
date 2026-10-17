# Pals 2.6: CLI, TUI ve SDK host bağlantıları

Bu akışlar Rust SDK'sini kullanır. `pal ask/run/team/work` gerçek sağlayıcı
seçimidir; otomatik offline cevap veya başka model/host fallback yoktur.
Normal testler loopback fixture kullanır; hesap/model kotası tüketmez.

## Profil ve görevler

Tüm genel seçenekler **alt komuttan önce** yazılır. Örnekler `models` çıktısında
bulunan model ID'sini ve önceden bağlanmış native Codex hesabını kullanır:

```sh
namzu-rs pal --provider codex --model MODEL_ID --effort low create Reader
namzu-rs pal list
namzu-rs pal show PAL_ID
namzu-rs pal ask PAL_ID 'Çalışma alanını listele'
namzu-rs pal tasks PAL_ID
namzu-rs pal inspect PAL_ID TASK_ID
namzu-rs pal ask PAL_ID 'Önceki sonuçtan devam et' --follow COMPLETE_TASK_ID
namzu-rs pal queue PAL_ID 'Daha sonra çalışacak görev' --task task-one
namzu-rs pal run PAL_ID task-one
```

`create` çıktıdaki `result.id` değerini döndürür. JSON sözleşmesi
`format_version:1`, `status:ok|error`, `result` (üst düzey hata için `error`).
Görev hataları exit 1 verir. Clap sözdizimi hataları stderr ve exit 2'dir.
`inspect` ve `inbox` özel konuşma/görev içeriği gösterebilir.

Pal, varsayılan özel state altında `pals/` metadata'sı ve kardeş
`pal-workspaces/` host çıktı alanıyla oluşturulur. `--state-dir` her komutta aynı
kaydı seçer; `--owner` aynı OS kullanıcısının isim alanıdır, ayrı OS güven sınırı
oluşturmaz. `--cwd` kullanılamaz: Pal çalışma alanı kayıt tarafından belirlenir.
`purpose` açıklama metadata'sıdır; kullanıcı mesajını system talimatına yükseltmez.

Profil model, sağlayıcı, endpoint, key-env **adı**, effort, gerçek Codex hesap
kimliği ve host/guest backend'ini sabitler. Sonraki komutlar profili kullanır;
açıkça farklı model/endpoint/hesap seçmek çalıştırmayı reddeder. Anahtar/token
profile yazılmaz. Config değişikliği kayıtlı rotayı sessizce değiştirmez. SDK ile
oluşturulmuş keyfi route dizgeleri bu CLI'de otomatik yorumlanmaz.

Araçlar hem kayıtlı profilin hem de **o çağrının** izin tavanına uyar. Yazma/shell
izni oluştururken verildiyse çalıştırırken de açıkça verilmelidir:

```sh
namzu-rs pal --provider codex --model MODEL_ID --allow-write create Writer
namzu-rs pal --allow-write ask WRITER_ID 'answer.txt içine 42 yaz'
namzu-rs pal --allow-write --tool-deny write_file ask WRITER_ID 'İncele'
namzu-rs pal update PAL_ID --revision 1 --name YeniAd --tools read_file,list_files
namzu-rs pal status PAL_ID --revision 2 paused
namzu-rs pal status PAL_ID --revision 3 active
```

Revizyon kontrolü eski yönetim isteğinin yeni profili ezmesini engeller.
Archive kalıcıdır. Eski görev/followup kendi profil revizyonunda kalır; güncel
pause/archive çalışmayı yine durdurur. Host backend'inde `--allow-shell`, host
OS erişimidir; guest seçimi aşağıdadır. Config/allow/deny her seferinde yeniden
daraltır. Aynı Pal'da eşzamanlı iki run yapılamaz.

## Ekipler ve mesaj işleme

```sh
namzu-rs pal --max-steps 32 --max-tool-calls 16 team PAL_A PAL_B \
  --prompt 'Bağımsız sonucu hesapla' --concurrency 2
namzu-rs pal send PAL_ID message-one 'Bu girdiyi işle'
namzu-rs pal send PAL_ID message-two 'İkinci girdi' --from OTHER_OWNED_PAL_ID
namzu-rs pal inbox PAL_ID
namzu-rs pal --max-steps 32 work PAL_ID --max-messages 4
```

Ekip en fazla 64 **farklı** Pal ve 1–16 worker kabul eder. Tüm profiller ve
bütçe ilk görev yayınlanmadan kontrol edilir. Süre varsayılan 300 saniye ve
tek ortak kesin deadline'dır; adım/araç/metin bütçesi Pal sayısına bölünerek
önceden ayrılır, kullanılmayan pay başka işe aktarılmaz. En fazla concurrency
kadar worker bir dalgada çalışır; her dalga join edilir. Etki kabulü ortak
deadline ile kesilir; kısa metadata kilidi için en fazla 5 saniyelik bekleme
ve iptalden bağımsız cleanup, komutun dönüş süresini uzatabilir. Sonuçlar giriş
sırasındadır; bir Pal'ın hatası diğerinin sonucu yerine yazılmaz. Yayınlama tek
çok-Pal transaction değildir: disk/yarış hatası daha önce yayınlanan **queued**
görevleri bırakabilir; bunlar yeniden yaratılmadan `tasks` ile incelenir.

`work` FIFO kuyruğu boşalana, sayı/deadline/iptal sınırına kadar **otomatik**
işler ve çıkar. Config bütçesi max-messages sayısına bölünür; SDK aggregate
rezervasyonu ve lease fencing korunur. Kalıcı, kendiliğinden başlayan arka plan
daemon'u yoktur. Aynı mesaj ID'si + aynı payload yeniden gönderildiğinde yeni
görev oluşturulmaz. Failed/uncertain model/araç işi otomatik denenmez; farklı
mesajlar kendi sonuçlarını alır. Pal sender aktif ve aynı owner'da olmalıdır;
mesaj gövdesi her zaman User içeriğidir.

Ctrl-C / SIGTERM / SIGHUP aynı iptal ağacını kullanır; HTTP ve child süreçler
join/cleanup edilir. Tamamlanan etkiler geri alınmaz. SIGKILL sonrası Running
kalan işi inceleyip açıkça kapatmak için:

```sh
namzu-rs pal inspect PAL_ID TASK_ID
namzu-rs pal acknowledge PAL_ID TASK_ID
namzu-rs pal inbox PAL_ID
namzu-rs pal resolve PAL_ID MESSAGE_ID
```

Mailbox belirsizliği önce lease/recover uzlaştırmasıyla NeedsInspection olur;
`work` bunu yapar ve yürütmeyi reddeder. `resolve`, yalnızca bu kaydı kapatır;
eksik cevabı üretmez veya eski etkiyi tekrar etmez. Görev ve guest bilgisayarın
pending kayıtları ayrı incelenir.

## Guest bilgisayarlar

Önce [COMPUTERS.md](COMPUTERS.md) koşullarıyla Rust guest image'ini kur;
`artifacts/pal-image.json` içindeki yerel **sha256 image ID** değerini kullan.
Runtime image çekmez/derlemez. Docker executable mutlak trusted path'tir;
varsayılan `/usr/local/bin/docker`, başka kurulumda `--docker /usr/bin/docker`.

```sh
python3 scripts/build_pal_image.py
namzu-rs pal --provider codex --model MODEL_ID --guest --allow-write create Guest
namzu-rs pal --image sha256:IMAGE_ID computer GUEST_ID create
namzu-rs pal --allow-write ask GUEST_ID 'answer dosyasına 42 yaz ve geri oku'
namzu-rs pal computer GUEST_ID show
namzu-rs pal computer GUEST_ID start
namzu-rs pal computer GUEST_ID restart
namzu-rs pal computer GUEST_ID stop
namzu-rs pal --allow-write computer GUEST_ID upload binary ./input.bin
namzu-rs pal computer GUEST_ID download binary ./NEW-output.bin
namzu-rs pal computer GUEST_ID list
namzu-rs pal computer GUEST_ID cleanup
```

`create` sonrası bilgisayar durur, named volume kalır. `start/restart` açıkça
sağlıklı idle PID1'i çalışır bırakır; görevler/transferler bitince bilgisayar
durur ve dosyalar korunur. Host callback unwind veya setup iptalinde de kalıcı
volume korunur ve remote iş bağımsız en fazla 20 saniyelik cleanup ile durdurulur.
Daemon erişilemiyorsa cleanup gerektiği kayda yazılır; fiziksel kapanma garantisi
yoktur. `cleanup` yalnızca kimliği doğrulanmış o container/volume'u siler.

`--image ID ask/team/work/tui` her turn için geçici guest açar. Başarılı/sağlıklı
sonuçta kaynaklar kaldırılır; followup **konuşmayı** taşır, geçici bilgisayarın
dosyalarını taşımaz. Dosya devamı için yukarıdaki kalıcı bilgisayarı kullan.
Pending/uncertain etki varsa geçici volume da korunur; model yeni yazma/shell
başlatamaz. `computer download/list` ile açık inceleme, ardından
`computer PAL_ID acknowledge` gerekir. Hiçbirinde sessiz host fallback yoktur.

Guest read/list varsayılandır; write ve shell yine profil + çağrı bayraklarını
ister. CPU/RAM/PID ve aktarım sınırları 2.5 ile ortaktır; hard disk kotası henüz
yoktur. Tarayıcı/görsel bilgisayar 4. modüldür.

## TUI ve SDK

```sh
namzu-rs pal --allow-write tui PAL_ID
```

Aynı Pal çalışma adaptörü, journal, model rotası ve araç politikası kullanılır.
Her yazma/shell çağrısı TUI'de ayrıca tek seferlik onay ister. F5 bu Pal'ın
görev geçmişini listeler; Complete geçmişinden devam **yeni** task/session açar.
Failed/queued/interrupted görev görüntülenir, yeniden oynatılmaz. Ctrl-N yeni
görevdir. F6 yalnızca yerel bütçeleri değiştirir; model/hesap rotasını değiştirmez.
Terminal ve worker kapanışı normal TUI ile ortaktır.

SDK `PalRun::execute`, `Team`, `MailboxHost` ile kullanılmaya devam eder.
Borrowed container lease gibi scoped runtime için yeni `MailboxExecutor`
kullanılabilir: kendi `PalRun` yürütmesinde sunulan guard'ı
`GuardedObserver {host, guard}` ile bağlamalıdır. Guard host callback öncesi ve
sonrası lease'i yeniden doğrular. Bu bir **trusted host** API'sidir; kendi
host kodunun SDK dışından etki yapmasına karşı güven sınırı değildir.
`GuestTools::into_computer`, `Computer::preserve_workspace_on_drop` ve açık
`release/detach/close` host lifecycle'ını birbirine bağlar. Core, CLI/provider'a
bağımlı değildir. Normal `exec/tui` çağrısına otomatik Pal araçları enjekte edilmez;
Pal akışları açık `pal` komut grubundan veya SDK'den başlatılır.

## Tekrar üretilebilir kabul

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --workspace --locked
python3 scripts/tui_smoke.py
python3 scripts/pals_smoke.py --image sha256:IMAGE_ID
cargo run --release -p namzu-computer --example smoke --locked -- sha256:IMAGE_ID
```

Kabul sonucu ve kapsam [PALS-ACCEPTANCE.md](PALS-ACCEPTANCE.md) içindedir.
