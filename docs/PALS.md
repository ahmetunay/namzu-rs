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
model kullanılmaz. 2.3 kalıcı kayıt, 2.4 mesaj kuyruğu, 2.5 bilgisayarlar ve 2.6
CLI/TUI bağlantıları bu teslimde henüz yoktur.
