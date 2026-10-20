# Jobq — kör teslim ve bağımsız checker sonucu, 2026-10-20

Durum: **checker tamamlandı; iki ajan da 8/8 görünür + 31/31 gizli testi geçti**.
Doğruluk kullanıcı tarafından sonradan sağlanan, değiştirilmemiş checker ile
doğrulandı. Exit 0 veya ajanın kendi test beyanı görev başarısı sayılmadı. Gönderilen task
paketi iki ayrı temiz workspace'e aynı dosya hash'leriyle açıldı. Aynı prompt,
GPT-6-Luna/low ve 300 saniye bütçe kullanıldı; birer koşu yapıldı. Rastgele sıra
Jcode → Namzu-RS; seed, zaman damgaları ve checker sonucu
[tamamlanmış JSON kaydında](../bench/jobq-checked-2026-10-20.json).
[Checker öncesi kayıt](../bench/jobq-pending-2026-10-20.json) değiştirilmeden korundu.
Bu tek tekrar ABBA veya tekrarlı sıra dengelemesi değildir.

| Ölçüm | Namzu-RS | Jcode |
| --- | --- | --- |
| Süre | 38.942 s | 77.468 s |
| wait4 bireysel high-water RSS | 14.75 MiB | 46.62 MiB |
| Örneklenen süreç ağacı peak RSS toplamı | 28.02 MiB | 70.13 MiB |
| Süreç exit code | 0 | 0 |
| Görünür test dosyaları | Değişmedi | Değişmedi |
| Görünür testler | **8/8** | **8/8** |
| Gizli testler | **31/31** | **31/31** |
| Bağımsız checker sonucu | **GEÇTİ** | **GEÇTİ** |

Her iki ajan yalnızca jobq/cli.py, jobq/jobqueue.py ve jobq/store.py değiştirdi.
Teslim arşivlerinin önceki SHA-256 değerleri doğrulandı; yeni açılmış kopyalara
checker uygulandı. Önce/sonra dosya manifestleri eşleşti. Çözümlere çalışma
bittikten sonra müdahale edilmedi; checker değişmedi, ajanlar yeniden çalıştırılmadı
ve model çağrısı yapılmadı. İlk süre/RSS/token ölçümleri aynen korundu.

Süreç ağacı ölçümü: 100 ms aralıklarla root ve gözlenen recursive çocukların
RSS toplamı. Paylaşılan sayfalar birden fazla sayılabilir; kısa pikler ve yeniden
parent edilen/kaçan çocuklar gözden kaçabilir. PSS veya tam OS izolasyonu değildir.
Önceki wait4 peak ölçümüyle aynı metrik diye birleştirilmemeli.

Namzu'nun token verisi SDK toplamları ve alan başına samples kapsamını içerir.
Jcode'un CLI usage alanları olduğu gibi kaydedildi; bütün model isteklerini
kapsadığı doğrulanmadı. Bu iki şema eşdeğer toplam token tüketimi diye
karşılaştırılmaz. İlk model isteğine kadar süre iki tarafta eşdeğer biçimde
ölçülmediği için null; tahminle doldurulmadı. Bu görevde ek READY turu yapılmadı.

Teslimler ayrı ZIP ve dosya SHA-256 manifestleriyle checker gelmeden sabitlendi.
Kaynak/input ZIP'leri, özel ham çıktılar ve runner cloud artifacts'te tutulur;
bu public depoya görev kaynakları, çözümler veya özel agent logları yüklenmedi.
Ölçüm raporu ve arşiv hash'leri bu depoda yedeklendi. Kullanıcıya sunulan
jobq-frozen-results.zip paketi input ve iki çözüm arşivi, manifestler, rapor ve
ölçüm runner'ını içerir. ZIP/bütünlük kanıtı cloud artifacts'tedir.

Paket SHA-256: `3dcba1898d495fc8dd888419abca1986ca3cbaa1ac488974465c6cd0c895e230`.
Checker geldikten sonra ZIP hash'leri tekrar doğrulanıp çözümlerin yeni açılmış
kopyaları sınandı; dondurulmuş teslimler ve ilk ölçüm kaydı korundu.


## Checker doğrulaması ve kalibrasyon

Checker'ın özgün görünür test dosyaları ilk task paketiyle byte-for-byte eşleşti.
Önce paketin referans çözümü ve ilk hatalı baseline çalıştırıldı; sonra iki ajan
sınandı. Her koşuda tam 8 görünür ve 31 gizli test keşfi zorunlu tutuldu; eksik
keşif veya çalışmayan test paketi başarı sayılmadı.

| Kontrol | Görünür | Gizli | İkili sonuç |
| --- | --- | --- | --- |
| Checker'ın referans çözümü | 8/8 | 31/31 | Geçti |
| Task'ın ilk hatalı kodu | 4/8 | 8/31 | Kaldı |
| Dondurulmuş Namzu-RS teslimi | 8/8 | 31/31 | Geçti |
| Dondurulmuş Jcode teslimi | 8/8 | 31/31 | Geçti |

İlk iki satır checker README'sinde bildirilen kalibrasyonla eşleşti. Checker
kaynakları, referans çözümü ve gizli test içerikleri public depoya eklenmedi.
Checker arşivi/kaynak manifesti hash'i, kalibrasyon, değerlendirme zamanı ve
sabit teslim hash'leri tamamlanmış JSON'da kayıtlı. Model çağrısı sayısı bu
puanlamada **0**; puanlamanın çalışma süresi önceki ajan süresine eklenmedi.

Sonuç: bu görev ve tek koşuda iki ajan aynı checker doğruluğunu elde etti;
Namzu-RS yaklaşık 38.9 s, Jcode 77.5 s sürdü. Tek görev/tek tekrar genel ürün
üstünlüğü veya istatistiksel anlamlılık iddiası değildir.

Ayrı jobq-checked-results.zip; dokunulmamış önceki frozen paketi, iki ham checker
sonucunu ve tamamlanmış sayısal değerlendirmeyi içerir. Gizli checker kaynakları
bu indirme paketine eklenmedi. Paket SHA-256:
`b93b8f07685321a26fde93b5fac54d9696b63c9d7a34876d4de9b48f569f6b6e`.
