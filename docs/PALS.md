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
2.2 ekipler, 2.3 kalıcı kayıt, 2.4 mesaj kuyruğu, 2.5 container bilgisayarları
ve 2.6 CLI/TUI bağlantıları henüz bu ilk teslimde yoktur.
