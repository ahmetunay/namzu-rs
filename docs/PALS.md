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
model kullanılmaz. 2.4 mesaj kuyruğu, 2.5 bilgisayarlar ve 2.6 CLI/TUI bağlantıları henüz yoktur.


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
  bağlar. Görev ID'si aynı Pal içinde tekrar kullanılamaz. Otomatik işleyici yoktur.
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

## Güncel kabul

2.1–2.3 için toplam **135 Rust testi**, format ve workspace Clippy doğrulanır.
Çocuk/ekip/kalıcı Pal örneği offline çalışır. Mevcut terminal davranışları ayrıca
25 gerçek Linux PTY senaryosuyla kontrol edilir. Bu Pals aşamaları için canlı
model kotası kullanılmaz. 2.4 otomatik mesaj işleme, 2.5 izole Pal bilgisayarları
ve 2.6 normal CLI/TUI Pals arayüzleri ayrı teslimlerdir.
