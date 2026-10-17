# Pals SDK — yeniden uygulama

## 2.1: çocuk görevler

`namzu-pals` CLI, model sürücüsü ve global kimlik bilgisi deposundan bağımsızdır.
Host gerçek sağlayıcıyı, araçları ve ayrı checkpoint/onay observer'ını verir.
`Delegation::new(owner, parent_context, remaining, parent_tools)` açık bir
delegasyon bütçesi kurar; `prepare(TaskSpec)` tek kullanımlık `ChildTask` üretir.
`ChildTask::run` yeni, yalnızca çocuk görevini içeren konuşma döndürür.
`TaskResult` sahip/görev kimliği ve typed `Result<Turn, AgentError>` taşır.

- Host `remaining` alanına özgün turn tavanını değil, delegasyona ayırdığı
  **kalan** bütçeyi vermelidir. Bu SDK çağıranın diğer işlerini otomatik ölçmez.
- Çocukların adım/araç/metin bütçeleri ortak havuzdan atomik rezerve edilir.
  Toplam rezervasyon tavanı aşamaz. Hata/iptal/drop sonrası iade yoktur;
  aynı havuzda tekrar deneyerek bütçe yenilenmez. En fazla 64 görev vardır.
- Görev hazırlanırken süre başlar; kuyruk/kurulum süresi de dahildir. Çocuğun
  mutlak deadline'ı ebeveyninkini aşamaz. Parent iptali aşağı yayılır; çocuk
  iptali parent veya kardeşleri iptal etmez.
- Varsayılan araçlar `read_file/list_files/search_files/diff_file` ile parent
  araçlarının kesişimidir. Açık araç listesi de parent kabulünün kesişimidir.
  Tanımlar ve gerçek dispatch daraltılır; onay callback'i korunur.
- Host, aynı araç adını farklı yetkiye sahip bir uygulamayla değiştirmemelidir.
  Sağlayıcı/observer/araç uygulamaları güvenilen SDK kodudur; kötücül Rust koduna
  karşı süreç izolasyonu yoktur. Host shell izni OS sandbox'ı değildir.
- Önceki konuşma, model kimlik bilgileri, yazma/shell veya delegasyon aracı
  kendiliğinden aktarılmaz. Kalıcı session ve bilgisayarlar ayrı aşamalardır.

Örnek kullanım `crates/pals/tests/children.rs` içindedir. Testler gerçek yerel
dosya araçlarını ve scripted sağlayıcıyı kullanır; canlı model/kota kullanmaz.

## 2.2: paralel ekipler

`Team::new` ayrı delegasyon havuzu/iptal alt ağacı ve 1–16 concurrency seçer.
`team.prepare` görevleri ilk etkiden önce kabul eder; `Job::new` her görev için
ayrı owned sağlayıcı/araç/observer taşır. `team.run(jobs)` en fazla 64 görevi
sınırlı worker havuzunda çalıştırır ve sonuçları **girdi sırasıyla** verir.
Bir görevin hata/panic'i diğer görevlerin sonuçlarını kaybettirmez.

`TeamControl` owner eşleşmeden ekip/görev iptalini reddeder. Başka bir ekibin
Job'u, owner aynı olsa bile scope kimliğiyle ilk etkiden önce reddedilir.
Host bu owner bilgisini doğrulanmış çağırandan sağlamalıdır; string bir OS
kimlik doğrulama sistemi değildir. Kontrol nesneleri model mesajına verilmez.
Ekip iptali çalışan ve kuyruktaki çocuklara yayılır; tek görev iptali kardeşlere
yayılmaz. Ekip drop'u alt ağacı iptal eder. Çalıştırma dönmeden önce tüm başlayan
worker'lar join edilir; arka planda unutulan thread yoktur. Kuyrukta da görev
deadline'ı işlemeye devam eder. Panic payload'ı görev sonucuna eklenmez.

İptal kooperatiftir: hostun verdiği Provider/Tools/Observer context'i kontrol
etmelidir. Keyfi Rust kodu veya takılmış kernel çağrısı zorla öldürülmez.
Host her ekip için ayrı **kalan** bütçe ayırır; ayrı ekip havuzlarının toplamını
takip etme sorumluluğu hosttadır. Paralel host yazmalarının çakışmalarını SDK
birleştirmez; ayrı workspace veya salt-okunur araçlar kullanılır. Container
izolasyonu 2.5'tedir.

`tests/teams.rs` iki sağlayıcının aynı anda gerçekten girdiğini kanal/bariyerle,
concurrency tavanını, sahipliği, kuyruğu, panic/iptali ve join'i sınar. Ücretli
model kullanılmaz. 2.5 bilgisayarlar ve 2.6 CLI/TUI bağlantıları henüz yoktur.


## 2.3: kalıcı Pals ve görev/oturum yaşam döngüsü

`Registry::open(state_root, workspace_root)` iki ayrı, çakışmayan, host tarafından
seçilmiş özel kök ister. Her Pal'ın ayrı 0700 çalışma klasörü ve ayrı özel
metadata/session dizini vardır. Bu **host çıktı çalışma alanıdır**; container
bilgisayarı değildir. Metadata, araçlara verilen workspace'in dışında tutulur.
`PrivateStore::child/open_child` dizin yeteneğine göre çalışır, symlink yaprağını
reddeder; mevcut Pal'ın silinmiş dizini okuma sırasında yeniden oluşturulmaz.

- `create/get/list/update/revision`: kalıcı kimlik/sahip, ad/amaç, route ve dar
  araç profili. `update` beklenen revizyonla atomik karşılaştırır; eski profil
  revizyonları değişmez. Her Pal en fazla 64 revizyon ve 128 görev saklar.
- `queue_task`: açık görev ekler, mevcut profil revizyonunu ve yeni özel session'ı
  bağlar. Görev ID'si aynı Pal içinde tekrar kullanılamaz. Mesaj işleyicisi 2.4'tedir.
- `start_task`: yalnızca queued görev alır; süreçler arası Pal kilidini çalışmanın
  sonuna kadar tutar. Aynı Pal/workspace için iki eşzamanlı sahip olamaz.
- `PalRun::execute`: gerçek SDK model/araç döngüsünü, parent deadline/iptal ve
  verilen turn bütçesiyle çalıştırır. Pinned profil ile host araç kabulünün
  kesişimini uygular; host onayını korur. Session checkpoint'i araçtan önce
  pending kaydını diske yazar, sonuçtan sonra günceller.
- Güncel pause/archive durumu her yeni model isteğinde ve checkpoint/etki
  sınırında yeniden okunur. Bu kooperatif bir kabul kontrolüdür; uçuşta olan
  model isteğini veya tamamlanmış dosya etkisini geri almaz. Güncel profil
  değişiklikleri eski görevin pinned profilini değiştirmez; host araç tavanı
  her çalıştırmada yine geçerlidir. Archive kalıcıdır; dosyaları silmez.
- Başarı `complete`, hata `failed`, iptal `cancelled`, yarım lease drop'u
  `interrupted` olur. Hatalı/iptal/belirsiz görev otomatik tekrar çalıştırılmaz.
- Gerçek süreç çökmesi `running`/pending kaydı bırakır; sonraki görev de bloke
  olur. Host etkileri inceleyip `acknowledge_interruption` çağırabilir. Bu yalnızca
  kaydı kapatır; aracı tekrar yürütmez. Aynı görev tekrar başlatılamaz.
- `queue_followup`, yalnızca completed session'dan **yeni task/session** oluşturur.
  Konuşmayı ve orijinal profil revizyonunu taşır, eski sonuçları değiştirmez.
  Failed/pending oturumdan devam reddedilir. Eski opaque provider state, mevcut
  sağlayıcının kendi route/hesap doğrulamasına tabidir.

Registry kaydı tek atomik JSON içinde profil revizyonları ve görev metadata'sını
saklar; session dosyaları ayrıdır. Yeni session önce yazılır, sonra görev
referansı yayımlanır. Başlatma/bitirmedeki dosyalar arası işlem yarıda kesilirse
running/pending/uyumsuz durum kapalı kalır. Bu çok dosyalı transaction/rollback
veya exactly-once model çağrısı garantisi değildir. Sahipsiz session/dizinler
kalabilir; otomatik silme/çöp toplama bu aşamada yoktur. Özel kayıtlar Git'e
alınmaz. Session başına mevcut 16 MiB özel dosya sınırı geçerlidir.

Host, sağlayıcıyı `PalRun::definition().route` ile aynı hesap/model/endpoint'e
bağlamalı ve **kalan** turn bütçesini vermelidir; Provider trait'inden kimlik
bilgisi keşfedilmez. Pals profili hesap anahtarı içermez. Registry root'unu host
shell'e veya güvenilmeyen araçlara açmak izolasyonu kaldırır. Owner string'i
hostun doğrulanmış çağıranı temsil eder; kendi başına hesap doğrulama değildir.

SDK örneği (tamamen offline):

```sh
cargo run -p namzu-pals --example offline --locked
```

Örnek çocuk ajanı, iki paralel görevi, ayrı çalışma alanında kalıcı Pal'ın gerçek
`read_file` çağrısını ve registry yeniden açıldıktan sonra session sonucunu
kontrol eder. Native Rust'tır; model/API anahtarı/hesap kullanmaz.

2.3 kabulünde gerçek alt süreç araç yazmasından hemen sonra `exit` eder; yeniden
başlatmada pending etkinin korunduğu, replay reddi ve açık kapanış doğrulanır.
Ayrı süreçten kilit çakışması, revizyon/route/sahiplik, pause/archive, onay,
symlink/kayıp çalışma alanı ve başarılı session devamı da test edilir.

## 2.4: kalıcı mesaj kuyruğu ve otomatik işleme

`registry.mailbox(owner, pal_id)` özel ve sahibine bağlı bir alıcı kuyruğu açar.
`MailMessage::operator(id, body)` operatör girişi; `MailSender::Pal { id }` aynı
sahibin aktif Pal'ından yerel iletişimdir. Gönderen kimliğini güvenilen host
sağlar; modelin serbestçe sender/owner seçmesi için bir araç sunulmaz. `body`
modele yalnızca **User** mesajı olarak gider; sender/correlation bilgisi system
komutuna dönüşmez. Her mesaj ayrı task/session açar; `in_reply_to` yalnızca
korelasyondur, eski konuşmayı veya izinleri kendiliğinden taşımaz. Kuyruk dış
servislere mesaj göndermez.

- `send`: sabit mesaj ID'sini hedef Pal içinde yaşam boyu tekilleştirir. Aynı
  ID/sender/body/correlation tekrarında mevcut task ID'sini verir; farklı içerik
  veya gönderen çatışmadır. Duplicate kontrolü pause ve dolu kuyrukta da geçerlidir.
  Yeni mesajın task'ı ve mesaj referansı **aynı atomik manifest** ile yayımlanır.
  Önce özel session yazılır; bu aralıkta çökme yalnızca çalıştırılamayan sahipsiz
  session bırakabilir. Yeniden gönderim ikinci yürütülebilir task oluşturmaz.
- En fazla 128 mesaj ve Pal başına mevcut 128 görev tavanı vardır; body en fazla
  16 KiB, kimlikler en fazla 64 ASCII karakterdir. FIFO eklenme sırasını kullanır.
  Acknowledged kayıtlar dedup için tutulur; otomatik TTL/silme yoktur.
- `claim(worker, duration)`: 1 ms–86400 saniye için kalıcı sahiplik verir. Her yeni
  claim artan generation taşır. `renew` yalnızca canlı/current claim'i uzatır;
  `release` yalnızca henüz başlanmamış queued/complete-session durumunu bırakır.
  Eski generation, başka worker veya başka Pal tokenı reddedilir. Bir Pal'da
  aynı anda bir claim/çalışma sahibi bulunur; farklı Pals ayrı çalışabilir.
  `claim` için `None`, boş kuyruk yanında mevcut claim/inceleme beklenmesini de
  belirtir; ayrıntı `messages()` durumlarından okunur.
- `process_one`: ilk uygun mesajı alıp pinned profil ile gerçek SDK döngüsünü
  yürütür, sonra sonucu duruma göre ack eder. `MailboxHost::open` doğru route,
  workspace, izin tavanı ve observer'ı kurar. Ham `start_task` mailbox task'ını
  çalıştıramaz: teslimat lease'inin kontrolünden geçmek zorundadır.
- `drain`: hostun açıkça başlattığı otomatik, sonlu işleyicidir. Idle, mesaj sayısı,
  altyapı hatası veya iptalde durur. Task hataları ayrı teslim sonucu olarak kalır. `BatchBudget` varsayılan dört mesaj/adet 16 adım/16 araç
  ve 256 KiB'dir. Worst-case toplam rezervasyon ilk etkiden önce `total` bütçeye
  sığmalıdır; süre tüm batch için tek parent deadline'ıyla sınırlıdır. Önceki
  teslim sonuçları ve durma nedeni `BatchResult` içinde korunur. Hatalı model/araç
  işi kendiliğinden tekrar denenmez. Arka planda kopuk thread/daemon oluşturmaz;
  sürekli polling/CLI/TUI sürücüsü 2.6'da bağlanacaktır.
- `ack`: yalnızca diskte terminal task'ı kapatır ve aynı claim için idempotenttir.
  Complete/failed/cancelled/interrupted sonucu modele geri dönmeden saklanır.
  Pending araç veya session/status uyuşmazlığı varsa `NeedsInspection` olur.

İşleme model/araç isteği ve checkpoint/etki sınırlarında lease'i tekrar kontrol
eder; host callback'lerinden sonra pause/archive ve lease kontrolleri de yapılır.
Expiry, canlı worker'ın süreç kilidini devralmaya izin vermez. Böylece eski
worker'ın uçuşta olan işi ile yeni worker aynı Pal üzerinde yarışmaz. İptal ve
etki sınırları kooperatiftir; keyfi host Rust kodu/kernel çağrısı zorla kesilmez.
Varsayılan worker lease'i kalan turn süresini kapsar; elle kısa lease alan host
uzun işlemler için kendisi renew etmelidir. Lease expiry uçuşta olan model
isteğini geri almaz ve OS sandbox değildir.

`recover` ve `claim`, süre aşmış teslimatları Pal süreç kilidini aldıktan sonra
şu kuralla uzlaştırır:

| Çökme sınırı | Yeniden açıldığında davranış |
| --- | --- |
| Task/mesaj manifest'i yayımlanmadan | Referans yok; aynı mesaj yeniden gönderilebilir |
| Claim alınmış, pristine queued session; işe başlanmamış | Aynı task yeni generation ile alınabilir |
| Model çalışması başlamış/running; pending araç olmasa da | `NeedsInspection`; otomatik replay yok |
| Araç etkisi olmuş, sonuç checkpoint'i yok | Pending kayıt korunur; `NeedsInspection` |
| Task terminal, tutarlı session; yalnızca ack eksik | Modele/araçlara dönmeden ack edilir |

`NeedsInspection` aynı Pal'daki sonraki teslimatı da durdurur. Etkileri inceleyen
host `resolve_inspection(message_id)` ile bu görevi interrupted/acknowledged
olarak kapatabilir; pending kanıtı korunur, araç tekrar çalıştırılmaz. İşlenmiş
mesajların ID'leri bu kapanışla yeniden kullanılamaz. El ile inceleme öncesi
otomatik retry/rollback veya exactly-once uzak model çağrısı iddiası yoktur.

Lease süreleri kalıcı UTC milisaniye kullanır. `MailboxClock` güvenilen host clock
adaptörüdür; testlerde sayaç ilerletilerek expiry sınanır, timer yarışı/sleep
kullanılmaz. Sistem saatinin geriye gitmesi expiry toparlanmasını geciktirebilir;
generation ve süreç kilidi denetimleri korunur. Eski 2.3 manifest'leri eksik
`mailbox` alanını boş kabul ederek okunur; yeni alanları eski binary'ye açma
uyumluluğu vaat edilmez. Kuyruk/mesaj/session gövdeleri özel state'tir, GitHub'a
ve telemetriye yüklenmez.

Çalışan, tamamen offline Rust örneği:

```sh
cargo run -p namzu-pals --example mailbox --locked
```

Örnek operatör ve Pal mesajını otomatik işler, gerçek `read_file` ile iki okuma
yapar; duplicate girişi ve registry yeniden açıldıktan sonra ack kayıtlarını
kontrol eder. Gerçek model/kota veya dış mesaj servisi kullanmaz.

## Güncel kabul

2.4 sonrasında toplam **157 Rust testi** geçti. 22 yeni mailbox testi: gerçek
alt süreçte beş çökme sınırı (yayınlama öncesi, claim sonrası, sağlayıcı içinde,
araç etkisi sonrası, task finish/ack arası); dedup/FIFO/yeniden başlatma;
eski/yanlış lease, sahiplik, renew/release/ack, pause, onay, aggregate bütçe,
iptal ve bozuk binding kontrolü. Mevcut Pals/terminal/provider testleri de geçti.
Format, workspace Clippy ve release derlemesi doğrulanır. Mailbox örneği offline
çalışır; bu aşamada canlı model kotası kullanılmaz. Önceki TUI 1.4 / Pals 2.3
kabulünde 25 gerçek Linux PTY senaryosu geçmişti; bu aşama TUI kodunu değiştirmez.
2.5 izole Pal bilgisayarları ve 2.6 normal CLI/TUI Pals arayüzleri ayrı teslimlerdir.
